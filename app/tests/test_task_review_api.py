"""POST /processes/{id}/tasks/{id}/review — human gate for requires_review tasks."""

import json

from sqlmodel import Session

from models import Process, TaskNode


def test_review_approve_ok_when_run_running_but_task_still_awaiting_review(client, test_engine):
    """Regression: parallel requires_review can leave process.status as running while another task awaits."""
    c, _, _ = client
    dag = {
        "team_name": "T",
        "goal_restatement": "G",
        "subagents": [
            {
                "client_uuid": "a",
                "role": "R",
                "system_prompt": "sys",
                "instructions": "ins",
                "dependencies": [],
            },
        ],
    }
    with Session(test_engine) as session:
        proc = Process(goal="g", status="running", dag_json=json.dumps(dag))
        session.add(proc)
        session.commit()
        session.refresh(proc)
        rid = proc.id
        task = TaskNode(
            process_id=rid,
            client_uuid="a",
            role="R",
            system_prompt="sys",
            instructions="ins",
            status="awaiting_review",
            requires_review=True,
            output="draft",
        )
        task.dependencies = []
        session.add(task)
        session.commit()
        session.refresh(task)
        tid = task.id

    r = c.post(f"/processes/{rid}/tasks/{tid}/review", json={"decision": "approve"})
    assert r.status_code == 200, r.text
    assert r.json().get("status") == "approved"
