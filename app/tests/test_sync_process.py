"""POST /processes/{id}/sync — recover stuck planning/execution or report human gates."""

import json

from sqlmodel import Session, select

from models import Process, TaskNode


def test_sync_404(client, test_engine):
    c, _, _ = client
    r = c.post("/processes/999999/sync")
    assert r.status_code == 404


def test_sync_terminal_completed(client, test_engine):
    c, _, _ = client
    with Session(test_engine) as session:
        proc = Process(goal="g", status="completed")
        session.add(proc)
        session.commit()
        session.refresh(proc)
        pid = proc.id

    r = c.post(f"/processes/{pid}/sync")
    assert r.status_code == 200
    body = r.json()
    assert body["action"] == "none"
    assert body["process_status"] == "completed"
    assert "finished" in body["detail"]


def test_sync_terminal_failed_hints_retry(client, test_engine):
    c, _, _ = client
    with Session(test_engine) as session:
        proc = Process(goal="g", status="failed", failure_reason="x")
        session.add(proc)
        session.commit()
        session.refresh(proc)
        pid = proc.id

    r = c.post(f"/processes/{pid}/sync")
    assert r.status_code == 200
    body = r.json()
    assert body["action"] == "none"
    assert body["process_status"] == "failed"
    assert "retry" in body["detail"].lower()


def test_sync_schedules_plan_for_planning(client, test_engine):
    c, mock_cls, mock_inst = client
    with Session(test_engine) as session:
        proc = Process(goal="hello", status="planning")
        session.add(proc)
        session.commit()
        session.refresh(proc)
        pid = proc.id

    r = c.post(f"/processes/{pid}/sync")
    assert r.status_code == 200
    body = r.json()
    assert body["action"] == "requeued_plan"
    mock_inst.plan.assert_called_once()


def test_sync_schedules_execute_for_approved(client, test_engine):
    c, mock_cls, mock_inst = client
    dag = json.dumps(
        {
            "team_name": "t",
            "goal_restatement": "g",
            "subagents": [
                {
                    "client_uuid": "a",
                    "role": "r",
                    "system_prompt": "s",
                    "instructions": "i",
                    "dependencies": [],
                }
            ],
        }
    )
    with Session(test_engine) as session:
        proc = Process(goal="g", status="approved", dag_json=dag)
        session.add(proc)
        session.commit()
        session.refresh(proc)
        pid = proc.id

    r = c.post(f"/processes/{pid}/sync")
    assert r.status_code == 200
    body = r.json()
    assert body["action"] == "requeued_execution"
    mock_inst.execute_dag.assert_called_once()


def test_sync_running_resets_tasks_and_schedules_execute(client, test_engine):
    c, mock_cls, mock_inst = client
    dag = json.dumps(
        {
            "team_name": "t",
            "goal_restatement": "g",
            "subagents": [
                {
                    "client_uuid": "a",
                    "role": "r",
                    "system_prompt": "s",
                    "instructions": "i",
                    "dependencies": [],
                }
            ],
        }
    )
    with Session(test_engine) as session:
        proc = Process(goal="g", status="running", dag_json=dag)
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

    r = c.post(f"/processes/{pid}/sync")
    assert r.status_code == 200
    body = r.json()
    assert body["action"] == "requeued_execution"
    assert body.get("reset_running_tasks") == 1
    mock_inst.execute_dag.assert_called_once()

    with Session(test_engine) as session:
        t = session.exec(select(TaskNode).where(TaskNode.process_id == pid)).first()
        assert t is not None
        assert t.status == "pending"


def test_sync_blocked_approval_required(client, test_engine):
    c, _, _ = client
    with Session(test_engine) as session:
        proc = Process(goal="g", status="approval_required", dag_json="{}")
        session.add(proc)
        session.commit()
        session.refresh(proc)
        pid = proc.id

    r = c.post(f"/processes/{pid}/sync")
    assert r.status_code == 200
    assert r.json()["action"] == "blocked"


def test_sync_aligns_running_with_awaiting_review(client, test_engine):
    c, mock_cls, mock_inst = client
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

    r = c.post(f"/processes/{pid}/sync")
    assert r.status_code == 200
    body = r.json()
    assert body["action"] == "aligned_status"
    assert body["process_status"] == "task_review_required"
    mock_inst.execute_dag.assert_not_called()
