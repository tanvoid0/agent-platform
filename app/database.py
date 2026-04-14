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

from platform_config import apply_platform_yaml_defaults

# Non-secret defaults from config/agent_platform.yaml (env and .env still win).
apply_platform_yaml_defaults()

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
    """Legacy: add columns introduced before Alembic (create_all did not migrate).

    Does not call ``apply_process_table_sqlite`` (run→process rename): Alembic revisions
    still reference the ``run`` table name until the rename migration runs.
    """
    with engine.begin() as conn:
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
        # client_id is added by Alembic revision h2i3j4k5l6m7 after ``run`` → ``process`` rename.


def create_db_and_tables() -> None:
    """Apply Alembic migrations; legacy DBs without alembic_version get column patches then upgrade."""
    from process_table_sqlite import apply_process_table_sqlite

    with engine.connect() as conn:
        inspector = inspect(conn)
        table_names = inspector.get_table_names()

    has_process = "process" in table_names
    has_run = "run" in table_names
    has_alembic = "alembic_version" in table_names

    if (has_process or has_run) and not has_alembic:
        _ensure_sqlite_columns()

    command.upgrade(_ALEMBIC_CFG, "head")

    with engine.begin() as conn:
        apply_process_table_sqlite(conn)

    with engine.connect() as conn:
        conn.exec_driver_sql("PRAGMA journal_mode=WAL;")
        conn.exec_driver_sql("PRAGMA synchronous=NORMAL;")

    _seed_team_templates_if_empty()


def _seed_team_templates_if_empty() -> None:
    """
    Default teams come from Alembic migrations. Runtime startup only seeds data when
    `teamtemplate` exists and is empty; it no longer mutates schema.
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
            return

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
