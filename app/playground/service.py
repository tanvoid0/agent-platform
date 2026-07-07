"""Standalone chat Playground service: thread CRUD + one LLM turn per send, no board/orchestrator."""

from __future__ import annotations

import asyncio
import json
import os
from typing import Any, AsyncIterator

import httpx
from fastapi import HTTPException
from sqlmodel import Session, select

import database

from chat_usage import (
    ContextUsageOut,
    estimate_context_usage,
    merge_llm_usages,
    parse_llm_usage_dict,
)
from context_budget import (
    fit_chat_messages_for_request,
    max_output_tokens_default,
)
from dag_schema import sanitize_llm_model_alias
from llm_proxy_env import (
    llm_proxy_base_url_v1,
    llm_proxy_http_timeout_seconds,
    llm_proxy_master_key,
)
from playground.models import PlaygroundChatThread
from time_utils import utc_now_naive

PLAYGROUND_SYSTEM_PROMPT = "You are a helpful assistant. Answer clearly and concisely."


def _auto_title_from_message(message: str) -> str:
    text = " ".join(message.split())
    if not text:
        return "New chat"
    if len(text) <= 48:
        return text
    return text[:45] + "..."


def _create_thread_row(
    session: Session, *, title: str | None = "New chat"
) -> PlaygroundChatThread:
    now = utc_now_naive()
    row = PlaygroundChatThread(title=title, created_at=now, updated_at=now)
    row.set_messages([])
    session.add(row)
    session.commit()
    session.refresh(row)
    return row


def _get_thread_by_id(session: Session, thread_id: int) -> PlaygroundChatThread:
    row = session.get(PlaygroundChatThread, thread_id)
    if not row:
        raise HTTPException(status_code=404, detail="Playground thread not found")
    return row


def _resolve_thread(
    session: Session, thread_id: int | None = None
) -> PlaygroundChatThread:
    if thread_id is not None:
        return _get_thread_by_id(session, thread_id)
    row = session.exec(
        select(PlaygroundChatThread).order_by(PlaygroundChatThread.updated_at.desc())
    ).first()
    if row:
        return row
    return _create_thread_row(session)


def create_thread(session: Session, *, title: str | None = None) -> PlaygroundChatThread:
    return _create_thread_row(session, title=title or "New chat")


def list_threads(session: Session) -> list[dict[str, Any]]:
    rows = session.exec(
        select(PlaygroundChatThread).order_by(PlaygroundChatThread.updated_at.desc())
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
                "title": row.title or "New chat",
                "message_count": len(messages),
                "preview": preview,
                "created_at": row.created_at.isoformat(),
                "updated_at": row.updated_at.isoformat(),
            }
        )
    return out


def _playground_context_usage(llm_messages: list[dict[str, str]]) -> ContextUsageOut:
    conversation = [m for m in llm_messages if m.get("role") != "system"]
    return estimate_context_usage(
        system_prompt=PLAYGROUND_SYSTEM_PROMPT,
        conversation_messages=conversation,
    )


async def get_thread(session: Session, thread_id: int | None = None) -> dict[str, Any]:
    thread = _resolve_thread(session, thread_id)
    llm_messages = _build_llm_messages(thread.get_messages(), "")
    # _build_llm_messages appends empty user msg; drop it for context estimate.
    if llm_messages and llm_messages[-1].get("content") == "":
        llm_messages = llm_messages[:-1]
    ctx = _playground_context_usage(llm_messages)
    return {
        "thread_id": thread.id,
        "title": thread.title or "New chat",
        "model": thread.model,
        "messages": thread.get_messages(),
        "context_window": ctx.context_window,
        "context_usage": ctx,
    }


def get_context_usage(session: Session, thread_id: int | None = None) -> ContextUsageOut:
    thread = _resolve_thread(session, thread_id)
    history = thread.get_messages()
    llm_messages: list[dict[str, str]] = [{"role": "system", "content": PLAYGROUND_SYSTEM_PROMPT}]
    for h in history:
        if h.get("content"):
            llm_messages.append({"role": h.get("role", "user"), "content": h["content"]})
    return _playground_context_usage(llm_messages)


def delete_thread(session: Session, thread_id: int) -> None:
    thread = _get_thread_by_id(session, thread_id)
    session.delete(thread)
    session.commit()


