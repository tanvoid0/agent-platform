"""DAGExecutor paths with mocked LLM (no live orchestrator)."""

from __future__ import annotations

import asyncio
import json

import pytest
from sqlmodel import Session, select

from models import Process, TaskNode
from orchestrator import DAGExecutor


@pytest.fixture
def executor_db(test_engine, monkeypatch):
    """DAGExecutor uses `from database import engine` — rebind to the test engine."""
    monkeypatch.setattr("orchestrator.engine", test_engine)
    return test_engine


def _dag_one_task(
    *,
    client_uuid: str = "a",
    subdecompose: bool = False,
) -> dict:
    agent = {
        "client_uuid": client_uuid,
        "role": "R",
        "system_prompt": "sys",
        "instructions": "ins",
        "dependencies": [],
    }
    if subdecompose:
        agent["subdecompose"] = True
    return {
        "team_name": "T",
        "goal_restatement": "G",
        "subagents": [agent],
    }


def _run_with_task(
    executor_db,
    *,
    dag: dict,
    run_status: str = "running",
    requires_review: bool = False,
) -> tuple[int, int]:
    """Insert Process + single TaskNode; return (process_id, task_id)."""
    with Session(executor_db) as session:
        proc = Process(
            goal="goal text",
            status=run_status,
            dag_json=json.dumps(dag),
        )
        session.add(proc)
        session.commit()
        session.refresh(proc)
        rid = proc.id
        task = TaskNode(
            process_id=rid,
            client_uuid=dag["subagents"][0]["client_uuid"],
            role=dag["subagents"][0]["role"],
            system_prompt=dag["subagents"][0]["system_prompt"],
            instructions=dag["subagents"][0]["instructions"],
            status="pending",
            requires_review=requires_review,
        )
        task.dependencies = dag["subagents"][0].get("dependencies", [])
        session.add(task)
        session.commit()
        session.refresh(task)
        return rid, task.id


def _process_id_from_task(executor_db, task_id: int) -> int:
    with Session(executor_db) as session:
        t = session.get(TaskNode, task_id)
        return t.process_id


def test_execute_task_plain_llm_updates_task_and_run(executor_db, monkeypatch):
    dag = _dag_one_task()
    _, task_id = _run_with_task(executor_db, dag=dag)

    async def fake_call_llm(*args, **kwargs):
        return ("assistant output", 42, 0.0)

    monkeypatch.setattr("orchestrator.call_llm", fake_call_llm)
    monkeypatch.setenv("AGENT_PLATFORM_TOOLS_ENABLED", "")
    monkeypatch.setenv("AGENT_PLATFORM_TOOL_BUDGET_PER_RUN", "0")

    ex = DAGExecutor(_process_id_from_task(executor_db, task_id))
    asyncio.run(ex.execute_task(task_id))

    with Session(executor_db) as session:
        task = session.get(TaskNode, task_id)
        assert task.status == "completed"
        assert task.output == "assistant output"
        assert task.tokens_used == 42
        proc = session.get(Process, task.process_id)
        assert proc.total_tokens == 42
        assert proc.tool_invocations_used == 0


def test_execute_task_tools_path_counts_invocations(executor_db, monkeypatch):
    dag = _dag_one_task()
    _, task_id = _run_with_task(executor_db, dag=dag)

    async def fake_with_tools(*args, **kwargs):
        return ("with tools out", 100, 0.02, 3)

    monkeypatch.setattr("orchestrator.call_llm_with_tools", fake_with_tools)
    monkeypatch.setenv("AGENT_PLATFORM_TOOLS_ENABLED", "1")
    monkeypatch.setenv("AGENT_PLATFORM_TOOLS_ALLOWLIST", "echo")
    monkeypatch.setenv("AGENT_PLATFORM_TOOL_BUDGET_PER_RUN", "10")

    ex = DAGExecutor(_process_id_from_task(executor_db, task_id))
    asyncio.run(ex.execute_task(task_id))

    with Session(executor_db) as session:
        proc = session.get(Process, _process_id_from_task(executor_db, task_id))
        assert proc.tool_invocations_used == 3
        assert proc.total_tokens == 100
        assert abs(proc.total_cost - 0.02) < 1e-9


