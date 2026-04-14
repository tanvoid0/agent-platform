from __future__ import annotations

from models import EventLog
from sqlmodel import Session


def append_event(
    session: Session,
    *,
    process_id: int,
    event_type: str,
    content: str,
    task_id: int | None = None,
) -> EventLog:
    log = EventLog(
        process_id=process_id,
        task_id=task_id,
        event_type=event_type,
        content=content,
    )
    session.add(log)
    session.commit()
    return log
