import os
from sqlmodel import create_engine, SQLModel, Session

DB_FILE = "agent_runs.db"
sqlite_url = f"sqlite:///{DB_FILE}"

# Use WAL mode for better concurrency with background tasks
connect_args = {"check_same_thread": False}
engine = create_engine(sqlite_url, connect_args=connect_args)

def create_db_and_tables():
    SQLModel.metadata.create_all(engine)
    # Enable WAL mode
    with engine.connect() as conn:
        conn.exec_driver_sql("PRAGMA journal_mode=WAL;")
        conn.exec_driver_sql("PRAGMA synchronous=NORMAL;")

def get_session():
    with Session(engine) as session:
        yield session
