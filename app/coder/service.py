"""Coder agent service: thread CRUD + a tool-calling agent loop per send.

One send = one agent run: the model may issue several tool calls (executed via
a ``ToolExecutor``) before it produces the final assistant text. The loop is
provider-agnostic — it speaks the OpenAI tools format through the LLM proxy,
which routes to Ollama / Gemini / LM Studio / AIMLAPI.
"""

from __future__ import annotations

import json
import os
from typing import Any, AsyncIterator

import httpx
from fastapi import HTTPException
from sqlmodel import Session, select

import database

from coder.executor import (
    APPROVAL_REQUIRED_TOOLS,
    TOOL_SPECS,
    ToolExecutionError,
    ToolExecutor,
    make_executor,
)
from chat_thread_title import (
    await_smart_title,
    fallback_title_from_message,
    is_placeholder_title,
    merge_title_sse_events,
    start_smart_title_task,
)
from chat_usage import (
    ContextUsageOut,
    LlmStepUsageOut,
    LlmUsageOut,
    estimate_context_usage,
    merge_llm_usages,
    parse_llm_usage,
    parse_llm_usage_dict,
)
from coder.models import CoderChatThread
from context_budget import (
    fit_chat_messages_for_request,
    max_output_tokens_default,
    tool_result_soft_cap_tokens,
    truncate_text_to_tokens,
)
from dag_schema import sanitize_llm_model_alias
from llm_proxy_env import (
    llm_proxy_base_url_v1,
    llm_proxy_http_timeout_seconds,
    llm_proxy_master_key,
)
from time_utils import utc_now_naive

CODER_SYSTEM_PROMPT = (
    "You are a coding assistant working directly in the user's workspace via tools.\n"
    "Rules:\n"
    "- All paths are relative to the workspace root.\n"
    "- Explore before you change: use list_dir and read_file to understand code before write_file.\n"
    "- write_file replaces the whole file; always read a file first and write back its full updated content.\n"
    "- Prefer small, targeted changes. Do not rewrite files you were not asked to touch.\n"
    "- When done, summarize what you changed and why in a short final answer."
)


def _max_iterations() -> int:
    raw = (os.getenv("CODER_MAX_ITERATIONS") or "15").strip()
    try:
        return max(1, int(raw))
    except ValueError:
        return 15


def _default_workspace_root() -> str | None:
    root = (os.getenv("CODER_WORKSPACE_ROOT") or "").strip()
    return root or None


def _create_thread_row(
    session: Session,
    *,
    title: str | None = "New session",
    workspace_root: str | None = None,
) -> CoderChatThread:
    now = utc_now_naive()
    row = CoderChatThread(
        title=title, workspace_root=workspace_root, created_at=now, updated_at=now
    )
    row.set_messages([])
    session.add(row)
    session.commit()
    session.refresh(row)
    return row


def _get_thread_by_id(session: Session, thread_id: int) -> CoderChatThread:
    row = session.get(CoderChatThread, thread_id)
    if not row:
        raise HTTPException(status_code=404, detail="Coder thread not found")
    return row


def _resolve_thread(session: Session, thread_id: int | None = None) -> CoderChatThread:
    if thread_id is not None:
        return _get_thread_by_id(session, thread_id)
    row = session.exec(
        select(CoderChatThread).order_by(CoderChatThread.updated_at.desc())
    ).first()
    if row:
        return row
    return _create_thread_row(session)


def create_thread(
    session: Session, *, title: str | None = None, workspace_root: str | None = None
) -> CoderChatThread:
    return _create_thread_row(
        session, title=title or "New session", workspace_root=workspace_root
    )


def list_threads(session: Session) -> list[dict[str, Any]]:
    rows = session.exec(
        select(CoderChatThread).order_by(CoderChatThread.updated_at.desc())
    ).all()
    out: list[dict[str, Any]] = []
    for row in rows:
        messages = row.get_messages()
        preview = ""
        for m in reversed(messages):
            if m.get("role") == "user" and m.get("content"):
                preview = str(m["content"])[:120]
                break
        out.append(
            {
                "id": row.id,
                "title": row.title or "New session",
                "workspace_root": row.workspace_root,
                "message_count": len(messages),
                "preview": preview,
                "created_at": row.created_at.isoformat(),
                "updated_at": row.updated_at.isoformat(),
            }
        )
    return out


