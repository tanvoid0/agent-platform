import json
import logging
import os
from typing import Any, Dict, List, Optional

logger = logging.getLogger(__name__)

import httpx
from pydantic import ValidationError

from dag_schema import (
    SubagentsOnly,
    planner_dag_to_json_dict,
    sanitize_llm_model_alias,
    validate_planner_dag,
)
from context_budget import (
    fit_chat_messages_for_request,
    max_output_tokens_default,
    subdag_parent_output_max_tokens,
    tool_result_soft_cap_tokens,
    truncate_text_to_tokens,
)
from orchestrator_env import (
    orchestrator_base_url_v1,
    orchestrator_http_timeout_seconds,
    orchestrator_master_key,
)
from tool_context import ToolContext
from tool_handlers import run_tool_async, tools_filtered_by_allowlist


def usage_cost_from_completion_response(data: Dict[str, Any]) -> float:
    """
    USD cost from an OpenAI-compatible chat completion JSON body.

    Supports common shapes from LiteLLM proxy, OpenRouter, and similar:
    usage.cost, usage.total_cost, usage.response_cost, top-level response_cost,
    _hidden_params.response_cost (LiteLLM Python SDK echo; some proxies omit).
    """
    def _coerce(v: Any) -> Optional[float]:
        if v is None:
            return None
        if isinstance(v, bool):
            return None
        if isinstance(v, (int, float)):
            return float(v)
        if isinstance(v, str):
            s = v.strip()
            if not s:
                return None
            try:
                return float(s)
            except ValueError:
                return None
        return None

    usage = data.get("usage") if isinstance(data.get("usage"), dict) else {}
    for key in ("cost", "total_cost"):
        if key in usage:
            c = _coerce(usage.get(key))
            if c is not None:
                return c

    rc = usage.get("response_cost")
    if isinstance(rc, dict):
        for key in ("total_cost", "cost"):
            if key in rc:
                c = _coerce(rc.get(key))
                if c is not None:
                    return c
    else:
        c = _coerce(rc)
        if c is not None:
            return c

    top = data.get("response_cost")
    if isinstance(top, dict):
        for key in ("total_cost", "cost"):
            if key in top:
                c = _coerce(top.get(key))
                if c is not None:
                    return c
    else:
        c = _coerce(top)
        if c is not None:
            return c

    hidden = data.get("_hidden_params")
    if isinstance(hidden, dict):
        c = _coerce(hidden.get("response_cost"))
        if c is not None:
            return c

    return 0.0


def _default_planner_model() -> str | None:
    """Planner model from env, or None to omit `model` and use orchestrator default."""
    v = (os.getenv("PLANNER_MODEL") or "").strip()
    if not v:
        return None
    return sanitize_llm_model_alias(v)


def _plan_max_attempts() -> int:
    """Total LLM calls allowed for planning when JSON/schema validation fails (min 1)."""
    raw = (os.getenv("AGENT_PLATFORM_PLAN_MAX_ATTEMPTS") or "3").strip()
    try:
        n = int(raw)
        return max(1, n)
    except ValueError:
        return 3


def _planner_fallback_model() -> Optional[str]:
    """Optional stronger alias used only on the last attempt if set and distinct from PLANNER_MODEL."""
    fb = sanitize_llm_model_alias((os.getenv("PLANNER_FALLBACK_MODEL") or "").strip() or None)
    if not fb:
        return None
    primary = _default_planner_model()
    if primary is not None and fb == primary:
        return None
    return fb


def _model_for_plan_attempt(
    attempt: int, max_attempts: int, fallback: Optional[str]
) -> str | None:
    if fallback and attempt == max_attempts - 1:
        return fallback
    return _default_planner_model()


def _default_subagent_model() -> str | None:
    sub = (os.getenv("SUBAGENT_MODEL") or "").strip()
    if sub:
        return sanitize_llm_model_alias(sub)
    return _default_planner_model()


class LLMConfigurationError(RuntimeError):
    """Missing or invalid local configuration (no secrets in message)."""


class LLMAuthenticationError(RuntimeError):
    """Orchestrator rejected credentials (no secrets in message)."""


class LLMTransportError(RuntimeError):
    """Network or unreachable orchestrator (no secrets in message)."""


class LLMRequestError(RuntimeError):
    """Orchestrator returned an error HTTP status (message may include a truncated response body)."""


