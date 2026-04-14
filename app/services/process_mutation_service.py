from __future__ import annotations

import json
from datetime import datetime, timezone

from models import EventLog, Process, TaskNode
from sqlmodel import Session


def append_process_event(
    session: Session,
    *,
    process_id: int,
    event_type: str,
    content: str,
    task_id: int | None = None,
) -> None:
    session.add(
        EventLog(
            process_id=process_id,
            task_id=task_id,
            event_type=event_type,
            content=content,
        )
    )


def requeue_task_for_changes(
    *,
    task: TaskNode,
    process: Process,
    feedback: str,
    instructions: str | None,
) -> None:
    task.draft_output = task.output
    task.output = None
    task.review_feedback = feedback
    task.reviewer_client_uuid = None
    task.revision_count += 1
    if instructions is not None:
        task.instructions = instructions
    task.status = "pending"
    task.failure_debug_json = None
    task.started_at = None
    task.completed_at = None
    task.tokens_used = 0
    process.status = "running"
    process.failure_reason = None


def reject_task_and_fail_process(*, task: TaskNode, process: Process) -> None:
    task.reviewer_client_uuid = None
    task.status = "failed"
    task.failure_debug_json = json.dumps(
        {
            "source": "review_reject",
            "message": "Human reviewer rejected this task at the review gate.",
        },
        ensure_ascii=False,
    )
    process.status = "failed"
    process.failure_reason = f"Task {task.client_uuid} rejected at review"


def approve_task_output(*, task: TaskNode, process: Process, output: str | None) -> None:
    if output is not None:
        task.output = output
    task.reviewer_client_uuid = None
    task.status = "completed"
    task.completed_at = datetime.now(timezone.utc).replace(tzinfo=None)
    task.draft_output = None
    task.review_feedback = None
    process.status = "running"
    process.failure_reason = None


def reset_failed_task_for_retry(*, task: TaskNode, process: Process) -> None:
    task.status = "pending"
    task.output = None
    task.draft_output = None
    task.review_feedback = None
    task.reviewer_client_uuid = None
    task.failure_debug_json = None
    task.revision_count = 0
    task.started_at = None
    task.completed_at = None
    task.tokens_used = 0
    process.failure_reason = None
    process.status = "approved"
