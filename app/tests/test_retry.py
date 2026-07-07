"""POST /processes/{id}/retry — planning vs execution branches."""

import json

from sqlmodel import Session, select

from models import EventLog, Process, TaskNode


def _minimal_dag_dict():
    return {
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
    }


def test_retry_planning_failure_schedules_plan(client, test_engine):
    c, mock_cls, mock_inst = client
    with Session(test_engine) as session:
        proc = Process(
            goal="test goal",
            status="failed",
            failure_reason="Planning failed: LLM",
            dag_json=None,
        )
        session.add(proc)
        session.commit()
        session.refresh(proc)
        rid = proc.id

    r = c.post(f"/processes/{rid}/retry")
    assert r.status_code == 200
    body = r.json()
    assert body["process_id"] == rid
    assert body["status"] == "planning"
    assert body["retry"] == "planning"

    mock_cls.assert_called_with(rid)
    mock_inst.plan.assert_called_once()

    with Session(test_engine) as session:
        proc = session.get(Process, rid)
        assert proc.status == "planning"
        assert proc.failure_reason is None

    with Session(test_engine) as session:
        logs = session.exec(select(EventLog).where(EventLog.process_id == rid)).all()
        assert any("re-planning" in log.content for log in logs)


def test_retry_execution_failure_resets_tasks_and_schedules_execute(client, test_engine):
    c, mock_cls, mock_inst = client
    dag = _minimal_dag_dict()
    dag_json = json.dumps(dag)

    with Session(test_engine) as session:
        proc = Process(
            goal="g",
            status="failed",
            failure_reason="Task a failed",
            dag_json=dag_json,
        )
        session.add(proc)
        session.commit()
        session.refresh(proc)
        rid = proc.id
        task = TaskNode(
            process_id=rid,
            client_uuid="a",
            role="R1",
            system_prompt="S",
            instructions="I",
            status="failed",
            output="bad",
        )
        task.dependencies = []
        session.add(task)
        session.commit()

    r = c.post(f"/processes/{rid}/retry")
    assert r.status_code == 200
    body = r.json()
    assert body["retry"] == "execution"
    assert body["status"] == "approved"

    mock_cls.assert_called_with(rid)
    mock_inst.execute_dag.assert_called_once()

    with Session(test_engine) as session:
        proc = session.get(Process, rid)
        assert proc.status == "approved"
        assert proc.failure_reason is None
        tasks = session.exec(select(TaskNode).where(TaskNode.process_id == rid)).all()
        assert len(tasks) == 1
        assert tasks[0].status == "pending"
        assert tasks[0].output is None


def test_retry_not_failed_returns_400(client, test_engine):
    c, _, _ = client
    with Session(test_engine) as session:
        proc = Process(goal="x", status="completed")
        session.add(proc)
        session.commit()
        session.refresh(proc)
        rid = proc.id

    r = c.post(f"/processes/{rid}/retry")
    assert r.status_code == 400


def test_retry_missing_run_returns_404(client):
    c, _, _ = client
    r = c.post("/processes/99999/retry")
    assert r.status_code == 404


def test_retry_execution_without_dag_json_returns_400(client, test_engine):
    c, _, _ = client
    with Session(test_engine) as session:
        proc = Process(goal="g", status="failed", failure_reason="x", dag_json=None)
        session.add(proc)
        session.commit()
        session.refresh(proc)
        rid = proc.id
        task = TaskNode(
            process_id=rid,
            client_uuid="a",
            role="R",
            system_prompt="s",
            instructions="i",
            status="failed",
        )
        task.dependencies = []
        session.add(task)
        session.commit()

    r = c.post(f"/processes/{rid}/retry")
    assert r.status_code == 400
    assert "DAG JSON" in r.json()["error"]["message"]


