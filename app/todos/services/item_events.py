"""Append-only event log for todo items."""

from __future__ import annotations

from typing import Any

from sqlmodel import Session, select

from time_utils import utc_now_naive
from todos.models import TodoItemEvent


def append_item_event(
    session: Session,
    item_id: int,
    event_type: str,
    content: dict[str, Any],
) -> TodoItemEvent:
    row = TodoItemEvent(
        item_id=item_id,
        event_type=event_type,
        created_at=utc_now_naive(),
    )
    row.set_content(content)
    session.add(row)
    session.commit()
    session.refresh(row)
    return row


def list_item_events(
    session: Session,
    item_id: int,
    *,
    after_id: int = 0,
    limit: int = 200,
) -> list[TodoItemEvent]:
    lim = min(max(limit, 1), 2000)
    return list(
        session.exec(
            select(TodoItemEvent)
            .where(TodoItemEvent.item_id == item_id)
            .where(TodoItemEvent.id > max(after_id, 0))
            .order_by(TodoItemEvent.id.asc())
            .limit(lim)
        ).all()
    )
