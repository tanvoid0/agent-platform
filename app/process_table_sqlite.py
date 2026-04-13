"""
SQLite DDL: parent table ``process`` and FK columns ``process_id`` on ``tasknode`` / ``eventlog``.

Shared by Alembic and `database.create_db_and_tables` for DBs that skipped the revision.
"""

from __future__ import annotations

from sqlalchemy import text


def _table_names(conn) -> set[str]:
    rows = conn.execute(text("SELECT name FROM sqlite_master WHERE type='table'")).fetchall()
    return {r[0] for r in rows}


def _columns(conn, table: str) -> set[str]:
    rows = conn.execute(text(f"PRAGMA table_info({table})")).fetchall()
    return {r[1] for r in rows}


def apply_process_table_sqlite(conn) -> bool:
    """
    Migrate legacy ``run`` / ``run_id`` names to ``process`` / ``process_id`` when present.
    Returns True if any DDL ran. Idempotent.
    """
    tables = _table_names(conn)
    did = False

    if "run" in tables and "process" not in tables:
        conn.execute(text("ALTER TABLE run RENAME TO process"))
        did = True
        tables = _table_names(conn)

    if "process" not in tables:
        return did

    if "tasknode" in tables and "run_id" in _columns(conn, "tasknode"):
        conn.execute(text("PRAGMA foreign_keys=OFF"))
        try:
            conn.execute(
                text(
                    """
                    CREATE TABLE tasknode__new (
                        id INTEGER NOT NULL,
                        process_id INTEGER NOT NULL,
                        client_uuid VARCHAR NOT NULL,
                        parent_client_uuid VARCHAR,
                        role VARCHAR NOT NULL,
                        system_prompt VARCHAR NOT NULL,
                        instructions VARCHAR NOT NULL,
                        llm_model VARCHAR(128),
                        dependencies_json VARCHAR NOT NULL,
                        status VARCHAR NOT NULL,
                        requires_review INTEGER NOT NULL DEFAULT 0,
                        review_feedback VARCHAR,
                        revision_count INTEGER NOT NULL DEFAULT 0,
                        draft_output VARCHAR,
                        output VARCHAR,
                        tokens_used INTEGER NOT NULL,
                        started_at DATETIME,
                        completed_at DATETIME,
                        PRIMARY KEY (id),
                        FOREIGN KEY(process_id) REFERENCES process (id)
                    )
                    """
                )
            )
            conn.execute(
                text(
                    """
                    INSERT INTO tasknode__new (
                        id, process_id, client_uuid, parent_client_uuid, role, system_prompt,
                        instructions, llm_model, dependencies_json, status, requires_review,
                        review_feedback, revision_count, draft_output, output, tokens_used,
                        started_at, completed_at
                    )
                    SELECT
                        id, run_id, client_uuid, parent_client_uuid, role, system_prompt,
                        instructions, llm_model, dependencies_json, status, requires_review,
                        review_feedback, revision_count, draft_output, output, tokens_used,
                        started_at, completed_at
                    FROM tasknode
                    """
                )
            )
            conn.execute(text("DROP TABLE tasknode"))
            conn.execute(text("ALTER TABLE tasknode__new RENAME TO tasknode"))
            conn.execute(text("CREATE INDEX IF NOT EXISTS ix_tasknode_client_uuid ON tasknode (client_uuid)"))
            conn.execute(
                text(
                    "CREATE INDEX IF NOT EXISTS ix_tasknode_parent_client_uuid ON tasknode (parent_client_uuid)"
                )
            )
        finally:
            conn.execute(text("PRAGMA foreign_keys=ON"))
        did = True

    if "eventlog" in _table_names(conn) and "run_id" in _columns(conn, "eventlog"):
        conn.execute(text("PRAGMA foreign_keys=OFF"))
        try:
            conn.execute(text("DROP INDEX IF EXISTS ix_eventlog_run_id"))
            conn.execute(
                text(
                    """
                    CREATE TABLE eventlog__new (
                        id INTEGER NOT NULL,
                        process_id INTEGER NOT NULL,
                        task_id INTEGER,
                        event_type VARCHAR NOT NULL,
                        content VARCHAR NOT NULL,
                        created_at DATETIME NOT NULL,
                        PRIMARY KEY (id),
                        FOREIGN KEY(process_id) REFERENCES process (id),
                        FOREIGN KEY(task_id) REFERENCES tasknode (id)
                    )
                    """
                )
            )
            conn.execute(
                text(
                    """
                    INSERT INTO eventlog__new (id, process_id, task_id, event_type, content, created_at)
                    SELECT id, run_id, task_id, event_type, content, created_at FROM eventlog
                    """
                )
            )
            conn.execute(text("DROP TABLE eventlog"))
            conn.execute(text("ALTER TABLE eventlog__new RENAME TO eventlog"))
            conn.execute(text("CREATE INDEX IF NOT EXISTS ix_eventlog_process_id ON eventlog (process_id)"))
        finally:
            conn.execute(text("PRAGMA foreign_keys=ON"))
        did = True

    return did
