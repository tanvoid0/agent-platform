"""Shared fixtures: in-memory DB (via engine patch) + DAGExecutor mock."""

from unittest.mock import AsyncMock, MagicMock

import pytest
from fastapi.testclient import TestClient
from sqlalchemy.pool import StaticPool
from sqlmodel import create_engine

import models  # noqa: F401 — register tables on SQLModel.metadata
import todos.models  # noqa: F401 — todo board tables
import assistant.models  # noqa: F401 — assistant tables
import playground.models  # noqa: F401 — playground chat tables
import coder.models  # noqa: F401 — coder agent chat tables
from database import create_db_and_tables
from llm_proxy.core.provider_config import clear_runtime_provider_bases
from main import app


@pytest.fixture(autouse=True)
def _isolate_llm_runtime_bases(monkeypatch):
    """Prevent TestClient lifespan discovery from leaking across tests."""
    monkeypatch.setenv("LOCAL_LLM_AUTO_DISCOVER", "0")
    clear_runtime_provider_bases()
    yield
    clear_runtime_provider_bases()


@pytest.fixture(autouse=True)
def _no_startup_recovery_by_default(monkeypatch):
    """TestClient lifespan must not requeue leftover processes mid-test.

    The recovery test enables this explicitly.
    """
    monkeypatch.setenv("AGENT_PLATFORM_RESUME_ON_STARTUP", "0")


@pytest.fixture(autouse=True)
def _api_routes_without_bearer_by_default(monkeypatch):
    """Match CI: HTTP APIs do not require Bearer unless a key is configured.

    Developers with AGENT_PLATFORM_MASTER_KEY set in the shell would otherwise
    get 401 on every TestClient call. Tests that need a key set it explicitly.
    """
    monkeypatch.delenv("AGENT_PLATFORM_MASTER_KEY", raising=False)


@pytest.fixture(autouse=True)
def _disable_smart_chat_titles_by_default(monkeypatch):
    """Existing API tests expect fallback titles from the first user message."""
    monkeypatch.setenv("CHAT_SMART_TITLES", "0")


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