def _orchestrator_http_error_message(status_code: int, response: httpx.Response) -> str:
    parts = [f"Orchestrator request failed with HTTP {status_code}."]
    if status_code == 404:
        model_missing = False
        try:
            j = response.json()
            err = j.get("error") if isinstance(j, dict) else None
            msg = (err.get("message") or "") if isinstance(err, dict) else ""
            if "model" in msg.lower() and "not found" in msg.lower():
                model_missing = True
        except Exception:
            pass
        if model_missing:
            parts.append(
                "Unknown model alias: set PLANNER_MODEL / SUBAGENT_MODEL to a name from "
                "GET /v1/models on the orchestrator, or define that alias in llm-orchestrator config.yaml, "
                "or unset those env vars to use the orchestrator default. "
                "Wrong LLM_ORCHESTRATOR_BASE_URL (not ending in /v1) also returns 404 — see .env.example."
            )
        else:
            parts.append(
                "Use LLM_ORCHESTRATOR_BASE_URL ending in /v1 (e.g. http://127.0.0.1:18408/v1). "
                "If the URL is correct, the upstream may return 404 for an unknown model."
            )
    try:
        t = response.text.strip()
        if t:
            if len(t) > 400:
                t = t[:397] + "..."
            parts.append(t)
    except Exception:
        pass
    return " ".join(parts)


async def call_llm(
    messages: list[Dict[str, str]],
    model: str | None = None,
    require_json: bool = False,
    temperature: float = 0.7,
    max_output_tokens: int | None = None,
) -> tuple[str, int, float]:
    """
    Calls the orchestrator proxy and returns (content, total_tokens, cost_usd).

    cost_usd is 0.0 when the upstream response omits cost fields (e.g. plain Ollama).
    """
    key = orchestrator_master_key()
    if not key:
        raise LLMConfigurationError(
            "ORCHESTRATOR_MASTER_KEY is not set. Add it to agent-platform/.env (same value as "
            "on llm-orchestrator). See .env.example."
        )

    base = orchestrator_base_url_v1()
    url = f"{base}/chat/completions"
    headers = {
        "Authorization": f"Bearer {key}",
        "Content-Type": "application/json",
    }

    raw_model = (model or "").strip() or _default_subagent_model()
    resolved_model = sanitize_llm_model_alias(raw_model)
    fitted, _prompt_budget = fit_chat_messages_for_request([dict(m) for m in messages])
    out_messages: list[Dict[str, str]] = []
    for m in fitted:
        role = m.get("role")
        c = m.get("content")
        out_messages.append(
            {
                "role": str(role),
                "content": c if isinstance(c, str) else (str(c) if c is not None else ""),
            }
        )
    payload: Dict[str, Any] = {
        "messages": out_messages,
        "temperature": temperature,
    }
    if resolved_model:
        payload["model"] = resolved_model

    out_cap = max_output_tokens if max_output_tokens is not None else max_output_tokens_default()
    payload["max_tokens"] = out_cap

    if require_json:
        payload["response_format"] = {"type": "json_object"}

    _timeout = orchestrator_http_timeout_seconds()
    try:
        async with httpx.AsyncClient(timeout=_timeout) as client:
            response = await client.post(url, headers=headers, json=payload)
            response.raise_for_status()
    except httpx.HTTPStatusError as e:
        if e.response.status_code == 401:
            raise LLMAuthenticationError(
                "Orchestrator returned 401: set ORCHESTRATOR_MASTER_KEY in agent-platform/.env to "
                "exactly match llm-orchestrator."
            ) from None
        raise LLMRequestError(_orchestrator_http_error_message(e.response.status_code, e.response)) from None
    except httpx.RequestError as e:
        raise LLMTransportError(
            f"Could not reach the LLM orchestrator at {base}. "
            "Start llm-orchestrator (e.g. docker compose in llm-orchestrator on port 18408). "
            "In Docker, use http://host.docker.internal:18408/v1 or leave "
            "LLM_ORCHESTRATOR_BASE_URL=http://127.0.0.1:18408/v1 (auto-rewritten in containers)."
        ) from e

    data = response.json()
    content = data["choices"][0]["message"]["content"]
    tokens = data.get("usage", {}).get("total_tokens", 0)
    cost = usage_cost_from_completion_response(data)
    return content, tokens, cost


def _truncate_for_prompt(text: str, max_tokens: int | None = None) -> str:
    mt = subdag_parent_output_max_tokens() if max_tokens is None else max_tokens
    return truncate_text_to_tokens(str(text).strip(), mt)


