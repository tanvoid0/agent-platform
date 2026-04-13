"""GET /processes/{id}/events — list persisted EventLog rows."""

from sqlmodel import Session, select

from models import EventLog, Process


def test_events_404_when_run_missing(client, test_engine):
    c, _, _ = client
    r = c.get("/processes/999999/events")
    assert r.status_code == 404


def test_events_lists_and_filters_by_type(client, test_engine):
    c, _, _ = client
    with Session(test_engine) as session:
        proc = Process(goal="g", status="completed")
        session.add(proc)
        session.commit()
        session.refresh(proc)
        rid = proc.id
        session.add(
            EventLog(process_id=rid, task_id=None, event_type="status_change", content="started"),
        )
        session.add(
            EventLog(process_id=rid, task_id=None, event_type="trace", content="hello"),
        )
        session.commit()

    r = c.get(f"/processes/{rid}/events")
    assert r.status_code == 200
    body = r.json()
    assert len(body["events"]) == 2
    assert body["events"][0]["event_type"] == "status_change"
    assert body["events"][1]["event_type"] == "trace"

    r2 = c.get(f"/processes/{rid}/events?event_type=trace")
    assert r2.status_code == 200
    evs = r2.json()["events"]
    assert len(evs) == 1
    assert evs[0]["content"] == "hello"

    with Session(test_engine) as session:
        count = len(session.exec(select(EventLog).where(EventLog.process_id == rid)).all())
        assert count == 2
