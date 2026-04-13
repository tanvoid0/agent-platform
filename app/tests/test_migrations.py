"""Alembic applies on startup; legacy DBs without alembic_version get stamped after column ensures."""

from sqlalchemy import inspect
from sqlalchemy.pool import StaticPool
from sqlmodel import create_engine

import models  # noqa: F401
from database import create_db_and_tables


def test_upgrade_creates_tables_and_version(monkeypatch):
    eng = create_engine(
        "sqlite://",
        connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    monkeypatch.setattr("database.engine", eng)
    create_db_and_tables()

    with eng.connect() as conn:
        insp = inspect(conn)
        names = insp.get_table_names()

    assert "process" in names
    assert "tasknode" in names
    assert "eventlog" in names
    assert "alembic_version" in names

    with eng.connect() as conn:
        info = conn.exec_driver_sql("PRAGMA table_info(tasknode)").fetchall()
    task_cols = {row[1] for row in info}
    assert "requires_review" in task_cols
    assert "review_feedback" in task_cols
    assert "revision_count" in task_cols
    assert "draft_output" in task_cols
    assert "failure_debug_json" in task_cols
    assert "process_id" in task_cols

    with eng.connect() as conn:
        pinfo = conn.exec_driver_sql("PRAGMA table_info(process)").fetchall()
    proc_cols = {row[1] for row in pinfo}
    assert "client_id" in proc_cols
    assert "project_id" in proc_cols

    assert "project" in names


def test_legacy_db_without_alembic_version_gets_stamped(monkeypatch):
    """Simulate pre-Alembic SQLite: legacy `run` table exists, no alembic_version."""
    eng = create_engine(
        "sqlite://",
        connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    monkeypatch.setattr("database.engine", eng)

    with eng.begin() as conn:
        conn.exec_driver_sql(
            """
            CREATE TABLE run (
                id INTEGER NOT NULL PRIMARY KEY,
                goal VARCHAR NOT NULL,
                status VARCHAR NOT NULL,
                dag_json VARCHAR,
                total_tokens INTEGER NOT NULL,
                created_at DATETIME NOT NULL,
                updated_at DATETIME NOT NULL
            )
            """
        )

    create_db_and_tables()

    with eng.connect() as conn:
        insp = inspect(conn)
        names = insp.get_table_names()

    assert "alembic_version" in names
    assert "teamtemplate" in names
    with eng.connect() as conn:
        n = conn.exec_driver_sql("SELECT COUNT(*) FROM teamtemplate").fetchone()[0]
    assert int(n) >= 3
    with eng.connect() as conn:
        info = conn.exec_driver_sql("PRAGMA table_info(process)").fetchall()
    col_names = {row[1] for row in info}
    assert "failure_reason" in col_names
    assert "total_cost" in col_names
    assert "tool_invocations_used" in col_names
    assert "client_id" in col_names