async def call_llm_with_tools(
    messages: List[Dict[str, Any]],
    model: str | None = None,
    *,
    allowed_tool_names: frozenset[str],
    tool_budget: int,
    temperature: float = 0.7,
    tool_context: ToolContext | None = None,
) -> tuple[str, int, float, int]:
    """
    Multi-round chat with OpenAI-compatible tool_calls. Respects `tool_budget` invocations
    per call; after the budget is exhausted, subsequent requests omit `tools` so the model
    must answer in plain text.

    Returns (final_assistant_text, total_tokens, total_cost, tool_invocations).
    """
    if tool_budget <= 0 or not allowed_tool_names:
        slim: list[Dict[str, str]] = []
        for m in messages:
            if m.get("role") not in ("system", "user"):
                continue
            c = m.get("content")
            if not isinstance(c, str):
                continue
            slim.append({"role": str(m["role"]), "content": c})
        content, tokens, cost = await call_llm(slim, model=model, temperature=temperature)
        return content, tokens, cost, 0

    tools = tools_filtered_by_allowlist(allowed_tool_names)
    if not tools:
        slim = [
            {"role": str(m["role"]), "content": str(m.get("content", ""))}
            for m in messages
            if m.get("role") in ("system", "user") and isinstance(m.get("content"), str)
        ]
        content, tokens, cost = await call_llm(slim, model=model, temperature=temperature)
        return content, tokens, cost, 0

    key = orchestrator_master_key()
    if not key:
        raise LLMConfigurationError(
            "ORCHESTRATOR_MASTER_KEY is not set. Add it to agent-platform/.env (same value as "
            "on llm-orchestrator). See .env.example."
        )

    base = orchestrator_base_url_v1()
    url = f"{base}/chat/completions"
    headers = {
        "Authorization": f"Bearer {key}",
        "Content-Type": "application/json",
    }

    raw_model = (model or "").strip() or _default_subagent_model()
    resolved_model = sanitize_llm_model_alias(raw_model)
    conversation: List[Dict[str, Any]] = [dict(m) for m in messages]
    http_timeout = orchestrator_http_timeout_seconds()

    total_tokens = 0
    total_cost = 0.0
    invocations = 0

    for _ in range(48):
        conversation, _budget = fit_chat_messages_for_request(conversation)
        payload: Dict[str, Any] = {
            "messages": conversation,
            "temperature": temperature,
            "max_tokens": max_output_tokens_default(),
        }
        if resolved_model:
            payload["model"] = resolved_model
        if invocations < tool_budget:
            payload["tools"] = tools
            payload["tool_choice"] = "auto"
        # When the budget is exhausted, omit tools so the model must answer in plain text.

        try:
            async with httpx.AsyncClient(timeout=http_timeout) as client:
                response = await client.post(url, headers=headers, json=payload)
                response.raise_for_status()
        except httpx.HTTPStatusError as e:
            if e.response.status_code == 401:
                raise LLMAuthenticationError(
                    "Orchestrator returned 401: set ORCHESTRATOR_MASTER_KEY in agent-platform/.env to "
                    "exactly match llm-orchestrator."
                ) from None
            raise LLMRequestError(
                _orchestrator_http_error_message(e.response.status_code, e.response)
            ) from None
        except httpx.RequestError as e:
            raise LLMTransportError(
                f"Could not reach the LLM orchestrator at {base}. "
                "Start llm-orchestrator (e.g. docker compose in llm-orchestrator on port 18408). "
                "In Docker, use http://host.docker.internal:18408/v1 or leave "
                "LLM_ORCHESTRATOR_BASE_URL=http://127.0.0.1:18408/v1 (auto-rewritten in containers)."
            ) from e

        data = response.json()
        usage = data.get("usage") or {}
        total_tokens += int(usage.get("total_tokens", 0) or 0)
        total_cost += usage_cost_from_completion_response(data)

        msg = data["choices"][0]["message"]
        tool_calls = msg.get("tool_calls") or []

        if not tool_calls:
            content = msg.get("content") or ""
            return str(content), total_tokens, total_cost, invocations

        conversation.append(msg)

        for tc in tool_calls:
            fn = tc.get("function") or {}
            name = (fn.get("name") or "").strip()
            args = fn.get("arguments")
            if not isinstance(args, str):
                args = json.dumps(args) if args is not None else "{}"
            tid = tc.get("id") or ""
            if invocations >= tool_budget:
                result = json.dumps({"error": "tool_budget_exhausted"})
            else:
                result = await run_tool_async(name, args, tool_context)
                invocations += 1
            if not isinstance(result, str):
                result = json.dumps(result) if result is not None else ""
            result = truncate_text_to_tokens(result, tool_result_soft_cap_tokens())
            conversation.append(
                {
                    "role": "tool",
                    "tool_call_id": tid,
                    "content": result,
                }
            )

    content = ""
    return content, total_tokens, total_cost, invocations


