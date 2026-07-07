"""Startup recovery: requeue processes interrupted by a server restart."""

import asyncio
from unittest.mock import AsyncMock, MagicMock

from sqlmodel import Session, select

from models import EventLog, Process, TaskNode
from services.startup_recovery import (
    recover_interrupted_processes,
    run_startup_recovery,
)


def _mock_executor(monkeypatch):
    mock_cls = MagicMock()
    mock_inst = MagicMock()
    mock_inst.plan = AsyncMock()
    mock_inst.execute_dag = AsyncMock()
    mock_cls.return_value = mock_inst
    monkeypatch.setattr("orchestrator.DAGExecutor", mock_cls)
    return mock_cls, mock_inst


def _recovery_events(engine, pid: int) -> list[str]:
    with Session(engine) as session:
        events = session.exec(select(EventLog).where(EventLog.process_id == pid)).all()
        return [e.content for e in events if "Startup recovery" in e.content]


def test_recovery_replans_interrupted_planning(test_engine, monkeypatch):
    mock_cls, mock_inst = _mock_executor(monkeypatch)
    with Session(test_engine) as session:
        proc = Process(goal="g", status="planning")
        session.add(proc)
        session.commit()
        session.refresh(proc)
        pid = proc.id

    counts = asyncio.run(recover_interrupted_processes())

    assert counts["replanned"] == 1
    mock_cls.assert_called_once_with(pid)
    mock_inst.plan.assert_called_once()
    assert _recovery_events(test_engine, pid)


def test_recovery_requeues_approved_with_dag(test_engine, monkeypatch):
    mock_cls, mock_inst = _mock_executor(monkeypatch)
    with Session(test_engine) as session:
        proc = Process(goal="g", status="approved", dag_json='{"subagents": []}')
        session.add(proc)
        session.commit()
        session.refresh(proc)
        pid = proc.id

    counts = asyncio.run(recover_interrupted_processes())

    assert counts["requeued"] == 1
    mock_cls.assert_called_once_with(pid)
    mock_inst.execute_dag.assert_called_once()


def test_recovery_skips_approved_without_dag(test_engine, monkeypatch):
    mock_cls, _ = _mock_executor(monkeypatch)
    with Session(test_engine) as session:
        proc = Process(goal="g", status="approved", dag_json=None)
        session.add(proc)
        session.commit()

    counts = asyncio.run(recover_interrupted_processes())

    assert counts["skipped"] == 1
    mock_cls.assert_not_called()


def test_recovery_resets_running_tasks_and_requeues(test_engine, monkeypatch):
    mock_cls, mock_inst = _mock_executor(monkeypatch)
    with Session(test_engine) as session:
        proc = Process(goal="g", status="running", dag_json="{}")
        session.add(proc)
        session.commit()
        session.refresh(proc)
        pid = proc.id
        task = TaskNode(
            process_id=pid,
            client_uuid="a",
            role="r",
            system_prompt="s",
            instructions="i",
            status="running",
        )
        task.dependencies = []
        session.add(task)
        session.commit()

    counts = asyncio.run(recover_interrupted_processes())

    assert counts["requeued"] == 1
    mock_inst.execute_dag.assert_called_once()
    with Session(test_engine) as session:
        t = session.exec(select(TaskNode).where(TaskNode.process_id == pid)).first()
        assert t.status == "pending"


def test_recovery_aligns_running_with_awaiting_review(test_engine, monkeypatch):
    mock_cls, mock_inst = _mock_executor(monkeypatch)
    with Session(test_engine) as session:
        proc = Process(goal="g", status="running", dag_json="{}")
        session.add(proc)
        session.commit()
        session.refresh(proc)
        pid = proc.id
        task = TaskNode(
            process_id=pid,
            client_uuid="a",
            role="r",
            system_prompt="s",
            instructions="i",
            status="awaiting_review",
        )
        task.dependencies = []
        session.add(task)
        session.commit()

    counts = asyncio.run(recover_interrupted_processes())

    assert counts["aligned_review"] == 1
    mock_inst.execute_dag.assert_not_called()
    mock_inst.plan.assert_not_called()
    with Session(test_engine) as session:
        proc = session.get(Process, pid)
        assert proc.status == "task_review_required"


def test_recovery_leaves_human_gates_and_terminal_untouched(test_engine, monkeypatch):
    mock_cls, _ = _mock_executor(monkeypatch)
    with Session(test_engine) as session:
        for status in ("approval_required", "task_review_required", "completed", "failed", "cancelled"):
            session.add(Process(goal="g", status=status, dag_json="{}"))
        session.commit()

    counts = asyncio.run(recover_interrupted_processes())

    assert counts == {"replanned": 0, "requeued": 0, "aligned_review": 0, "skipped": 0}
    mock_cls.assert_not_called()


def test_recovery_disabled_by_env(test_engine, monkeypatch):
    mock_cls, _ = _mock_executor(monkeypatch)
    monkeypatch.setenv("AGENT_PLATFORM_RESUME_ON_STARTUP", "0")
    with Session(test_engine) as session:
        session.add(Process(goal="g", status="planning"))
        session.commit()

    asyncio.run(run_startup_recovery())

    mock_cls.assert_not_called()
