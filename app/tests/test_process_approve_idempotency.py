"""POST /processes/{id}/approve — duplicate-safe responses."""

from sqlmodel import Session

from models import Process


def test_approve_idempotent_when_already_approved(client, test_engine):
    """Second approve after status is approved must not re-schedule execution."""
    c, mock_cls, mock_inst = client
    with Session(test_engine) as session:
        proc = Process(goal="g", status="approved", dag_json="{}")
        session.add(proc)
        session.commit()
        session.refresh(proc)
        pid = proc.id

    r = c.post(f"/processes/{pid}/approve", json={"dag_json": "{}"})
    assert r.status_code == 200
    body = r.json()
    assert body.get("idempotent") is True
    assert body.get("status") == "approved"
    mock_inst.execute_dag.assert_not_called()
    mock_cls.assert_not_called()