async def generate_subdag_expansion(
    *,
    run_goal: str,
    parent_uuid: str,
    parent_role: str,
    parent_output: str,
    existing_uuids: set[str],
) -> tuple[list[dict[str, Any]], int, float]:
    """
    Ask the planner model for additional subagents that depend on `parent_uuid`.
    Returns (new_subagents as JSON dicts, tokens, cost).

    On invalid JSON or validation failure, retries like `generate_planner_dag`
    (``AGENT_PLATFORM_PLAN_MAX_ATTEMPTS``, ``PLANNER_FALLBACK_MODEL`` on last attempt).
    """
    system_prompt = f"""You extend an existing execution DAG. Output JSON only with this shape:
{{
  "subagents": [
    {{
      "client_uuid": "new unique id (never reuse existing UUIDs)",
      "role": "short role name",
      "system_prompt": "identity and boundaries",
      "instructions": "single, concrete deliverable for this subagent",
      "dependencies": ["must include "{parent_uuid}" and may include other new UUIDs"],
      "model": "optional real orchestrator alias only; omit unless you know the proxy name (never role slugs like typescript-expert or react-scaffolder)",
      "subdecompose": "optional boolean; true if this subagent's output may justify further split tasks later",
      "requires_review": "optional boolean; true only if human gate needed before dependents run"
    }}
  ]
}}
Rules:
- Prefer **many small parallel subagents** over one large step: each node should complete one clear artifact
  (file slice, API section, research angle, test batch, doc section). Peers that do not depend on each
  other should **not** list each other—only list "{parent_uuid}" or prior new UUIDs they truly need.
- At least one new subagent (more is better when the parent output has separable work).
- Every new subagent MUST list "{parent_uuid}" in dependencies (direct dependency on the parent task).
- client_uuid values must be unique and MUST NOT be any of the existing UUIDs.
- Dependencies may only reference "{parent_uuid}" or client_uuids from your new subagents (acyclic).
"""
    user_blob = (
        f"Process goal:\n{run_goal}\n\n"
        f"Parent task UUID: {parent_uuid}\n"
        f"Parent role: {parent_role}\n\n"
        f"Existing UUIDs (do not reuse):\n{', '.join(sorted(existing_uuids))}\n\n"
        f"Parent output to decompose:\n{_truncate_for_prompt(parent_output)}"
    )
    messages = [
        {"role": "system", "content": system_prompt},
        {"role": "user", "content": user_blob},
    ]

    max_attempts = _plan_max_attempts()
    fallback = _planner_fallback_model()
    total_tokens = 0
    total_cost = 0.0
    last_err: Exception | None = None

    for attempt in range(max_attempts):
        model = _model_for_plan_attempt(attempt, max_attempts, fallback)
        content, tokens, cost = await call_llm(
            messages,
            model=model,
            require_json=True,
            temperature=0.1,
        )
        total_tokens += tokens
        total_cost += cost
        try:
            raw = json.loads(content)
            partial = SubagentsOnly.model_validate(raw)
            out: list[dict[str, Any]] = []
            for spec in partial.subagents:
                if spec.client_uuid in existing_uuids:
                    raise ValueError(
                        f"Sub-decomposition reused existing client_uuid: {spec.client_uuid!r}"
                    )
                if parent_uuid not in spec.dependencies:
                    raise ValueError(
                        f"Sub-decomposition subagent {spec.client_uuid!r} must depend on parent "
                        f"{parent_uuid!r}"
                    )
                out.append(spec.model_dump(mode="json", by_alias=True))
            return out, total_tokens, total_cost
        except json.JSONDecodeError as e:
            last_err = e
            logger.warning(
                "Sub-decomposition attempt %s/%s: invalid JSON (%s)",
                attempt + 1,
                max_attempts,
                e,
            )
        except ValidationError as e:
            last_err = e
            logger.warning(
                "Sub-decomposition attempt %s/%s: payload validation failed (%s)",
                attempt + 1,
                max_attempts,
                e,
            )
        except ValueError as e:
            last_err = e
            logger.warning(
                "Sub-decomposition attempt %s/%s: invalid expansion (%s)",
                attempt + 1,
                max_attempts,
                e,
            )

    detail = str(last_err) if last_err else "unknown error"
    raise ValueError(
        f"Sub-decomposition planner failed after {max_attempts} attempt(s) (JSON/schema). "
        f"Last error: {detail}"
    ) from last_err


