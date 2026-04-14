from __future__ import annotations

from models import Process
from sqlmodel import Session


def set_process_running_or_review_required(
    session: Session,
    *,
    process_id: int,
    awaiting_review: bool,
) -> Process:
    run = session.get(Process, process_id)
    if not run:
        raise ValueError("process not found")
    run.status = "task_review_required" if awaiting_review else "running"
    run.failure_reason = None
    session.commit()
    return run


def pause_process_for_task_review(session: Session, *, process_id: int) -> Process:
    run = session.get(Process, process_id)
    if not run:
        raise ValueError("process not found")
    run.status = "task_review_required"
    run.failure_reason = None
    session.commit()
    return run


def complete_process(session: Session, *, process_id: int) -> Process:
    run = session.get(Process, process_id)
    if not run:
        raise ValueError("process not found")
    run.status = "completed"
    run.failure_reason = None
    session.commit()
    return run


def fail_process(session: Session, *, process_id: int, reason: str) -> Process:
    run = session.get(Process, process_id)
    if not run:
        raise ValueError("process not found")
    run.status = "failed"
    run.failure_reason = reason
    session.commit()
    return run
