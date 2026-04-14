from __future__ import annotations

from models import Process, TaskNode


def task_status_counts(tasks: list[TaskNode]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for task in tasks:
        counts[task.status] = counts.get(task.status, 0) + 1
    return counts


def sync_terminal_detail(status: str) -> str:
    if status == "failed":
        return (
            "Process failed; use POST /retry to re-plan or re-run execution, "
            "or retry individual failed tasks — sync does not apply."
        )
    return "Process is already finished; sync does nothing."


def sync_review_gate_detail(awaiting_review_count: int) -> str:
    return (
        "Waiting for human task review "
        f"({awaiting_review_count} task(s) in awaiting_review). "
        "Use the review actions on each task."
    )


def align_running_process_to_review_required(process: Process) -> None:
    process.status = "task_review_required"
    process.failure_reason = None


def reset_running_tasks_to_pending(tasks: list[TaskNode]) -> int:
    reset_n = 0
    for task in tasks:
        if task.status != "running":
            continue
        task.status = "pending"
        task.output = None
        task.draft_output = None
        task.review_feedback = None
        task.reviewer_client_uuid = None
        task.failure_debug_json = None
        task.started_at = None
        task.completed_at = None
        task.tokens_used = 0
        reset_n += 1
    return reset_n