def test_subdag_expansion_adds_nodes_and_merges_dag(executor_db, monkeypatch):
    monkeypatch.setenv("AGENT_PLATFORM_SUBDECOMP_MAX_EXPANSIONS", "1")
    monkeypatch.setenv("AGENT_PLATFORM_SUBDECOMP_MAX_NEW_TASKS", "16")
    monkeypatch.setenv("AGENT_PLATFORM_TOOLS_ENABLED", "")
    monkeypatch.setenv("AGENT_PLATFORM_TOOL_BUDGET_PER_RUN", "0")

    dag = _dag_one_task(subdecompose=True)
    _, task_id = _run_with_task(executor_db, dag=dag)

    async def fake_call_llm(*args, **kwargs):
        return ("parent done", 10, 0.0)

    new_agent = {
        "client_uuid": "child-1",
        "role": "Child",
        "system_prompt": "cs",
        "instructions": "ci",
        "dependencies": ["a"],
    }

    async def fake_expand(**kwargs):
        return ([new_agent], 7, 0.001)

    monkeypatch.setattr("orchestrator.call_llm", fake_call_llm)
    monkeypatch.setattr("orchestrator.generate_subdag_expansion", fake_expand)

    ex = DAGExecutor(_process_id_from_task(executor_db, task_id))
    asyncio.run(ex.execute_task(task_id))

    with Session(executor_db) as session:
        tasks = session.exec(select(TaskNode).where(TaskNode.process_id == ex.process_id)).all()
        uuids = {t.client_uuid for t in tasks}
        assert uuids == {"a", "child-1"}
        child = next(t for t in tasks if t.client_uuid == "child-1")
        assert child.status == "pending"
        assert child.dependencies == ["a"]
        assert child.parent_client_uuid == "a"

        proc = session.get(Process, ex.process_id)
        assert proc.total_tokens == 10 + 7
        assert abs(proc.total_cost - 0.001) < 1e-9
        merged = json.loads(proc.dag_json)
        muuids = {s["client_uuid"] for s in merged["subagents"]}
        assert muuids == {"a", "child-1"}


def test_requires_review_sets_awaiting_review_and_pauses_run(executor_db, monkeypatch):
    dag = _dag_one_task()
    _, task_id = _run_with_task(executor_db, dag=dag, requires_review=True)

    async def fake_call_llm(*args, **kwargs):
        return ("draft output", 5, 0.0)

    monkeypatch.setattr("orchestrator.call_llm", fake_call_llm)
    monkeypatch.setenv("AGENT_PLATFORM_TOOLS_ENABLED", "")
    monkeypatch.setenv("AGENT_PLATFORM_TOOL_BUDGET_PER_RUN", "0")

    ex = DAGExecutor(_process_id_from_task(executor_db, task_id))
    asyncio.run(ex.execute_dag())

    with Session(executor_db) as session:
        task = session.get(TaskNode, task_id)
        proc = session.get(Process, task.process_id)
        assert task.status == "awaiting_review"
        assert task.output == "draft output"
        assert proc.status == "task_review_required"


