from __future__ import annotations

import json

import pytest
from sqlmodel import Session, select

from models import Process, TaskNode
from services.process_approval_service import (
    apply_process_approval,
    is_idempotent_approval_status,
    validate_approved_dag_json,
)


def _minimal_dag_json() -> str:
    return json.dumps(
        {
            "team_name": "T",
            "goal_restatement": "G",
            "subagents": [
                {
                    "client_uuid": "a",
                    "role": "R",
                    "system_prompt": "S",
                    "instructions": "I",
                    "dependencies": [],
                }
            ],
        }
    )


def test_is_idempotent_approval_status():
    assert is_idempotent_approval_status("running") is True
    assert is_idempotent_approval_status("completed") is True
    assert is_idempotent_approval_status("approved") is True
    assert is_idempotent_approval_status("approval_required") is False


def test_validate_approved_dag_json_raises_on_invalid_json():
    with pytest.raises(ValueError, match="Invalid JSON for approved DAG"):
        validate_approved_dag_json("{")


def test_apply_process_approval_replaces_tasks_and_sets_canonical_dag(test_engine):
    with Session(test_engine) as session:
        proc = Process(goal="g", status="approval_required")
        session.add(proc)
        session.commit()
        session.refresh(proc)
        pid = proc.id
        old = TaskNode(
            process_id=pid,
            client_uuid="old",
            role="old",
            system_prompt="s",
            instructions="i",
            status="pending",
        )
        old.dependencies = []
        session.add(old)
        session.commit()

        apply_process_approval(session, process_id=pid, dag_json=_minimal_dag_json())
        session.commit()

        tasks = session.exec(select(TaskNode).where(TaskNode.process_id == pid)).all()
        assert len(tasks) == 1
        assert tasks[0].client_uuid == "a"
        proc2 = session.get(Process, pid)
        assert proc2 is not None and proc2.dag_json is not None
