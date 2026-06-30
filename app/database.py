import json
import logging
import os
import sys
import time
from contextlib import contextmanager
from pathlib import Path

from dotenv import load_dotenv

logger = logging.getLogger(__name__)


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
from sqlalchemy import event, inspect, text
from sqlmodel import Session, create_engine

# Support PostgreSQL or SQLite
_db_url = (os.getenv("DATABASE_URL") or "").strip()
if _db_url:
    # PostgreSQL via DATABASE_URL
    db_url = _db_url
    connect_args = {}
else:
    # Fall back to SQLite
    _db_raw = (os.getenv("AGENT_PLATFORM_DB_PATH") or "data/agent_platform.db").strip()
    _db_path = Path(_db_raw)
    if _db_path.parent != Path("."):
        _db_path.parent.mkdir(parents=True, exist_ok=True)
    db_url = f"sqlite:///{_db_path.as_posix()}"
    # busy_timeout (seconds) reduces "database is locked" under concurrent writers.
    _sqlite_busy_timeout_s = float(os.getenv("AGENT_PLATFORM_SQLITE_BUSY_TIMEOUT_SECONDS", "30"))
    connect_args = {"check_same_thread": False, "timeout": _sqlite_busy_timeout_s}

engine = create_engine(db_url, connect_args=connect_args)


@event.listens_for(engine, "connect")
def _on_connect(dbapi_connection, _connection_record) -> None:
    if engine.url.get_backend_name() == "sqlite":
        cursor = dbapi_connection.cursor()
        cursor.execute("PRAGMA journal_mode=WAL;")
        cursor.execute("PRAGMA synchronous=NORMAL;")
        cursor.close()

_APP_DIR = Path(__file__).resolve().parent
_ALEMBIC_CFG = Config(str(_APP_DIR / "alembic.ini"))


def _is_sqlite() -> bool:
    """Checked against the *current* ``engine`` global, not cached at import.

    Tests monkeypatch ``database.engine`` to an in-memory SQLite engine after this
    module loads (e.g. when ``DATABASE_URL`` points at PostgreSQL); callers must
    re-check the live engine rather than a value frozen at import time.
    """
    return engine.url.get_backend_name() == "sqlite"


@contextmanager
def _sqlite_startup_migration_lock():
    """Serialize schema work across uvicorn workers (each runs lifespan on SQLite)."""
    if not _is_sqlite():
        yield  # PostgreSQL handles this natively
        return
    db_path = Path(engine.url.database or "data/agent_platform.db")
    lock_path = db_path.with_name(f"{db_path.name}.startup.lock")
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    fh = open(lock_path, "a+b")  # noqa: SIM115 — short-lived lock file handle
    timeout_s = float(os.getenv("AGENT_PLATFORM_SQLITE_STARTUP_LOCK_TIMEOUT_SECONDS", "120"))
    deadline = time.time() + timeout_s
    try:
        if sys.platform == "win32":
            import msvcrt

            while True:
                fh.seek(0)
                try:
                    msvcrt.locking(fh.fileno(), msvcrt.LK_NBLCK, 1)
                    break
                except OSError:
                    if time.time() >= deadline:
                        fh.close()
                        raise TimeoutError(
                            "Timed out waiting for SQLite startup migration lock "
                            f"({lock_path}). Another process may be holding it."
                        ) from None
                    time.sleep(0.05)
        else:
            import fcntl

            fd = fh.fileno()
            while True:
                try:
                    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    break
                except BlockingIOError:
                    if time.time() >= deadline:
                        fh.close()
                        raise TimeoutError(
                            "Timed out waiting for SQLite startup migration lock "
                            f"({lock_path}). Another process may be holding it."
                        ) from None
                    time.sleep(0.05)
        yield
    finally:
        if sys.platform == "win32":
            import msvcrt

            fh.seek(0)
            try:
                msvcrt.locking(fh.fileno(), msvcrt.LK_UNLCK, 1)
            except OSError:
                pass
        else:
            import fcntl

            try:
                fcntl.flock(fh.fileno(), fcntl.LOCK_UN)
            except OSError:
                pass
        fh.close()