def test_execute_dag_no_deadlock_when_dependent_pending_and_upstream_awaiting_review(
    executor_db, monkeypatch,
):
    """B depends on A; A is awaiting_review — must pause, not deadlock."""
    dag = {
        "team_name": "T",
        "goal_restatement": "G",
        "subagents": [
            {
                "client_uuid": "a",
                "role": "A",
                "system_prompt": "sys",
                "instructions": "ins",
                "dependencies": [],
            },
            {
                "client_uuid": "b",
                "role": "B",
                "system_prompt": "sys",
                "instructions": "ins",
                "dependencies": ["a"],
            },
        ],
    }
    with Session(executor_db) as session:
        proc = Process(goal="goal text", status="running", dag_json=json.dumps(dag))
        session.add(proc)
        session.commit()
        session.refresh(proc)
        rid = proc.id
        ta = TaskNode(
            process_id=rid,
            client_uuid="a",
            role="A",
            system_prompt="sys",
            instructions="ins",
            status="pending",
            requires_review=True,
        )
        ta.dependencies = []
        tb = TaskNode(
            process_id=rid,
            client_uuid="b",
            role="B",
            system_prompt="sys",
            instructions="ins",
            status="pending",
            requires_review=False,
        )
        tb.dependencies = ["a"]
        session.add(ta)
        session.add(tb)
        session.commit()
        session.refresh(ta)

    calls = {"n": 0}

    async def fake_call_llm(*args, **kwargs):
        calls["n"] += 1
        return ("draft a", 3, 0.0)

    monkeypatch.setattr("orchestrator.call_llm", fake_call_llm)
    monkeypatch.setenv("AGENT_PLATFORM_TOOLS_ENABLED", "")
    monkeypatch.setenv("AGENT_PLATFORM_TOOL_BUDGET_PER_RUN", "0")

    ex = DAGExecutor(rid)
    asyncio.run(ex.execute_dag())

    with Session(executor_db) as session:
        proc = session.get(Process, rid)
        assert proc.status == "task_review_required"
        assert calls["n"] == 1
        ta2 = session.exec(
            select(TaskNode).where(TaskNode.process_id == rid).where(TaskNode.client_uuid == "a")
        ).first()
        tb2 = session.exec(
            select(TaskNode).where(TaskNode.process_id == rid).where(TaskNode.client_uuid == "b")
        ).first()
        assert ta2.status == "awaiting_review"
        assert tb2.status == "pending"
        assert ta2.reviewer_client_uuid == "b"


def test_reviewer_assigned_when_parallel_peer_completes(executor_db, monkeypatch):
    """A needs review; independent B completes — B is assigned as reviewer for A."""
    monkeypatch.setenv("AGENT_PLATFORM_DAG_MAX_CONCURRENT_TASKS", "1")
    monkeypatch.setenv("AGENT_PLATFORM_TOOLS_ENABLED", "")
    monkeypatch.setenv("AGENT_PLATFORM_TOOL_BUDGET_PER_RUN", "0")

    dag = {
        "team_name": "T",
        "goal_restatement": "G",
        "subagents": [
            {
                "client_uuid": "a",
                "role": "Author",
                "system_prompt": "sys",
                "instructions": "ins",
                "dependencies": [],
                "requires_review": True,
            },
            {
                "client_uuid": "b",
                "role": "Peer",
                "system_prompt": "sys",
                "instructions": "ins",
                "dependencies": [],
            },
        ],
    }
    with Session(executor_db) as session:
        proc = Process(goal="goal text", status="running", dag_json=json.dumps(dag))
        session.add(proc)
        session.commit()
        session.refresh(proc)
        rid = proc.id
        ta = TaskNode(
            process_id=rid,
            client_uuid="a",
            role="Author",
            system_prompt="sys",
            instructions="ins",
            status="pending",
            requires_review=True,
        )
        ta.dependencies = []
        tb = TaskNode(
            process_id=rid,
            client_uuid="b",
            role="Peer",
            system_prompt="sys",
            instructions="ins",
            status="pending",
            requires_review=False,
        )
        tb.dependencies = []
        session.add(ta)
        session.add(tb)
        session.commit()
        session.refresh(ta)

    calls = {"n": 0}

    async def fake_call_llm(*args, **kwargs):
        calls["n"] += 1
        return ("out", 1, 0.0)

    monkeypatch.setattr("orchestrator.call_llm", fake_call_llm)

    ex = DAGExecutor(rid)
    asyncio.run(ex.execute_dag())

    with Session(executor_db) as session:
        ta2 = session.exec(
            select(TaskNode).where(TaskNode.process_id == rid).where(TaskNode.client_uuid == "a")
        ).first()
        tb2 = session.exec(
            select(TaskNode).where(TaskNode.process_id == rid).where(TaskNode.client_uuid == "b")
        ).first()
        assert ta2 is not None and ta2.status == "awaiting_review"
        assert tb2 is not None and tb2.status == "completed"
        assert ta2.reviewer_client_uuid == "b"
        assert calls["n"] == 2


