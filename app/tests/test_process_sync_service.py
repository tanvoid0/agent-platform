from __future__ import annotations

from datetime import datetime, timezone

from models import Process, TaskNode
from services.process_sync_service import (
    align_running_process_to_review_required,
    reset_running_tasks_to_pending,
    sync_review_gate_detail,
    sync_terminal_detail,
    task_status_counts,
)


def _task(status: str) -> TaskNode:
    task = TaskNode(
        process_id=1,
        client_uuid=f"t-{status}",
        role="R",
        system_prompt="S",
        instructions="I",
        status=status,
    )
    task.dependencies = []
    task.output = "x"
    task.tokens_used = 5
    ts = datetime.now(timezone.utc).replace(tzinfo=None)
    task.started_at = ts
    task.completed_at = ts
    return task


def test_task_status_counts_groups_by_status():
    tasks = [_task("pending"), _task("running"), _task("running")]
    assert task_status_counts(tasks) == {"pending": 1, "running": 2}


def test_sync_terminal_detail_covers_failed_and_other_terminal():
    assert "retry" in sync_terminal_detail("failed").lower()
    assert "finished" in sync_terminal_detail("completed").lower()


def test_sync_review_gate_detail_mentions_awaiting_count():
    detail = sync_review_gate_detail(3)
    assert "3 task(s)" in detail
    assert "review" in detail.lower()


def test_align_running_process_to_review_required():
    proc = Process(goal="g", status="running", failure_reason="x")
    align_running_process_to_review_required(proc)
    assert proc.status == "task_review_required"
    assert proc.failure_reason is None


def test_reset_running_tasks_to_pending_only_mutates_running():
    running = _task("running")
    pending = _task("pending")
    reset = reset_running_tasks_to_pending([running, pending])
    assert reset == 1
    assert running.status == "pending"
    assert running.output is None
    assert running.tokens_used == 0
    assert pending.status == "pending"
    assert pending.output == "x"