def _build_payload(
    messages: list[dict[str, str]],
    model: str | None,
    *,
    temperature: float | None = None,
    top_p: float | None = None,
    max_tokens: int | None = None,
) -> dict[str, Any]:
    fitted, _ = fit_chat_messages_for_request(messages)
    payload: dict[str, Any] = {
        "messages": fitted,
        "max_tokens": max_tokens if max_tokens is not None else max_output_tokens_default(),
    }
    sm = sanitize_llm_model_alias(model) if model else None
    if sm:
        payload["model"] = sm
    if temperature is not None:
        payload["temperature"] = temperature
    if top_p is not None:
        payload["top_p"] = top_p
    return payload


def _build_llm_messages(history: list[dict[str, Any]], message: str) -> list[dict[str, str]]:
    llm_messages = [{"role": "system", "content": PLAYGROUND_SYSTEM_PROMPT}]
    for h in history:
        if h.get("content"):
            llm_messages.append({"role": h.get("role", "user"), "content": h["content"]})
    llm_messages.append({"role": "user", "content": message})
    return llm_messages


def _connect_max_attempts() -> int:
    raw = (os.environ.get("LLM_CLIENT_CONNECT_MAX_ATTEMPTS") or "3").strip()
    try:
        return max(1, int(raw))
    except ValueError:
        return 3


async def _post_chat_completions_with_retry(
    payload: dict[str, Any], headers: dict[str, str], base: str
) -> httpx.Response:
    """POST with retry on connect-phase failures (request never sent, safe to retry)."""
    max_attempts = _connect_max_attempts()
    for attempt in range(max_attempts):
        try:
            async with httpx.AsyncClient(timeout=llm_proxy_http_timeout_seconds()) as client:
                return await client.post(f"{base}/chat/completions", headers=headers, json=payload)
        except (httpx.ConnectError, httpx.ConnectTimeout) as e:
            if attempt < max_attempts - 1:
                await asyncio.sleep(0.2 * (2**attempt))
                continue
            raise HTTPException(status_code=502, detail=f"Upstream request failed: {e}") from e
        except httpx.RequestError as e:
            raise HTTPException(status_code=502, detail=f"Upstream request failed: {e}") from e
    raise HTTPException(status_code=502, detail="Upstream request failed: exhausted retries")


async def _call_llm(
    messages: list[dict[str, str]],
    model: str | None,
    *,
    temperature: float | None = None,
    top_p: float | None = None,
    max_tokens: int | None = None,
) -> tuple[str, dict[str, Any] | None]:
    key = llm_proxy_master_key()
    if not key:
        raise HTTPException(status_code=503, detail="AGENT_PLATFORM_MASTER_KEY is not set.")

    payload = _build_payload(
        messages, model, temperature=temperature, top_p=top_p, max_tokens=max_tokens
    )
    base = llm_proxy_base_url_v1()
    headers = {"Content-Type": "application/json", "Authorization": f"Bearer {key}"}
    r = await _post_chat_completions_with_retry(payload, headers, base)

    if r.status_code != 200:
        raise HTTPException(status_code=502, detail=f"LLM proxy returned HTTP {r.status_code}")

    data = r.json()
    usage = data.get("usage") if isinstance(data.get("usage"), dict) else None
    choices = data.get("choices") or []
    if choices:
        content = (choices[0].get("message") or {}).get("content") or ""
        return content, usage
    return "", usage


async def send_message(
    session: Session,
    message: str,
    *,
    thread_id: int | None = None,
    model: str | None = None,
    temperature: float | None = None,
    top_p: float | None = None,
    max_tokens: int | None = None,
) -> dict[str, Any]:
    thread = _resolve_thread(session, thread_id)
    history = thread.get_messages()

    llm_messages = _build_llm_messages(history, message)
    context_usage = _playground_context_usage(llm_messages)

    # Persist the user turn before the LLM call, mirroring stream_message: an
    # upstream failure must not lose the message the user already sent.
    history.append({"role": "user", "content": message})
    thread.set_messages(history)
    if not thread.title or thread.title == "New chat":
        thread.title = _auto_title_from_message(message)
    if model:
        thread.model = model
    thread.updated_at = utc_now_naive()
    session.add(thread)
    session.commit()

    reply, usage = await _call_llm(
        llm_messages, model, temperature=temperature, top_p=top_p, max_tokens=max_tokens
    )

    turn_usage = merge_llm_usages([parse_llm_usage_dict(usage, label="chat")])
    assistant_message: dict[str, Any] = {"role": "assistant", "content": reply}
    if usage is not None:
        assistant_message["usage"] = usage
    history.append(assistant_message)
    thread.set_messages(history)
    thread.updated_at = utc_now_naive()
    session.add(thread)
    session.commit()

    return {
        "thread_id": thread.id,
        "title": thread.title or "New chat",
        "context_window": context_usage.context_window,
        "messages": thread.get_messages(),
        "context_usage": context_usage,
        "usage": turn_usage,
    }


