"""The one snapshot the desktop Status page renders itself from, plus the log ring behind Logs."""

import logging

from sqlmodel import Session

from models import Process
from observability import RingBufferHandler


def test_status_reports_readiness_processes_and_paths(client):
    c, _, _ = client
    r = c.get("/api/v1/system/status")
    assert r.status_code == 200
    body = r.json()

    assert body["service"] == "agent-platform"
    # Both readiness blocks carry an explicit ok, so the page never has to infer it from the HTTP
    # status of a call that succeeded.
    assert isinstance(body["readiness"]["ok"], bool)
    assert isinstance(body["llm_proxy"]["ok"], bool)
    assert body["processes"] == {"by_status": {}, "active": 0, "total": 0}
    assert body["paths"]["database_backend"] == "sqlite"
    assert body["uptime_seconds"] >= 0


def test_status_counts_active_runs_apart_from_finished(client, test_engine):
    c, _, _ = client
    with Session(test_engine) as session:
        for status in ("running", "approval_required", "completed", "failed"):
            session.add(Process(goal=f"goal {status}", status=status))
        session.commit()

    counts = c.get("/api/v1/system/status").json()["processes"]
    assert counts["total"] == 4
    # `approval_required` is stalled on a human, not finished — a monitoring screen has to show it.
    assert counts["active"] == 2
    assert counts["by_status"]["completed"] == 1


def _record(message: str) -> logging.LogRecord:
    return logging.LogRecord("t", logging.INFO, __file__, 1, message, None, None)


def test_log_ring_serves_only_what_the_caller_has_not_seen():
    ring = RingBufferHandler(capacity=3)
    ring.setFormatter(logging.Formatter("%(message)s"))
    for i in range(2):
        ring.emit(_record(f"line {i}"))

    first = ring.snapshot()
    assert first["lines"] == ["line 0", "line 1"]
    assert first["next"] == 2
    assert first["dropped"] == 0

    ring.emit(_record("line 2"))
    assert ring.snapshot(first["next"])["lines"] == ["line 2"]


def test_log_ring_reports_the_gap_when_it_wraps():
    ring = RingBufferHandler(capacity=2)
    ring.setFormatter(logging.Formatter("%(message)s"))
    for i in range(5):
        ring.emit(_record(f"line {i}"))

    # The caller last saw seq 0; lines 1 and 2 fell out, and saying so beats a silent gap.
    snap = ring.snapshot(after=1)
    assert snap["lines"] == ["line 3", "line 4"]
    assert snap["dropped"] == 2


def test_logs_endpoint_answers_even_before_logging_is_wired(client):
    c, _, _ = client
    r = c.get("/api/v1/system/logs")
    assert r.status_code == 200
    assert set(r.json()) == {"lines", "next", "dropped"}
