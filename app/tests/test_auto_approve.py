"""Auto-approve: POST /processes flag + env, and planner → execute without manual POST /approve."""

import asyncio
from unittest.mock import AsyncMock

from sqlmodel import Session

from database import create_db_and_tables
from models import Process


def test_post_runs_passes_auto_approve_to_executor(client, test_engine):
    c, mock_cls, _mock_inst = client
    tr = c.get("/teams/")
    tid = tr.json()["teams"][0]["id"]
    r = c.post("/processes", json={"goal": "g", "auto_approve": True, "team_template_id": tid})
    assert r.status_code == 200
    assert mock_cls.call_args.kwargs.get("auto_approve") is True


def test_post_runs_auto_approve_defaults_false(client, test_engine):
    c, mock_cls, _mock_inst = client
    tr = c.get("/teams/")
    tid = tr.json()["teams"][0]["id"]
    r = c.post("/processes", json={"goal": "g", "team_template_id": tid})
    assert r.status_code == 200
    assert mock_cls.call_args.kwargs.get("auto_approve") is False


def test_plan_with_auto_approve_invokes_execute_dag(monkeypatch, test_engine):
    monkeypatch.setattr("orchestrator.engine", test_engine)
    create_db_and_tables()

    async def fake_planner_dag(_goal: str, _team_context: str | None = None):
        return (
            {
                "team_name": "T",
                "goal_restatement": "G",
                "subagents": [
                    {
                        "client_uuid": "a",
                        "role": "R1",
                        "system_prompt": "S",
                        "instructions": "I",
                        "dependencies": [],
                    }
                ],
            },
            0,
            0.0,
        )

    monkeypatch.setattr("orchestrator.generate_planner_dag", fake_planner_dag)

    from orchestrator import DAGExecutor

    with Session(test_engine) as session:
        proc = Process(goal="g", status="pending")
        session.add(proc)
        session.commit()
        session.refresh(proc)
        rid = proc.id

    ex = DAGExecutor(rid, auto_approve=True)
    mock_exec = AsyncMock()
    ex.execute_dag = mock_exec
    asyncio.run(ex.plan("goal"))
    mock_exec.assert_called_once()


def test_plan_without_auto_approve_does_not_execute(monkeypatch, test_engine):
    monkeypatch.setattr("orchestrator.engine", test_engine)
    create_db_and_tables()

    async def fake_planner_dag(_goal: str, _team_context: str | None = None):
        return (
            {
                "team_name": "T",
                "goal_restatement": "G",
                "subagents": [
                    {
                        "client_uuid": "a",
                        "role": "R1",
                        "system_prompt": "S",
                        "instructions": "I",
                        "dependencies": [],
                    }
                ],
            },
            0,
            0.0,
        )

    monkeypatch.setattr("orchestrator.generate_planner_dag", fake_planner_dag)

    from orchestrator import DAGExecutor

    with Session(test_engine) as session:
        proc = Process(goal="g", status="pending")
        session.add(proc)
        session.commit()
        session.refresh(proc)
        rid = proc.id

    ex = DAGExecutor(rid, auto_approve=False)
    mock_exec = AsyncMock()
    ex.execute_dag = mock_exec
    asyncio.run(ex.plan("goal"))
    mock_exec.assert_not_called()


def test_env_auto_approve_triggers_execute(monkeypatch, test_engine):
    monkeypatch.setenv("AGENT_PLATFORM_AUTO_APPROVE", "1")
    monkeypatch.setattr("orchestrator.engine", test_engine)
    create_db_and_tables()

    async def fake_planner_dag(_goal: str, _team_context: str | None = None):
        return (
            {
                "team_name": "T",
                "goal_restatement": "G",
                "subagents": [
                    {
                        "client_uuid": "a",
                        "role": "R1",
                        "system_prompt": "S",
                        "instructions": "I",
                        "dependencies": [],
                    }
                ],
            },
            0,
            0.0,
        )

    monkeypatch.setattr("orchestrator.generate_planner_dag", fake_planner_dag)

    from orchestrator import DAGExecutor

    with Session(test_engine) as session:
        proc = Process(goal="g", status="pending")
        session.add(proc)
        session.commit()
        session.refresh(proc)
        rid = proc.id

    ex = DAGExecutor(rid, auto_approve=False)
    mock_exec = AsyncMock()
    ex.execute_dag = mock_exec
    asyncio.run(ex.plan("goal"))
    mock_exec.assert_called_once()
