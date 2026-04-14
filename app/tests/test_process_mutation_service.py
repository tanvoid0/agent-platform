from models import Process, TaskNode
from services.process_mutation_service import (
    approve_task_output,
    reject_task_and_fail_process,
    requeue_task_for_changes,
    reset_failed_task_for_retry,
)


def _task() -> TaskNode:
    return TaskNode(
        process_id=1,
        client_uuid="agent-1",
        role="writer",
        system_prompt="s",
        instructions="i",
        status="awaiting_review",
    )


def _proc() -> Process:
    return Process(goal="g", status="task_review_required")


def test_requeue_task_for_changes_sets_pending_state():
    task = _task()
    task.output = "draft"
    process = _proc()

    requeue_task_for_changes(
        task=task,
        process=process,
        feedback="needs revision",
        instructions="redo section 2",
    )

    assert task.status == "pending"
    assert task.draft_output == "draft"
    assert task.output is None
    assert task.review_feedback == "needs revision"
    assert task.instructions == "redo section 2"
    assert process.status == "running"


def test_reject_task_and_fail_process_sets_failure_fields():
    task = _task()
    process = _proc()

    reject_task_and_fail_process(task=task, process=process)

    assert task.status == "failed"
    assert "review_reject" in (task.failure_debug_json or "")
    assert process.status == "failed"
    assert "rejected" in (process.failure_reason or "")


def test_approve_task_output_marks_completed():
    task = _task()
    process = _proc()

    approve_task_output(task=task, process=process, output="final")

    assert task.status == "completed"
    assert task.output == "final"
    assert task.completed_at is not None
    assert process.status == "running"


def test_reset_failed_task_for_retry_clears_runtime_fields():
    task = _task()
    task.status = "failed"
    task.output = "old"
    task.failure_debug_json = '{"source":"x"}'
    process = _proc()
    process.status = "failed"

    reset_failed_task_for_retry(task=task, process=process)

    assert task.status == "pending"
    assert task.output is None
    assert task.failure_debug_json is None
    assert process.status == "approved"
