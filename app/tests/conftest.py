"""Shared fixtures: in-memory DB (via engine patch) + DAGExecutor mock."""

from unittest.mock import AsyncMock, MagicMock

import pytest
from fastapi.testclient import TestClient
from sqlalchemy.pool import StaticPool
from sqlmodel import create_engine

import models  # noqa: F401 — register tables on SQLModel.metadata
from database import create_db_and_tables
from main import app


@pytest.fixture
def test_engine(monkeypatch):
    """Swap app DB to a fresh in-memory SQLite for the duration of the test."""
    eng = create_engine(
        "sqlite://",
        connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )
    monkeypatch.setattr("database.engine", eng)
    create_db_and_tables()
    yield eng


@pytest.fixture
def client(test_engine, monkeypatch):
    mock_cls = MagicMock()
    mock_inst = MagicMock()
    mock_inst.plan = AsyncMock()
    mock_inst.execute_dag = AsyncMock()
    mock_cls.return_value = mock_inst
    monkeypatch.setattr("process_routes.DAGExecutor", mock_cls)

    with TestClient(app) as c:
        yield c, mock_cls, mock_inst