def test_execute_dag_batches_ready_tasks_when_max_concurrent_is_one(
    executor_db, monkeypatch,
):
    """Two independent tasks: only one runs at a time when max concurrent is 1 (FIFO by id)."""
    monkeypatch.setenv("AGENT_PLATFORM_DAG_MAX_CONCURRENT_TASKS", "1")
    monkeypatch.setenv("AGENT_PLATFORM_TOOLS_ENABLED", "")
    monkeypatch.setenv("AGENT_PLATFORM_TOOL_BUDGET_PER_RUN", "0")

    dag = {
        "team_name": "T",
        "goal_restatement": "G",
        "subagents": [
            {
                "client_uuid": "a",
                "role": "A",
                "system_prompt": "sys",
                "instructions": "ins",
                "dependencies": [],
            },
            {
                "client_uuid": "b",
                "role": "B",
                "system_prompt": "sys",
                "instructions": "ins",
                "dependencies": [],
            },
        ],
    }
    with Session(executor_db) as session:
        proc = Process(goal="goal text", status="running", dag_json=json.dumps(dag))
        session.add(proc)
        session.commit()
        session.refresh(proc)
        rid = proc.id
        ta = TaskNode(
            process_id=rid,
            client_uuid="a",
            role="A",
            system_prompt="sys",
            instructions="ins",
            status="pending",
            requires_review=False,
        )
        ta.dependencies = []
        tb = TaskNode(
            process_id=rid,
            client_uuid="b",
            role="B",
            system_prompt="sys",
            instructions="ins",
            status="pending",
            requires_review=False,
        )
        tb.dependencies = []
        session.add(ta)
        session.add(tb)
        session.commit()

    calls: list[str] = []

    async def fake_call_llm(*args, **kwargs):
        calls.append("x")
        return ("done", 1, 0.0)

    monkeypatch.setattr("orchestrator.call_llm", fake_call_llm)

    ex = DAGExecutor(rid)
    asyncio.run(ex.execute_dag())

    assert len(calls) == 2
    with Session(executor_db) as session:
        proc = session.get(Process, rid)
        assert proc.status == "completed"


def test_revision_prompt_includes_feedback(executor_db, monkeypatch):
    dag = _dag_one_task()
    _, task_id = _run_with_task(executor_db, dag=dag, requires_review=True)

    captured: list[str] = []

    async def fake_call_llm(messages, *args, **kwargs):
        user = next((m["content"] for m in messages if m["role"] == "user"), "")
        captured.append(user)
        n = len(captured)
        if n == 1:
            return ("first draft", 2, 0.0)
        return ("second draft", 2, 0.0)

    monkeypatch.setattr("orchestrator.call_llm", fake_call_llm)
    monkeypatch.setenv("AGENT_PLATFORM_TOOLS_ENABLED", "")
    monkeypatch.setenv("AGENT_PLATFORM_TOOL_BUDGET_PER_RUN", "0")

    ex = DAGExecutor(_process_id_from_task(executor_db, task_id))
    asyncio.run(ex.execute_task(task_id))

    with Session(executor_db) as session:
        task = session.get(TaskNode, task_id)
        task.draft_output = task.output
        task.output = None
        task.review_feedback = "be shorter"
        task.status = "pending"
        task.started_at = None
        task.completed_at = None
        session.add(task)
        session.commit()

    asyncio.run(ex.execute_task(task_id))

    assert len(captured) == 2
    assert "Previous attempt:" in captured[1]
    assert "first draft" in captured[1]
    assert "be shorter" in captured[1]