async def generate_planner_dag(
    goal: str, team_context: str | None = None
) -> tuple[Dict[str, Any], int, float]:
    """
    Uses the Planner-Executor pattern to generate a DAG representation
    of the team and tasks required to accomplish the goal.

    On invalid JSON or schema validation failure, retries up to ``AGENT_PLATFORM_PLAN_MAX_ATTEMPTS``
    (default 3). The last attempt uses ``PLANNER_FALLBACK_MODEL`` when set and distinct from
    ``PLANNER_MODEL`` when that is set (ADR: stronger model for structured output). If ``PLANNER_MODEL``
    is unset, requests use the orchestrator default unless a fallback alias is configured.
    """
    system_prompt = """You are an elite Agentic Team Planner. Decompose the user's goal into a DAG of subagents.

When the user message includes a "Preferred team roster" section, **prefer** aligning subagent `role` names and
splitting work along those roles when it fits the goal; you may still add or merge roles if the goal requires it.

**Granularity:** Prefer **more, smaller** subagents over few monolithic ones. Each subagent should own one
outcome (research slice, integration step, doc section, test pass, refactor chunk). Put **independent**
work in **separate** nodes with **empty or minimal** dependencies so they can run in parallel in the
same wave when possible. Use dependencies only for true ordering (B needs A's output).

**model field:** Omit `model` unless you use a real orchestrator alias from the server. Never put role titles,
programming languages, or skill labels (e.g. `typescript-expert`, `react-scaffolder`) in `model`—those belong in `role` / prompts only.

**subdecompose:** Set `subdecompose`: true on nodes whose deliverable is likely to reveal follow-on work
after execution (e.g. exploration, broad research, scaffolding) so the system can add subtasks from the
completed output. Omit or false for tight, predictable leaf tasks.

Output valid JSON strictly adhering to this schema:
{
  "team_name": "Name of the team",
  "goal_restatement": "What we are doing",
  "subagents": [
    {
      "client_uuid": "A unique string id like 'agent_1'",
      "role": "e.g. Researcher, Synthesizer",
      "system_prompt": "Identity and boundaries of the agent",
      "instructions": "Specific task for this agent. Mention that it will receive context from dependencies.",
      "dependencies": ["client_uuid_of_prior_agent"],
      "model": "optional; real orchestrator alias only (e.g. gemma4, gemini-flash). Omit to use server default. Never use role or skill slugs (e.g. typescript-expert, react-scaffolder).",
      "subdecompose": "optional boolean; if true, after this node completes the server may append child subtasks from its output (within AGENT_PLATFORM_SUBDECOMP_* limits).",
      "requires_review": "optional boolean; if true, execution pauses for human review after this node's output."
    }
  ]
}
Make sure all dependencies are valid client_uuids from the subagents list. Ensure no circular dependencies.
"""
    user_parts = [f"Goal: {goal}"]
    if team_context and team_context.strip():
        user_parts.append("")
        user_parts.append(team_context.strip())
    messages = [
        {"role": "system", "content": system_prompt},
        {"role": "user", "content": "\n".join(user_parts)},
    ]

    max_attempts = _plan_max_attempts()
    fallback = _planner_fallback_model()
    total_tokens = 0
    total_cost = 0.0
    last_err: Exception | None = None

    for attempt in range(max_attempts):
        model = _model_for_plan_attempt(attempt, max_attempts, fallback)
        content, tokens, cost = await call_llm(
            messages,
            model=model,
            require_json=True,
            temperature=0.1,
        )
        total_tokens += tokens
        total_cost += cost
        try:
            raw = json.loads(content)
            planner = validate_planner_dag(raw)
            return planner_dag_to_json_dict(planner), total_tokens, total_cost
        except json.JSONDecodeError as e:
            last_err = e
            logger.warning(
                "Planner attempt %s/%s: invalid JSON (%s)",
                attempt + 1,
                max_attempts,
                e,
            )
        except ValueError as e:
            last_err = e
            logger.warning(
                "Planner attempt %s/%s: DAG validation failed (%s)",
                attempt + 1,
                max_attempts,
                e,
            )

    detail = str(last_err) if last_err else "unknown error"
    raise ValueError(
        f"Planner failed after {max_attempts} attempt(s) (JSON/schema). Last error: {detail}"
    ) from last_err
