import json
import os
from typing import Any, Dict

import httpx


class LLMConfigurationError(RuntimeError):
    """Missing or invalid local configuration (no secrets in message)."""


class LLMAuthenticationError(RuntimeError):
    """Orchestrator rejected credentials (no secrets in message)."""


class LLMTransportError(RuntimeError):
    """Network or unreachable orchestrator (no secrets in message)."""


class LLMRequestError(RuntimeError):
    """Orchestrator returned an error response (status only; no URL or body)."""


def _orchestrator_base_url() -> str:
    raw = (os.getenv("LLM_ORCHESTRATOR_BASE_URL") or "http://127.0.0.1:18408/v1").strip()
    return raw.rstrip("/")


def _master_key() -> str:
    return (os.getenv("LITELLM_MASTER_KEY") or "").strip()


async def call_llm(
    messages: list[Dict[str, str]],
    model: str = "gemini-flash",
    require_json: bool = False,
    temperature: float = 0.7,
) -> tuple[str, int]:
    """
    Calls the orchestrator proxy and returns (content, total_tokens).
    """
    key = _master_key()
    if not key:
        raise LLMConfigurationError(
            "LITELLM_MASTER_KEY is not set. Add it to agent-platform/.env (same value as "
            "llm-orchestrator). See .env.example."
        )

    base = _orchestrator_base_url()
    url = f"{base}/chat/completions"
    headers = {
        "Authorization": f"Bearer {key}",
        "Content-Type": "application/json",
    }

    payload: Dict[str, Any] = {
        "model": model,
        "messages": messages,
        "temperature": temperature,
    }

    if require_json:
        payload["response_format"] = {"type": "json_object"}

    try:
        async with httpx.AsyncClient(timeout=120.0) as client:
            response = await client.post(url, headers=headers, json=payload)
            response.raise_for_status()
    except httpx.HTTPStatusError as e:
        if e.response.status_code == 401:
            raise LLMAuthenticationError(
                "Orchestrator returned 401: set LITELLM_MASTER_KEY in agent-platform/.env to "
                "exactly match ORCHESTRATOR_MASTER_KEY or LITELLM_MASTER_KEY on llm-orchestrator."
            ) from None
        raise LLMRequestError(
            f"Orchestrator request failed with HTTP {e.response.status_code}."
        ) from None
    except httpx.RequestError:
        raise LLMTransportError(
            "Could not reach the LLM orchestrator. Check LLM_ORCHESTRATOR_BASE_URL "
            "(e.g. host.docker.internal for Docker) and that the service is running."
        ) from None

    data = response.json()
    content = data["choices"][0]["message"]["content"]
    tokens = data.get("usage", {}).get("total_tokens", 0)
    return content, tokens


async def generate_planner_dag(goal: str) -> tuple[Dict[str, Any], int]:
    """
    Uses the Planner-Executor pattern to generate a DAG representation
    of the team and tasks required to accomplish the goal.
    """
    system_prompt = """You are an elite Agentic Team Planner. You must decompose the user's goal into a DAG (Directed Acyclic Graph) of subagents.
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
      "dependencies": ["client_uuid_of_prior_agent"] // Empty list if no dependencies
    }
  ]
}
Make sure all dependencies are valid client_uuids from the subagents list. Ensure no circular dependencies.
"""
    messages = [
        {"role": "system", "content": system_prompt},
        {"role": "user", "content": f"Goal: {goal}"},
    ]

    content, tokens = await call_llm(messages, require_json=True, temperature=0.1)

    try:
        dag = json.loads(content)
        return dag, tokens
    except json.JSONDecodeError:
        raise ValueError(f"Planner failed to return valid JSON. Output: {content}")
