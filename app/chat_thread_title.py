"""Parallel smart title generation for chat threads (Assistant, Coder, Playground)."""

from __future__ import annotations

import asyncio
import json
import os
from typing import Any, AsyncIterator

from sqlmodel import Session

from llm_client import call_llm
from time_utils import utc_now_naive

DEFAULT_PLACEHOLDERS = frozenset({"New chat", "New session"})

_TITLE_SYSTEM = (
    "You generate short conversation titles. Reply with only the title text: "
    "max 6 words, no quotes, no trailing punctuation."
)


def chat_smart_titles_enabled() -> bool:
    raw = (os.getenv("CHAT_SMART_TITLES") or "1").strip().lower()
    return raw not in ("0", "false", "no", "off")


def fallback_title_from_message(message: str, *, default: str = "New chat") -> str:
    text = " ".join(message.split())
    if not text:
        return default
    if len(text) <= 48:
        return text
    return text[:45] + "..."


def is_placeholder_title(
    title: str | None,
    *,
    placeholders: frozenset[str] | set[str] | None = None,
) -> bool:
    if not title or not str(title).strip():
        return True
    ph = placeholders if placeholders is not None else DEFAULT_PLACEHOLDERS
    return str(title).strip() in ph


def _clean_smart_title(raw: str) -> str:
    t = raw.strip().strip("\"'`").strip()
    if not t:
        return ""
    t = t.splitlines()[0].strip()
    t = t.rstrip(".!?")
    if len(t) > 128:
        t = t[:125] + "..."
    return t


async def generate_smart_title(message: str, *, model: str | None = None) -> str | None:
    if not chat_smart_titles_enabled():
        return None
    text = " ".join(message.split())
    if not text:
        return None
    try:
        content, _, _ = await call_llm(
            [
                {"role": "system", "content": _TITLE_SYSTEM},
                {"role": "user", "content": text[:800]},
            ],
            model=model,
            temperature=0.2,
            max_output_tokens=24,
        )
    except Exception:
        return None
    cleaned = _clean_smart_title(content)
    return cleaned or None


def persist_thread_title(session: Session, thread: Any, title: str) -> None:
    thread.title = title
    thread.updated_at = utc_now_naive()
    session.add(thread)
    session.commit()
    session.refresh(thread)


def start_smart_title_task(
    message: str,
    *,
    model: str | None = None,
) -> asyncio.Task[str | None] | None:
    if not chat_smart_titles_enabled():
        return None
    return asyncio.create_task(generate_smart_title(message, model=model))


async def await_smart_title(
    session: Session,
    thread: Any,
    title_task: asyncio.Task[str | None] | None,
    *,
    fallback: str,
) -> str:
    final = fallback
    if title_task is not None:
        try:
            smart = await title_task
            if smart:
                final = smart
        except Exception:
            pass
    if (thread.title or "") != final:
        persist_thread_title(session, thread, final)
    return final


def format_sse_event(event: str, data: dict[str, Any]) -> str:
    return f"event: {event}\ndata: {json.dumps(data, ensure_ascii=False)}\n\n"


async def _resolve_title_from_task(
    title_task: asyncio.Task[str | None],
    *,
    fallback: str,
) -> str:
    try:
        smart = await title_task
        if smart:
            return smart
    except Exception:
        pass
    return fallback


async def merge_title_sse_events(
    source: AsyncIterator[str],
    title_task: asyncio.Task[str | None] | None,
    *,
    thread_id: int,
    fallback_title: str,
    session: Session | None = None,
    thread: Any | None = None,
) -> AsyncIterator[str]:
    """Interleave a ``title`` SSE event when the parallel smart-title task completes."""
    if title_task is None:
        async for chunk in source:
            yield chunk
        return

    queue: asyncio.Queue[tuple[str, str] | None] = asyncio.Queue()

    async def _source_worker() -> None:
        try:
            async for chunk in source:
                await queue.put(("source", chunk))
        finally:
            await queue.put(None)

    async def _title_worker() -> None:
        final = await _resolve_title_from_task(title_task, fallback=fallback_title)
        if session is not None and thread is not None and (thread.title or "") != final:
            persist_thread_title(session, thread, final)
        await queue.put(
            (
                "title",
                format_sse_event("title", {"thread_id": thread_id, "title": final}),
            )
        )

    src_worker = asyncio.create_task(_source_worker())
    title_worker = asyncio.create_task(_title_worker())
    title_yielded = False
    source_closed = False

    try:
        while not (source_closed and title_yielded):
            item = await queue.get()
            if item is None:
                source_closed = True
                continue
            kind, payload = item
            if kind == "title":
                title_yielded = True
            yield payload
        while not title_yielded:
            item = await queue.get()
            if item is None:
                break
            kind, payload = item
            if kind == "title":
                title_yielded = True
                yield payload
    finally:
        if not src_worker.done():
            src_worker.cancel()
        if not title_worker.done():
            title_worker.cancel()
