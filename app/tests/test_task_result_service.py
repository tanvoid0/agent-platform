from models import Process, TaskNode
from services.task_result_service import apply_task_failure, apply_task_success
from sqlmodel import Session


def _seed_task_and_process(test_engine, requires_review: bool = False) -> tuple[int, int]:
    with Session(test_engine) as session:
        proc = Process(goal="goal", status="running")
        session.add(proc)
        session.commit()
        session.refresh(proc)
        task = TaskNode(
            process_id=proc.id,
            client_uuid="a",
            role="writer",
            system_prompt="sys",
            instructions="ins",
            status="running",
            requires_review=requires_review,
        )
        session.add(task)
        session.commit()
        session.refresh(task)
        return proc.id, task.id


def test_apply_task_success_completed_path(test_engine):
    process_id, task_id = _seed_task_and_process(test_engine, requires_review=False)
    with Session(test_engine) as session:
        task, run, needs_expand = apply_task_success(
            session,
            process_id=process_id,
            task_id=task_id,
            output="done",
            tokens=12,
            task_cost=0.3,
            tool_calls=2,
        )
        assert task.status == "completed"
        assert task.output == "done"
        assert run.total_tokens == 12
        assert run.tool_invocations_used == 2
        assert needs_expand is True


def test_apply_task_success_review_path(test_engine):
    process_id, task_id = _seed_task_and_process(test_engine, requires_review=True)
    with Session(test_engine) as session:
        task, run, needs_expand = apply_task_success(
            session,
            process_id=process_id,
            task_id=task_id,
            output="draft",
            tokens=8,
            task_cost=0.1,
            tool_calls=0,
        )
        assert task.status == "awaiting_review"
        assert run.status == "task_review_required"
        assert needs_expand is False


def test_apply_task_failure_marks_failed(test_engine):
    process_id, task_id = _seed_task_and_process(test_engine, requires_review=False)
    with Session(test_engine) as session:
        task, run = apply_task_failure(
            session,
            process_id=process_id,
            task_id=task_id,
            failure_debug_json='{"source":"llm"}',
            failure_reason="boom",
        )
        assert task.status == "failed"
        assert run.status == "failed"
        assert run.failure_reason == "boom"
