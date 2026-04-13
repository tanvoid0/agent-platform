import json
import os
from pathlib import Path

from dotenv import load_dotenv


def _agent_platform_dotenv_path() -> Path:
    """Repo: agent-platform/app/*.py → agent-platform/.env. Docker image: /app/*.py → /app/.env."""
    here = Path(__file__).resolve().parent
    repo_root_env = here.parent / ".env"
    flat_image_env = here / ".env"
    if flat_image_env.is_file():
        return flat_image_env
    if repo_root_env.is_file():
        return repo_root_env
    # Default target when missing (load_dotenv no-ops); avoid /.env when parent is filesystem root.
    if here.parent == Path(here.anchor):
        return flat_image_env
    return repo_root_env


# Load agent-platform/.env before any os.getenv used for DB path (uvicorn + Alembic + tests).
load_dotenv(_agent_platform_dotenv_path())

from alembic import command
from alembic.config import Config
from sqlalchemy import inspect, text
from sqlmodel import Session, create_engine

_db_raw = (os.getenv("AGENT_PLATFORM_DB_PATH") or "data/agent_platform.db").strip()
_db_path = Path(_db_raw)
if _db_path.parent != Path("."):
    _db_path.parent.mkdir(parents=True, exist_ok=True)

sqlite_url = f"sqlite:///{_db_path.as_posix()}"

connect_args = {"check_same_thread": False}
engine = create_engine(sqlite_url, connect_args=connect_args)

_APP_DIR = Path(__file__).resolve().parent
_ALEMBIC_CFG = Config(str(_APP_DIR / "alembic.ini"))


def _ensure_sqlite_columns() -> None:
    """Legacy: add columns introduced before Alembic (create_all did not migrate)."""
    with engine.begin() as conn:
        from process_table_sqlite import apply_process_table_sqlite

        apply_process_table_sqlite(conn)
        target = None
        for tbl in ("process", "run"):
            rows = conn.exec_driver_sql(f"PRAGMA table_info({tbl})").fetchall()
            if rows:
                target = tbl
                break
        if not target:
            return
        col_names = {row[1] for row in rows}
        if "failure_reason" not in col_names:
            conn.exec_driver_sql(f"ALTER TABLE {target} ADD COLUMN failure_reason TEXT")
        if "total_cost" not in col_names:
            conn.exec_driver_sql(f"ALTER TABLE {target} ADD COLUMN total_cost REAL DEFAULT 0")
        if "tool_invocations_used" not in col_names:
            conn.exec_driver_sql(f"ALTER TABLE {target} ADD COLUMN tool_invocations_used INTEGER DEFAULT 0")
        if "client_id" not in col_names:
            conn.exec_driver_sql(f"ALTER TABLE {target} ADD COLUMN client_id VARCHAR(256)")
            conn.exec_driver_sql(
                f"CREATE INDEX IF NOT EXISTS ix_{target}_client_id ON {target} (client_id)"
            )


def create_db_and_tables() -> None:
    """Apply Alembic migrations; legacy DBs without alembic_version are stamped after column ensures."""
    from process_table_sqlite import apply_process_table_sqlite

    with engine.begin() as conn:
        apply_process_table_sqlite(conn)

    with engine.connect() as conn:
        inspector = inspect(conn)
        table_names = inspector.get_table_names()

    has_process = "process" in table_names
    has_run = "run" in table_names
    has_alembic = "alembic_version" in table_names

    if (has_process or has_run) and not has_alembic:
        _ensure_sqlite_columns()
        command.stamp(_ALEMBIC_CFG, "head")
    else:
        command.upgrade(_ALEMBIC_CFG, "head")

    with engine.begin() as conn:
        apply_process_table_sqlite(conn)

    with engine.connect() as conn:
        conn.exec_driver_sql("PRAGMA journal_mode=WAL;")
        conn.exec_driver_sql("PRAGMA synchronous=NORMAL;")

    _ensure_team_template_schema_and_seed()


def _ensure_team_template_schema_and_seed() -> None:
    """
    Default teams come from Alembic revision e5f6a7b8c9d0. Legacy DBs were stamped to head without
    running upgrades, so `teamtemplate` may be missing or empty. Idempotently create schema and
    insert SEED_TEAM_TEMPLATES when the table has no rows.
    """
    from default_team_templates import SEED_TEAM_TEMPLATES

    with engine.begin() as conn:
        tables = {
            r[0]
            for r in conn.execute(
                text("SELECT name FROM sqlite_master WHERE type='table'")
            ).fetchall()
        }

        if "teamtemplate" not in tables:
            conn.execute(
                text(
                    """
                    CREATE TABLE teamtemplate (
                        id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                        name VARCHAR(256) NOT NULL,
                        description VARCHAR(4096),
                        color VARCHAR(32),
                        category VARCHAR(128),
                        roster_json VARCHAR NOT NULL,
                        created_at DATETIME NOT NULL,
                        updated_at DATETIME NOT NULL
                    )
                    """
                )
            )
        else:
            cols = {row[1] for row in conn.execute(text("PRAGMA table_info(teamtemplate)")).fetchall()}
            if "category" not in cols:
                conn.execute(text("ALTER TABLE teamtemplate ADD COLUMN category VARCHAR(128)"))

        if "process" in tables:
            pcols = {row[1] for row in conn.execute(text("PRAGMA table_info(process)")).fetchall()}
            if "team_template_id" not in pcols:
                conn.execute(text("ALTER TABLE process ADD COLUMN team_template_id INTEGER"))
            if "team_snapshot_json" not in pcols:
                conn.execute(text("ALTER TABLE process ADD COLUMN team_snapshot_json VARCHAR"))

        row = conn.execute(text("SELECT COUNT(*) FROM teamtemplate")).fetchone()
        n = int(row[0]) if row else 0
        if n > 0:
            return

        for tmpl in SEED_TEAM_TEMPLATES:
            conn.execute(
                text(
                    """
                    INSERT INTO teamtemplate (
                        name, description, color, category, roster_json, created_at, updated_at
                    )
                    VALUES (
                        :name, :description, :color, :category, :roster_json,
                        datetime('now'), datetime('now')
                    )
                    """
                ),
                {
                    "name": tmpl["name"],
                    "description": tmpl["description"],
                    "color": tmpl["color"],
                    "category": tmpl.get("category"),
                    "roster_json": json.dumps(tmpl["roster"]),
                },
            )


def get_session():
    with Session(engine) as session:
        yield session