def _sse(event: str, data: dict[str, Any]) -> str:
    return f"event: {event}\ndata: {json.dumps(data, ensure_ascii=False)}\n\n"


async def stream_message(
    message: str,
    *,
    thread_id: int | None = None,
    model: str | None = None,
    temperature: float | None = None,
    top_p: float | None = None,
    max_tokens: int | None = None,
) -> AsyncIterator[str]:
    """Stream one assistant turn as SSE, persisting the (possibly partial) reply.

    Emits ``delta`` events per token, a terminal ``done`` event with the full
    persisted thread, or an ``error`` event. The user message plus whatever text
    streamed before a disconnect is always committed in ``finally`` so a reload
    (GET /chat/thread) returns the same data even if the connection dropped.
    """
    key = llm_proxy_master_key()
    if not key:
        yield _sse("error", {"detail": "AGENT_PLATFORM_MASTER_KEY is not set."})
        return

    # Fresh session, resolved lazily so tests that monkeypatch `database.engine` win.
    with Session(database.engine) as session:
        thread = _resolve_thread(session, thread_id)
        history = thread.get_messages()
        llm_messages = _build_llm_messages(history, message)
        context_usage = _playground_context_usage(llm_messages)

        reply_parts: list[str] = []
        usage: dict[str, Any] | None = None
        persisted = False

        def _persist() -> None:
            nonlocal persisted
            if persisted:
                return
            persisted = True
            history.append({"role": "user", "content": message})
            text = "".join(reply_parts)
            if text:
                assistant_message: dict[str, Any] = {"role": "assistant", "content": text}
                if usage is not None:
                    assistant_message["usage"] = usage
                history.append(assistant_message)
            thread.set_messages(history)
            if not thread.title or thread.title == "New chat":
                thread.title = _auto_title_from_message(message)
            if model:
                thread.model = model
            thread.updated_at = utc_now_naive()
            session.add(thread)
            session.commit()

        def _done_payload() -> dict[str, Any]:
            turn_usage = merge_llm_usages([parse_llm_usage_dict(usage, label="chat")])
            return {
                "thread_id": thread.id,
                "title": thread.title or "New chat",
                "context_window": context_usage.context_window,
                "messages": thread.get_messages(),
                "context_usage": context_usage.model_dump(),
                "usage": turn_usage.model_dump(),
            }

        try:
            payload = _build_payload(
                llm_messages,
                model,
                temperature=temperature,
                top_p=top_p,
                max_tokens=max_tokens,
            )
            payload["stream"] = True
            payload["stream_options"] = {"include_usage": True}

            base = llm_proxy_base_url_v1()
            headers = {"Content-Type": "application/json", "Authorization": f"Bearer {key}"}
            # Disable the per-read timeout so idle gaps between tokens don't abort a live stream.
            timeout = httpx.Timeout(llm_proxy_http_timeout_seconds(), read=None)
            async with httpx.AsyncClient(timeout=timeout) as http:
                async with http.stream(
                    "POST", f"{base}/chat/completions", headers=headers, json=payload
                ) as r:
                    if r.status_code != 200:
                        await r.aread()
                        _persist()
                        yield _sse("error", {"detail": f"LLM proxy returned HTTP {r.status_code}"})
                        yield _sse("done", _done_payload())
                        return
                    async for line in r.aiter_lines():
                        if not line or not line.startswith("data:"):
                            continue
                        data = line[len("data:") :].strip()
                        if data == "[DONE]":
                            break
                        try:
                            obj = json.loads(data)
                        except json.JSONDecodeError:
                            continue
                        choices = obj.get("choices") or []
                        if choices:
                            delta = (choices[0].get("delta") or {}).get("content")
                            if delta:
                                reply_parts.append(delta)
                                yield _sse("delta", {"content": delta})
                        u = obj.get("usage")
                        if isinstance(u, dict):
                            usage = u
            _persist()
            yield _sse("done", _done_payload())
        except httpx.RequestError as e:
            _persist()
            yield _sse("error", {"detail": f"Upstream request failed: {e}"})
            yield _sse("done", _done_payload())
        finally:
            # Client disconnect throws GeneratorExit here; commit what we have.
            _persist()
