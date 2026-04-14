from models import Process
from services.process_runtime_service import (
    complete_process,
    fail_process,
    pause_process_for_task_review,
    set_process_running_or_review_required,
)
from sqlmodel import Session


def _seed_process(test_engine, status: str = "running") -> int:
    with Session(test_engine) as session:
        proc = Process(goal="g", status=status)
        session.add(proc)
        session.commit()
        session.refresh(proc)
        return proc.id


def test_set_process_running_or_review_required(test_engine):
    process_id = _seed_process(test_engine, "pending")
    with Session(test_engine) as session:
        run = set_process_running_or_review_required(
            session,
            process_id=process_id,
            awaiting_review=False,
        )
        assert run.status == "running"
        run = set_process_running_or_review_required(
            session,
            process_id=process_id,
            awaiting_review=True,
        )
        assert run.status == "task_review_required"


def test_pause_process_for_task_review(test_engine):
    process_id = _seed_process(test_engine, "running")
    with Session(test_engine) as session:
        run = pause_process_for_task_review(session, process_id=process_id)
        assert run.status == "task_review_required"
        assert run.failure_reason is None


def test_complete_process(test_engine):
    process_id = _seed_process(test_engine, "running")
    with Session(test_engine) as session:
        run = complete_process(session, process_id=process_id)
        assert run.status == "completed"
        assert run.failure_reason is None


def test_fail_process(test_engine):
    process_id = _seed_process(test_engine, "running")
    with Session(test_engine) as session:
        run = fail_process(session, process_id=process_id, reason="deadlock")
        assert run.status == "failed"
        assert run.failure_reason == "deadlock"