def test_retry_failed_task_resets_task_and_schedules_execute(client, test_engine):
    c, mock_cls, mock_inst = client
    dag = _minimal_dag_dict()
    dag_json = json.dumps(dag)

    with Session(test_engine) as session:
        proc = Process(
            goal="g",
            status="failed",
            failure_reason="Task a failed: LLM",
            dag_json=dag_json,
        )
        session.add(proc)
        session.commit()
        session.refresh(proc)
        rid = proc.id
        task = TaskNode(
            process_id=rid,
            client_uuid="a",
            role="R1",
            system_prompt="S",
            instructions="I",
            status="failed",
            output="bad",
            tokens_used=99,
        )
        task.dependencies = []
        session.add(task)
        session.commit()
        session.refresh(task)
        tid = task.id

    r = c.post(f"/processes/{rid}/tasks/{tid}/retry")
    assert r.status_code == 200
    body = r.json()
    assert body["process_id"] == rid
    assert body["task_id"] == tid
    assert body["retry"] == "task"
    assert body["status"] == "approved"

    mock_cls.assert_called_with(rid)
    mock_inst.execute_dag.assert_called_once()

    with Session(test_engine) as session:
        proc = session.get(Process, rid)
        assert proc.status == "approved"
        assert proc.failure_reason is None
        task = session.get(TaskNode, tid)
        assert task.status == "pending"
        assert task.output is None
        assert task.tokens_used == 0


def test_retry_failed_task_wrong_run_returns_404(client, test_engine):
    c, _, _ = client
    dag_json = json.dumps(_minimal_dag_dict())
    with Session(test_engine) as session:
        r1 = Process(goal="a", status="failed", failure_reason="x", dag_json=dag_json)
        r2 = Process(goal="b", status="failed", failure_reason="y", dag_json=dag_json)
        session.add(r1)
        session.add(r2)
        session.commit()
        session.refresh(r1)
        session.refresh(r2)
        task = TaskNode(
            process_id=r1.id,
            client_uuid="a",
            role="R",
            system_prompt="s",
            instructions="i",
            status="failed",
        )
        task.dependencies = []
        session.add(task)
        session.commit()
        session.refresh(task)
        wrong_process_id = r2.id
        task_id = task.id

    r = c.post(f"/processes/{wrong_process_id}/tasks/{task_id}/retry")
    assert r.status_code == 404


def test_retry_failed_task_not_failed_returns_400(client, test_engine):
    c, _, _ = client
    dag_json = json.dumps(_minimal_dag_dict())
    with Session(test_engine) as session:
        proc = Process(goal="g", status="failed", failure_reason="x", dag_json=dag_json)
        session.add(proc)
        session.commit()
        session.refresh(proc)
        task = TaskNode(
            process_id=proc.id,
            client_uuid="a",
            role="R",
            system_prompt="s",
            instructions="i",
            status="completed",
            output="ok",
        )
        task.dependencies = []
        session.add(task)
        session.commit()
        session.refresh(task)
        process_id = proc.id
        task_id = task.id

    r = c.post(f"/processes/{process_id}/tasks/{task_id}/retry")
    assert r.status_code == 400


def test_retry_failed_task_run_not_failed_returns_400(client, test_engine):
    c, _, _ = client
    dag_json = json.dumps(_minimal_dag_dict())
    with Session(test_engine) as session:
        proc = Process(goal="g", status="completed", dag_json=dag_json)
        session.add(proc)
        session.commit()
        session.refresh(proc)
        task = TaskNode(
            process_id=proc.id,
            client_uuid="a",
            role="R",
            system_prompt="s",
            instructions="i",
            status="failed",
        )
        task.dependencies = []
        session.add(task)
        session.commit()
        session.refresh(task)
        process_id = proc.id
        task_id = task.id

    r = c.post(f"/processes/{process_id}/tasks/{task_id}/retry")
    assert r.status_code == 400


def test_retry_failed_task_no_dag_json_returns_400(client, test_engine):
    c, _, _ = client
    with Session(test_engine) as session:
        proc = Process(goal="g", status="failed", failure_reason="x", dag_json=None)
        session.add(proc)
        session.commit()
        session.refresh(proc)
        task = TaskNode(
            process_id=proc.id,
            client_uuid="a",
            role="R",
            system_prompt="s",
            instructions="i",
            status="failed",
        )
        task.dependencies = []
        session.add(task)
        session.commit()
        session.refresh(task)
        process_id = proc.id
        task_id = task.id

    r = c.post(f"/processes/{process_id}/tasks/{task_id}/retry")
    assert r.status_code == 400
