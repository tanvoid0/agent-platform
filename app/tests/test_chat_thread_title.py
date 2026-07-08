"""Unit tests for parallel smart chat thread titles."""

import asyncio
import json
from collections.abc import AsyncIterator
from unittest.mock import AsyncMock, patch

import pytest
from sqlmodel import Session

from chat_thread_title import (
    await_smart_title,
    chat_smart_titles_enabled,
    fallback_title_from_message,
    format_sse_event,
    generate_smart_title,
    is_placeholder_title,
    merge_title_sse_events,
    persist_thread_title,
    start_smart_title_task,
)
from playground.models import PlaygroundChatThread


def _run(coro):
    return asyncio.run(coro)


def test_fallback_title_from_message_short_and_long():
    assert fallback_title_from_message("  Say hi  ") == "Say hi"
    long_msg = "x" * 60
    assert fallback_title_from_message(long_msg) == ("x" * 45) + "..."
    assert fallback_title_from_message("", default="New session") == "New session"


def test_is_placeholder_title():
    assert is_placeholder_title(None)
    assert is_placeholder_title("")
    assert is_placeholder_title("New chat")
    assert is_placeholder_title("New session")
    assert not is_placeholder_title("Meal planning")


@pytest.mark.parametrize(
    ("value", "expected"),
    [
        ("1", True),
        ("0", False),
        ("false", False),
        ("off", False),
        (None, True),
    ],
)
def test_chat_smart_titles_enabled(monkeypatch, value, expected):
    monkeypatch.delenv("CHAT_SMART_TITLES", raising=False)
    if value is not None:
        monkeypatch.setenv("CHAT_SMART_TITLES", value)
    assert chat_smart_titles_enabled() is expected


def test_generate_smart_title_calls_llm(monkeypatch):
    monkeypatch.setenv("CHAT_SMART_TITLES", "1")

    async def _test():
        with patch(
            "chat_thread_title.call_llm",
            new_callable=AsyncMock,
            return_value=("Weekly workout plan", 12, 0.0),
        ) as mock_llm:
            title = await generate_smart_title("Help me plan workouts this week")
        assert title == "Weekly workout plan"
        mock_llm.assert_awaited_once()

    _run(_test())


def test_generate_smart_title_disabled_returns_none(monkeypatch):
    monkeypatch.setenv("CHAT_SMART_TITLES", "0")

    async def _test():
        with patch("chat_thread_title.call_llm", new_callable=AsyncMock) as mock_llm:
            assert await generate_smart_title("Hello") is None
        mock_llm.assert_not_called()

    _run(_test())


def test_start_smart_title_task_respects_env(monkeypatch):
    monkeypatch.setenv("CHAT_SMART_TITLES", "0")
    assert start_smart_title_task("Hello") is None

    monkeypatch.setenv("CHAT_SMART_TITLES", "1")

    async def _test():
        with patch(
            "chat_thread_title.generate_smart_title",
            new_callable=AsyncMock,
            return_value="Hello",
        ):
            task = start_smart_title_task("Hello")
        assert task is not None
        task.cancel()

    _run(_test())


def test_persist_thread_title(test_engine):
    with Session(test_engine) as session:
        row = PlaygroundChatThread(title="New chat")
        row.set_messages([])
        session.add(row)
        session.commit()
        session.refresh(row)
        persist_thread_title(session, row, "Smart title")
        assert row.title == "Smart title"


def test_await_smart_title_uses_smart_result(test_engine, monkeypatch):
    monkeypatch.setenv("CHAT_SMART_TITLES", "1")

    async def _test():
        with Session(test_engine) as session:
            row = PlaygroundChatThread(title="Fallback")
            row.set_messages([])
            session.add(row)
            session.commit()
            session.refresh(row)

            async def _smart() -> str | None:
                return "LLM title"

            task = asyncio.create_task(_smart())
            final = await await_smart_title(session, row, task, fallback="Fallback")
            assert final == "LLM title"
            assert row.title == "LLM title"

    _run(_test())


def test_await_smart_title_keeps_fallback_when_task_none(test_engine):
    async def _test():
        with Session(test_engine) as session:
            row = PlaygroundChatThread(title="New chat")
            row.set_messages([])
            session.add(row)
            session.commit()
            session.refresh(row)
            final = await await_smart_title(session, row, None, fallback="Say hi")
            assert final == "Say hi"
            assert row.title == "Say hi"

    _run(_test())


def _parse_sse(text: str) -> list[tuple[str, dict]]:
    events: list[tuple[str, dict]] = []
    for block in text.strip().split("\n\n"):
        event = ""
        data = ""
        for line in block.splitlines():
            if line.startswith("event:"):
                event = line[len("event:") :].strip()
            elif line.startswith("data:"):
                data = line[len("data:") :].strip()
        if event:
            events.append((event, json.loads(data) if data else {}))
    return events


def test_merge_title_sse_events_emits_title(test_engine, monkeypatch):
    monkeypatch.setenv("CHAT_SMART_TITLES", "1")

    async def _source() -> AsyncIterator[str]:
        yield format_sse_event("delta", {"content": "Hi"})
        await asyncio.sleep(0.05)
        yield format_sse_event("done", {"title": "fallback"})

    async def _test():
        async def _smart() -> str | None:
            await asyncio.sleep(0.01)
            return "Smart greeting"

        with Session(test_engine) as session:
            row = PlaygroundChatThread(title="New chat")
            row.set_messages([])
            session.add(row)
            session.commit()
            session.refresh(row)
            task = asyncio.create_task(_smart())
            chunks: list[str] = []
            async for chunk in merge_title_sse_events(
                _source(),
                task,
                thread_id=row.id,
                fallback_title="Say hi",
                session=session,
                thread=row,
            ):
                chunks.append(chunk)

        events = _parse_sse("".join(chunks))
        kinds = [e[0] for e in events]
        assert "title" in kinds
        title_event = next(e for e in events if e[0] == "title")
        assert title_event[1]["title"] == "Smart greeting"
        assert kinds.index("title") < kinds.index("done")

    _run(_test())