def _coder_context_usage(llm_messages: list[dict[str, Any]]) -> ContextUsageOut:
    conversation = [m for m in llm_messages if m.get("role") != "system"]
    return estimate_context_usage(
        system_prompt=CODER_SYSTEM_PROMPT,
        tools=TOOL_SPECS,
        conversation_messages=conversation,
    )


async def get_thread(session: Session, thread_id: int | None = None) -> dict[str, Any]:
    thread = _resolve_thread(session, thread_id)
    llm_messages = _llm_messages_from_history(thread.get_messages())
    ctx = _coder_context_usage(llm_messages)
    return {
        "thread_id": thread.id,
        "title": thread.title or "New session",
        "workspace_root": thread.workspace_root,
        "model": thread.model,
        "messages": thread.get_messages(),
        "context_window": ctx.context_window,
        "context_usage": ctx,
    }


def get_context_usage(session: Session, thread_id: int | None = None) -> ContextUsageOut:
    thread = _resolve_thread(session, thread_id)
    llm_messages = _llm_messages_from_history(thread.get_messages())
    return _coder_context_usage(llm_messages)


def delete_thread(session: Session, thread_id: int) -> None:
    thread = _get_thread_by_id(session, thread_id)
    session.delete(thread)
    session.commit()


def _llm_messages_from_history(history: list[dict[str, Any]]) -> list[dict[str, Any]]:
    llm_messages: list[dict[str, Any]] = [
        {"role": "system", "content": CODER_SYSTEM_PROMPT}
    ]
    for h in history:
        role = h.get("role", "user")
        m: dict[str, Any] = {"role": role, "content": h.get("content") or ""}
        if h.get("tool_calls"):
            m["tool_calls"] = h["tool_calls"]
        if role == "tool" and h.get("tool_call_id"):
            m["tool_call_id"] = h["tool_call_id"]
        llm_messages.append(m)
    return llm_messages


def _build_llm_messages(history: list[dict[str, Any]], message: str) -> list[dict[str, Any]]:
    llm_messages = _llm_messages_from_history(history)
    llm_messages.append({"role": "user", "content": message})
    return llm_messages


