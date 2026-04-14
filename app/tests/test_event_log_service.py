from models import EventLog, Process
from services.event_log_service import append_event
from sqlmodel import Session, select


def test_append_event_writes_row(test_engine):
    process_id = None
    with Session(test_engine) as session:
        proc = Process(goal="g", status="running")
        session.add(proc)
        session.commit()
        session.refresh(proc)
        process_id = proc.id
        append_event(
            session,
            process_id=proc.id,
            event_type="status_change",
            content="hello",
        )

    with Session(test_engine) as session:
        rows = session.exec(select(EventLog).where(EventLog.process_id == process_id)).all()
        assert len(rows) == 1
        assert rows[0].event_type == "status_change"
        assert rows[0].content == "hello"
