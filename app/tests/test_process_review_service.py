from __future__ import annotations

from models import Process, TaskNode
from shared_enums import ReviewDecision
from services.process_review_service import apply_task_review_decision


def _task() -> TaskNode:
    t = TaskNode(
        process_id=1,
        client_uuid="writer",
        role="r",
        system_prompt="s",
        instructions="i",
        status="awaiting_review",
        requires_review=True,
        output="draft",
    )
    t.dependencies = []
    return t


def _process() -> Process:
    return Process(goal="g", status="task_review_required")


def test_apply_task_review_decision_request_changes():
    task = _task()
    process = _process()
    res = apply_task_review_decision(
        task=task,
        process=process,
        decision=ReviewDecision.REQUEST_CHANGES,
        output=None,
        feedback="needs more detail",
        instructions="expand section 2",
    )
    assert res.status == "requeued"
    assert res.revision_count == 1
    assert task.status == "pending"
    assert process.status == "running"


def test_apply_task_review_decision_request_changes_requires_feedback():
    task = _task()
    process = _process()
    try:
        apply_task_review_decision(
            task=task,
            process=process,
            decision=ReviewDecision.REQUEST_CHANGES,
            output=None,
            feedback="  ",
            instructions=None,
        )
    except ValueError as e:
        assert "feedback is required" in str(e)
    else:
        raise AssertionError("expected ValueError")


def test_apply_task_review_decision_reject():
    task = _task()
    process = _process()
    res = apply_task_review_decision(
        task=task,
        process=process,
        decision=ReviewDecision.REJECT,
        output=None,
        feedback=None,
        instructions=None,
    )
    assert res.status == "rejected"
    assert task.status == "failed"
    assert process.status == "failed"


def test_apply_task_review_decision_approve():
    task = _task()
    process = _process()
    res = apply_task_review_decision(
        task=task,
        process=process,
        decision=ReviewDecision.APPROVE,
        output="final output",
        feedback=None,
        instructions=None,
    )
    assert res.status == "approved"
    assert task.status == "completed"
    assert task.output == "final output"
    assert process.status == "running"