def _truncate_history_for_retry(history: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Keep messages through the last non-empty user turn; drop partial assistant/tool tail."""
    last_user: int | None = None
    for i in range(len(history) - 1, -1, -1):
        if history[i].get("role") != "user":
            continue
        if str(history[i].get("content") or "").strip():
            last_user = i
            break
    if last_user is None:
        raise HTTPException(status_code=400, detail="No user message to retry")
    return list(history[: last_user + 1])


def _parse_tool_calls_raw(raw_calls: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Normalize a list of OpenAI-format tool_call dicts.

    ``function.arguments`` is a JSON string per spec, but some local backends
    return it as an object already — accept both.
    """
    calls: list[dict[str, Any]] = []
    for i, tc in enumerate(raw_calls or []):
        fn = tc.get("function") or {}
        name = fn.get("name") or ""
        raw_args = fn.get("arguments")
        if isinstance(raw_args, dict):
            args = raw_args
        else:
            try:
                args = json.loads(raw_args) if raw_args else {}
            except json.JSONDecodeError:
                args = {}
        if not isinstance(args, dict):
            args = {}
        calls.append(
            {
                "id": tc.get("id") or f"call_{i}",
                "name": name,
                "arguments": args,
                "raw": tc,
            }
        )
    return calls


def _parse_tool_calls(message: dict[str, Any]) -> list[dict[str, Any]]:
    return _parse_tool_calls_raw(message.get("tool_calls") or [])


def _normalize_provider(provider: str | None) -> str | None:
    if provider is None:
        return None
    p = provider.strip().lower()
    return p or None


async def _call_llm_step(
    messages: list[dict[str, Any]],
    model: str | None,
    *,
    provider: str | None = None,
    max_tokens: int | None = None,
) -> tuple[dict[str, Any], dict[str, Any] | None]:
    """One non-streaming chat/completions call with tools; returns (assistant message, usage)."""
    key = llm_proxy_master_key()
    if not key:
        raise HTTPException(status_code=503, detail="AGENT_PLATFORM_MASTER_KEY is not set.")

    fitted, _ = fit_chat_messages_for_request(messages)
    payload: dict[str, Any] = {
        "messages": fitted,
        "tools": TOOL_SPECS,
        "max_tokens": max_tokens if max_tokens is not None else max_output_tokens_default(),
    }
    sm = sanitize_llm_model_alias(model) if model else None
    if sm:
        payload["model"] = sm
    prov = _normalize_provider(provider)
    if prov:
        payload["provider"] = prov

    base = llm_proxy_base_url_v1()
    headers = {"Content-Type": "application/json", "Authorization": f"Bearer {key}"}
    try:
        async with httpx.AsyncClient(timeout=llm_proxy_http_timeout_seconds()) as client:
            r = await client.post(f"{base}/chat/completions", headers=headers, json=payload)
    except httpx.RequestError as e:
        raise HTTPException(status_code=502, detail=f"Upstream request failed: {e}") from e

    if r.status_code != 200:
        body_snip = (r.text or "").strip().replace("\n", " ")[:400]
        detail = f"LLM proxy returned HTTP {r.status_code}"
        if body_snip:
            detail = f"{detail}: {body_snip}"
        raise HTTPException(status_code=502, detail=detail)

    data = r.json()
    usage = data.get("usage") if isinstance(data.get("usage"), dict) else None
    choices = data.get("choices") or []
    message = (choices[0].get("message") or {}) if choices else {}
    return message, usage


async def run_agent_turn(
    llm_messages: list[dict[str, Any]],
    new_history: list[dict[str, Any]],
    executor: ToolExecutor,
    model: str | None,
    *,
    provider: str | None = None,
    max_tokens: int | None = None,
    auto_approve_commands: bool = False,
    pending_out: list[dict[str, Any]] | None = None,
    resume_calls: list[dict[str, Any]] | None = None,
    usage_steps_out: list[LlmStepUsageOut] | None = None,
) -> AsyncIterator[tuple[str, dict[str, Any]]]:
    """Run one agent turn; yields (event, data) and appends persisted messages to new_history.

    Events: ``tool_call`` {name, arguments}, ``tool_result`` {name, content},
    ``approval_required`` {call_id, name, arguments}, ``assistant`` {content, usage?}.
    The caller owns persistence of new_history.

    If a tool in ``executor.APPROVAL_REQUIRED_TOOLS`` is hit and
    ``auto_approve_commands`` is False, the turn pauses: the pending call plus
    any not-yet-executed calls from the same batch are appended to
    ``pending_out`` (as ``{call_id, name, arguments, remaining}``) and the
    generator returns without calling the LLM again. Resume by re-invoking
    with ``resume_calls`` set to the parsed remaining calls after resolving
    the pending one.
    """
    calls = resume_calls
    usage_steps: list[LlmStepUsageOut] = []
    step_num = 0
    for _ in range(_max_iterations()):
        if calls is None:
            message, usage = await _call_llm_step(
                llm_messages, model, provider=provider, max_tokens=max_tokens
            )
            step_num += 1
            step_usage = parse_llm_usage_dict(usage, label=f"agent_step_{step_num}")
            usage_steps.append(step_usage)
            content = message.get("content") or ""
            calls = _parse_tool_calls(message)

            assistant_msg: dict[str, Any] = {"role": "assistant", "content": content}
            if calls:
                assistant_msg["tool_calls"] = [c["raw"] for c in calls]
            if usage is not None:
                assistant_msg["usage"] = usage
            llm_messages.append(assistant_msg)
            new_history.append(assistant_msg)

            if not calls:
                turn_usage = merge_llm_usages(usage_steps)
                if usage_steps_out is not None:
                    usage_steps_out.extend(usage_steps)
                yield "assistant", {
                    "content": content,
                    "usage": turn_usage.model_dump(),
                }
                return

        for idx, c in enumerate(calls):
            if c["name"] in APPROVAL_REQUIRED_TOOLS and not auto_approve_commands:
                if pending_out is not None:
                    pending_out.append(
                        {
                            "call_id": c["id"],
                            "name": c["name"],
                            "arguments": c["arguments"],
                            "remaining": [cc["raw"] for cc in calls[idx + 1 :]],
                        }
                    )
                yield "approval_required", {
                    "call_id": c["id"],
                    "name": c["name"],
                    "arguments": c["arguments"],
                }
                return

            yield "tool_call", {
                "call_id": c["id"],
                "name": c["name"],
                "arguments": c["arguments"],
            }
            result = await executor.execute(c["name"], c["arguments"], call_id=c["id"])
            result = truncate_text_to_tokens(result, tool_result_soft_cap_tokens())
            tool_msg = {
                "role": "tool",
                "tool_call_id": c["id"],
                "name": c["name"],
                "content": result,
            }
            llm_messages.append(tool_msg)
            new_history.append(tool_msg)
            yield "tool_result", {"name": c["name"], "content": result}

        calls = None

    turn_usage = merge_llm_usages(usage_steps)
    if usage_steps_out is not None:
        usage_steps_out.extend(usage_steps)
    yield "assistant", {
        "content": f"Stopped: reached the maximum of {_max_iterations()} agent iterations.",
        "usage": turn_usage.model_dump(),
    }


def _resolve_workspace(
    thread: CoderChatThread, requested: str | None
) -> str:
    root = (requested or "").strip() or thread.workspace_root or _default_workspace_root()
    if not root:
        raise HTTPException(
            status_code=400,
            detail=(
                "No workspace_root configured. Pass workspace_root in the request, "
                "set it on the thread, or set CODER_WORKSPACE_ROOT."
            ),
        )
    return root


def _sse(event: str, data: dict[str, Any]) -> str:
    return f"event: {event}\ndata: {json.dumps(data, ensure_ascii=False)}\n\n"


def _require_no_pending(thread: CoderChatThread) -> None:
    if thread.get_pending_call():
        raise HTTPException(
            status_code=409,
            detail=(
                "Thread has a command awaiting approval. "
                "Resolve it via /coder/chat/approve before sending a new message."
            ),
        )


async def send_message(
    session: Session,
    message: str,
    *,
    thread_id: int | None = None,
    model: str | None = None,
    provider: str | None = None,
    workspace_root: str | None = None,
    allow_commands: bool = False,
    auto_approve_commands: bool = False,
    max_tokens: int | None = None,
    client_id: str | None = None,
    delegate_tools: bool = False,
) -> dict[str, Any]:
    """Non-streaming agent run: executes the full turn, returns the persisted thread.

    If the model calls a tool in ``APPROVAL_REQUIRED_TOOLS`` (e.g. run_command)
    and ``auto_approve_commands`` is False, the run pauses and the response's
    messages will end with an unresolved assistant tool_calls entry; resolve
    it via ``resolve_pending_call`` before sending another message.
    """
    if not llm_proxy_master_key():
        raise HTTPException(status_code=503, detail="AGENT_PLATFORM_MASTER_KEY is not set.")
    thread = _resolve_thread(session, thread_id)
    title_task = None
    fallback_title = thread.title or "New session"
    if is_placeholder_title(thread.title, placeholders=frozenset({"New session"})):
        fallback_title = fallback_title_from_message(message, default="New session")
        thread.title = fallback_title
        title_task = start_smart_title_task(message, model=model)
    _require_no_pending(thread)
    root = _resolve_workspace(thread, workspace_root)
    try:
        executor = make_executor(
            root,
            thread_id=thread.id,
            client_id=client_id,
            allow_commands=allow_commands,
            delegate_tools=delegate_tools,
        )
    except ToolExecutionError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e

    history = thread.get_messages()
    llm_messages = _build_llm_messages(history, message)
    context_usage = _coder_context_usage(llm_messages)
    new_history: list[dict[str, Any]] = []
    pending_out: list[dict[str, Any]] = []
    usage_steps: list[LlmStepUsageOut] = []

    async for _event, _data in run_agent_turn(
        llm_messages,
        new_history,
        executor,
        model,
        provider=provider,
        max_tokens=max_tokens,
        auto_approve_commands=auto_approve_commands,
        pending_out=pending_out,
        usage_steps_out=usage_steps,
    ):
        pass

    turn_usage = merge_llm_usages(usage_steps) if usage_steps else LlmUsageOut()

    history.append({"role": "user", "content": message})
    history.extend(new_history)
    thread.set_messages(history)
    thread.set_pending_call(pending_out[0] if pending_out else None)
    if is_placeholder_title(thread.title, placeholders=frozenset({"New session"})):
        thread.title = fallback_title
    thread.workspace_root = str(executor.workspace_root)
    if model:
        thread.model = model
    thread.updated_at = utc_now_naive()
    session.add(thread)
    session.commit()

    final_title = await await_smart_title(
        session, thread, title_task, fallback=fallback_title
    )

    return {
        "thread_id": thread.id,
        "title": final_title,
        "workspace_root": thread.workspace_root,
        "context_window": context_usage.context_window,
        "messages": thread.get_messages(),
        "pending_call": thread.get_pending_call(),
        "context_usage": context_usage,
        "usage": turn_usage,
    }


async def stream_message(
    message: str,
    *,
    thread_id: int | None = None,
    model: str | None = None,
    provider: str | None = None,
    workspace_root: str | None = None,
    allow_commands: bool = False,
    auto_approve_commands: bool = False,
    max_tokens: int | None = None,
    client_id: str | None = None,
    delegate_tools: bool = False,
) -> AsyncIterator[str]:
    """Stream one agent turn as SSE: tool_call / tool_result / approval_required / assistant / done / error.

    The user message plus whatever the agent completed before a disconnect is
    committed in ``finally`` so a reload returns consistent history. If the
    turn pauses on ``approval_required``, the pending call is persisted on the
    thread; resolve it via ``resolve_pending_call`` before sending again.
    """
    key = llm_proxy_master_key()
    if not key:
        yield _sse("error", {"detail": "AGENT_PLATFORM_MASTER_KEY is not set."})
        return

    # Fresh session, resolved lazily so tests that monkeypatch `database.engine` win.
    with Session(database.engine) as session:
        thread = _resolve_thread(session, thread_id)
        title_task = None
        fallback_title = thread.title or "New session"
        if is_placeholder_title(thread.title, placeholders=frozenset({"New session"})):
            fallback_title = fallback_title_from_message(message, default="New session")
            thread.title = fallback_title
            title_task = start_smart_title_task(message, model=model)

        async def _stream_body() -> AsyncIterator[str]:
            try:
                _require_no_pending(thread)
                root = _resolve_workspace(thread, workspace_root)
                executor = make_executor(
                    root,
                    thread_id=thread.id,
                    client_id=client_id,
                    allow_commands=allow_commands,
                    delegate_tools=delegate_tools,
                )
            except HTTPException as e:
                yield _sse("error", {"detail": e.detail})
                return
            except ToolExecutionError as e:
                yield _sse("error", {"detail": str(e)})
                return

            history = thread.get_messages()
            llm_messages = _build_llm_messages(history, message)
            context_usage = _coder_context_usage(llm_messages)
            new_history: list[dict[str, Any]] = []
            pending_out: list[dict[str, Any]] = []
            usage_steps: list[LlmStepUsageOut] = []
            persisted = False

            def _persist() -> None:
                nonlocal persisted
                if persisted:
                    return
                persisted = True
                history.append({"role": "user", "content": message})
                history.extend(new_history)
                thread.set_messages(history)
                thread.set_pending_call(pending_out[0] if pending_out else None)
                if is_placeholder_title(
                    thread.title, placeholders=frozenset({"New session"})
                ):
                    thread.title = fallback_title
                thread.workspace_root = str(executor.workspace_root)
                if model:
                    thread.model = model
                thread.updated_at = utc_now_naive()
                session.add(thread)
                session.commit()

            def _done_payload() -> dict[str, Any]:
                turn_usage = merge_llm_usages(usage_steps) if usage_steps else LlmUsageOut()
                return {
                    "thread_id": thread.id,
                    "title": thread.title or "New session",
                    "workspace_root": thread.workspace_root,
                    "context_window": context_usage.context_window,
                    "messages": thread.get_messages(),
                    "pending_call": thread.get_pending_call(),
                    "context_usage": context_usage.model_dump(),
                    "usage": turn_usage.model_dump(),
                }

            try:
                async for event, data in run_agent_turn(
                    llm_messages,
                    new_history,
                    executor,
                    model,
                    provider=provider,
                    max_tokens=max_tokens,
                    auto_approve_commands=auto_approve_commands,
                    pending_out=pending_out,
                    usage_steps_out=usage_steps,
                ):
                    yield _sse(event, data)
                _persist()
                yield _sse("done", _done_payload())
            except HTTPException as e:
                _persist()
                yield _sse("error", {"detail": e.detail})
                yield _sse("done", _done_payload())
            except httpx.RequestError as e:
                _persist()
                yield _sse("error", {"detail": f"Upstream request failed: {e}"})
                yield _sse("done", _done_payload())
            finally:
                # Client disconnect throws GeneratorExit here; commit what we have.
                _persist()

        async for chunk in merge_title_sse_events(
            _stream_body(),
            title_task,
            thread_id=thread.id,
            fallback_title=fallback_title,
            session=session,
            thread=thread,
        ):
            yield chunk


async def stream_retry(
    *,
    thread_id: int,
    model: str | None = None,
    provider: str | None = None,
    workspace_root: str | None = None,
    allow_commands: bool = False,
    auto_approve_commands: bool = False,
    max_tokens: int | None = None,
    client_id: str | None = None,
    delegate_tools: bool = False,
) -> AsyncIterator[str]:
    """Re-run the agent turn after the last user message without appending a new one."""
    key = llm_proxy_master_key()
    if not key:
        yield _sse("error", {"detail": "AGENT_PLATFORM_MASTER_KEY is not set."})
        return

    with Session(database.engine) as session:
        thread = _get_thread_by_id(session, thread_id)
        fallback_title = thread.title or "New session"

        async def _stream_body() -> AsyncIterator[str]:
            try:
                truncated = _truncate_history_for_retry(thread.get_messages())
                thread.set_messages(truncated)
                thread.set_pending_call(None)
                thread.updated_at = utc_now_naive()
                session.add(thread)
                session.commit()
                session.refresh(thread)

                root = _resolve_workspace(thread, workspace_root)
                executor = make_executor(
                    root,
                    thread_id=thread.id,
                    client_id=client_id,
                    allow_commands=allow_commands,
                    delegate_tools=delegate_tools,
                )
            except HTTPException as e:
                yield _sse("error", {"detail": e.detail})
                return
            except ToolExecutionError as e:
                yield _sse("error", {"detail": str(e)})
                return

            llm_messages = _llm_messages_from_history(truncated)
            context_usage = _coder_context_usage(llm_messages)
            new_history: list[dict[str, Any]] = []
            pending_out: list[dict[str, Any]] = []
            usage_steps: list[LlmStepUsageOut] = []
            persisted = False

            def _persist() -> None:
                nonlocal persisted
                if persisted:
                    return
                persisted = True
                thread.set_messages(truncated + new_history)
                thread.set_pending_call(pending_out[0] if pending_out else None)
                thread.workspace_root = str(executor.workspace_root)
                if model:
                    thread.model = model
                thread.updated_at = utc_now_naive()
                session.add(thread)
                session.commit()

            def _done_payload() -> dict[str, Any]:
                turn_usage = merge_llm_usages(usage_steps) if usage_steps else LlmUsageOut()
                return {
                    "thread_id": thread.id,
                    "title": thread.title or "New session",
                    "workspace_root": thread.workspace_root,
                    "context_window": context_usage.context_window,
                    "messages": thread.get_messages(),
                    "pending_call": thread.get_pending_call(),
                    "context_usage": context_usage.model_dump(),
                    "usage": turn_usage.model_dump(),
                }

            try:
                async for event, data in run_agent_turn(
                    llm_messages,
                    new_history,
                    executor,
                    model,
                    provider=provider,
                    max_tokens=max_tokens,
                    auto_approve_commands=auto_approve_commands,
                    pending_out=pending_out,
                    usage_steps_out=usage_steps,
                ):
                    yield _sse(event, data)
                _persist()
                yield _sse("done", _done_payload())
            except HTTPException as e:
                _persist()
                yield _sse("error", {"detail": e.detail})
                yield _sse("done", _done_payload())
            except httpx.RequestError as e:
                _persist()
                yield _sse("error", {"detail": f"Upstream request failed: {e}"})
                yield _sse("done", _done_payload())
            finally:
                _persist()

        async for chunk in merge_title_sse_events(
            _stream_body(),
            None,
            thread_id=thread.id,
            fallback_title=fallback_title,
            session=session,
            thread=thread,
        ):
            yield chunk


def _is_command_override(original: str, edited: str) -> bool:
    """True when ``edited`` is an intentional full command replacement.

    Shorthand tokens (e.g. remember-rule ``powershell`` for a longer command)
    must not replace the model's full command string.
    """
    if not edited or edited == original:
        return False
    if original.startswith(edited + " "):
        return False
    first = original.split(None, 1)[0] if original else ""
    if edited == first:
        return False
    return True


async def resolve_pending_call(
    *,
    thread_id: int,
    call_id: str,
    approve: bool,
    edited_command: str | None = None,
    model: str | None = None,
    provider: str | None = None,
    allow_commands: bool = True,
    auto_approve_commands: bool = False,
    max_tokens: int | None = None,
    client_id: str | None = None,
    delegate_tools: bool = False,
) -> AsyncIterator[str]:
    """Resolve a paused run_command approval and resume the agent turn as SSE.

    Executes (or rejects) the pending call, then continues processing any
    remaining calls from the same batch, then keeps looping the agent turn
    exactly like ``stream_message`` — including pausing again if another
    command needs approval.
    """
    key = llm_proxy_master_key()
    if not key:
        yield _sse("error", {"detail": "AGENT_PLATFORM_MASTER_KEY is not set."})
        return

    with Session(database.engine) as session:
        thread = _get_thread_by_id(session, thread_id)
        pending = thread.get_pending_call()
        if not pending:
            yield _sse("error", {"detail": "No pending call on this thread."})
            return
        if pending.get("call_id") != call_id:
            yield _sse(
                "error",
                {"detail": f"call_id mismatch: pending is {pending.get('call_id')!r}"},
            )
            return

        try:
            root = _resolve_workspace(thread, None)
            executor = make_executor(
                root,
                thread_id=thread.id,
                client_id=client_id,
                allow_commands=allow_commands,
                delegate_tools=delegate_tools,
            )
        except HTTPException as e:
            yield _sse("error", {"detail": e.detail})
            return
        except ToolExecutionError as e:
            yield _sse("error", {"detail": str(e)})
            return

        history = thread.get_messages()
        llm_messages = _llm_messages_from_history(history)
        context_usage = _coder_context_usage(llm_messages)
        new_history: list[dict[str, Any]] = []
        usage_steps: list[LlmStepUsageOut] = []

        args = dict(pending.get("arguments") or {})
        if pending.get("name") == "run_command" and edited_command is not None:
            original = str(args.get("command") or "").strip()
            edited = edited_command.strip()
            # Desktop "Accept & remember" used to send the rule pattern (e.g.
            # "powershell", "dir") as edited_command. That must not replace the
            # full command the model requested.
            if edited and edited != original and _is_command_override(original, edited):
                args["command"] = edited

        if approve:
            yield _sse(
                "tool_call",
                {
                    "call_id": pending["call_id"],
                    "name": pending["name"],
                    "arguments": args,
                },
            )
            result = await executor.execute(
                pending["name"], args, call_id=pending["call_id"]
            )
        else:
            result = "Error: command rejected by the user."
        result = truncate_text_to_tokens(result, tool_result_soft_cap_tokens())
        tool_msg = {
            "role": "tool",
            "tool_call_id": pending["call_id"],
            "name": pending["name"],
            "content": result,
        }
        llm_messages.append(tool_msg)
        new_history.append(tool_msg)
        yield _sse("tool_result", {"name": pending["name"], "content": result})

        remaining_calls = _parse_tool_calls_raw(pending.get("remaining") or [])
        pending_out: list[dict[str, Any]] = []
        persisted = False

        def _persist() -> None:
            nonlocal persisted
            if persisted:
                return
            persisted = True
            history.extend(new_history)
            thread.set_messages(history)
            thread.set_pending_call(pending_out[0] if pending_out else None)
            thread.updated_at = utc_now_naive()
            session.add(thread)
            session.commit()

        def _done_payload() -> dict[str, Any]:
            turn_usage = merge_llm_usages(usage_steps) if usage_steps else LlmUsageOut()
            return {
                "thread_id": thread.id,
                "title": thread.title or "New session",
                "workspace_root": thread.workspace_root,
                "context_window": context_usage.context_window,
                "messages": thread.get_messages(),
                "pending_call": thread.get_pending_call(),
                "context_usage": context_usage.model_dump(),
                "usage": turn_usage.model_dump(),
            }

        try:
            async for event, data in run_agent_turn(
                llm_messages,
                new_history,
                executor,
                model or thread.model,
                provider=provider,
                max_tokens=max_tokens,
                auto_approve_commands=auto_approve_commands,
                pending_out=pending_out,
                resume_calls=remaining_calls,
                usage_steps_out=usage_steps,
            ):
                yield _sse(event, data)
            _persist()
            yield _sse("done", _done_payload())
        except HTTPException as e:
            _persist()
            yield _sse("error", {"detail": e.detail})
            yield _sse("done", _done_payload())
        except httpx.RequestError as e:
            _persist()
            yield _sse("error", {"detail": f"Upstream request failed: {e}"})
            yield _sse("done", _done_payload())
        finally:
            _persist()