def _ensure_sqlite_columns() -> None:
    """Legacy: add columns introduced before Alembic (SQLite only).

    Does not call ``apply_process_table_sqlite`` (run→process rename): Alembic revisions
    still reference the ``run`` table name until the rename migration runs.
    """
    if not _is_sqlite():
        return  # PostgreSQL handled by Alembic

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
    with _sqlite_startup_migration_lock():
        with engine.connect() as conn:
            inspector = inspect(conn)
            table_names = inspector.get_table_names()

        has_process = "process" in table_names
        has_run = "run" in table_names
        has_alembic = "alembic_version" in table_names

        if _is_sqlite():
            if (has_process or has_run) and not has_alembic:
                _ensure_sqlite_columns()

            # The Windows hang (commit ca51947) was only ever observed against the
            # on-disk DB file, plausibly an interaction with _sqlite_startup_migration_lock's
            # file lock. In-memory DBs (pytest's `sqlite://` fixture) have no file to
            # contend over, so run migrations normally there — skipping them left test
            # runs with no schema at all ("no such table: ...").
            is_memory_db = not engine.url.database
            if is_memory_db:
                logger.info("Running Alembic migrations for in-memory SQLite...")
                command.upgrade(_ALEMBIC_CFG, "head")
                logger.info("Alembic migrations complete")
            else:
                logger.info("Alembic migrations SKIPPED (command.upgrade hangs on Windows file-based SQLite)")
                # TODO: Investigate and fix the root cause; see commit ca51947.

            from process_table_sqlite import apply_process_table_sqlite
            with engine.begin() as conn:
                apply_process_table_sqlite(conn)

            with engine.connect() as conn:
                conn.exec_driver_sql("PRAGMA journal_mode=WAL;")
                conn.exec_driver_sql("PRAGMA synchronous=NORMAL;")
        else:
            # PostgreSQL: run full Alembic migrations
            logger.info("Running Alembic migrations for PostgreSQL...")
            try:
                command.upgrade(_ALEMBIC_CFG, "head")
                logger.info("Alembic migrations complete")
            except Exception as e:
                logger.error(f"Migration failed: {e}")
                raise

        logger.info("Starting team template seeding...")
        _seed_team_templates_if_empty()
        logger.info("Team templates seeded")

        logger.info("Starting todo domain seeding...")
        _seed_todo_domain_if_empty()
        logger.info("Todo domain seeded")


def _seed_todo_domain_if_empty() -> None:
    from sqlmodel import Session

    from todos.seeds import seed_todo_domain_if_empty

    with Session(engine) as session:
        seed_todo_domain_if_empty(session)


def _seed_team_templates_if_empty() -> None:
    """
    Default teams come from Alembic migrations. Runtime startup only seeds data when
    `teamtemplate` exists and is empty; it no longer mutates schema.
    """
    from default_team_templates import SEED_TEAM_TEMPLATES

    with engine.begin() as conn:
        is_sqlite = _is_sqlite()
        # Get table names compatible with both SQLite and PostgreSQL
        if is_sqlite:
            tables = {
                r[0]
                for r in conn.execute(
                    text("SELECT name FROM sqlite_master WHERE type='table'")
                ).fetchall()
            }
        else:
            tables = {
                r[0]
                for r in conn.execute(
                    text("""
                        SELECT table_name FROM information_schema.tables
                        WHERE table_schema = 'public'
                    """)
                ).fetchall()
            }

        if "teamtemplate" not in tables:
            return

        row = conn.execute(text("SELECT COUNT(*) FROM teamtemplate")).fetchone()
        n = int(row[0]) if row else 0
        if n > 0:
            return

        # Get appropriate NOW function based on database
        now_func = "datetime('now')" if is_sqlite else "now()"

        for tmpl in SEED_TEAM_TEMPLATES:
            conn.execute(
                text(
                    f"""
                    INSERT INTO teamtemplate (
                        name, description, color, category, roster_json, created_at, updated_at
                    )
                    VALUES (
                        :name, :description, :color, :category, :roster_json,
                        {now_func}, {now_func}
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
