from __future__ import annotations

from models import Process, TaskNode
from services.review_assignment_service import sync_review_assignments
from sqlmodel import Session
from time_utils import utc_now_naive


def apply_task_success(
    session: Session,
    *,
    process_id: int,
    task_id: int,
    output: str,
    tokens: int,
    task_cost: float,
    tool_calls: int,
) -> tuple[TaskNode, Process, bool]:
    task = session.get(TaskNode, task_id)
    run = session.get(Process, process_id)
    if not task or not run:
        raise ValueError("task or process not found")

    task.output = output
    task.tokens_used = tokens
    run.total_tokens += tokens
    run.total_cost += task_cost
    run.tool_invocations_used = int(run.tool_invocations_used or 0) + int(tool_calls)

    needs_expand = False
    if task.requires_review:
        task.status = "awaiting_review"
        task.completed_at = None
        run.status = "task_review_required"
    else:
        task.status = "completed"
        task.completed_at = utc_now_naive()
        needs_expand = True

    sync_review_assignments(session, process_id)
    session.commit()
    return task, run, needs_expand


def apply_task_failure(
    session: Session,
    *,
    process_id: int,
    task_id: int,
    failure_debug_json: str,
    failure_reason: str,
) -> tuple[TaskNode, Process]:
    task = session.get(TaskNode, task_id)
    run = session.get(Process, process_id)
    if not task or not run:
        raise ValueError("task or process not found")

    task.status = "failed"
    task.failure_debug_json = failure_debug_json
    run.status = "failed"
    run.failure_reason = failure_reason
    session.commit()
    return task, run
