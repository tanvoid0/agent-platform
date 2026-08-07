"""Shared fixtures: in-memory DB (via engine patch) + DAGExecutor mock.

Set ``AGENT_PLATFORM_TEST_BASE_URL`` to run a test file against a *running*
server over HTTP instead of in-process — the parity harness for ADR 0007, where
a domain migrates to Rust only once its own test file passes against both. Add
``AGENT_PLATFORM_TEST_KEY`` when that server has a master key. Only tests that
assert on HTTP behaviour can run this way; anything that reaches for the mocked
``DAGExecutor`` or monkeypatches server internals is testing Python objects, not
the contract, and will fail loudly rather than silently prove nothing.
"""

import os
from unittest.mock import AsyncMock, MagicMock

import httpx
import pytest
from fastapi.testclient import TestClient
from sqlalchemy.pool import StaticPool
from sqlmodel import create_engine

import models  # noqa: F401 — register tables on SQLModel.metadata
import todos.models  # noqa: F401 — todo board tables
import assistant.models  # noqa: F401 — assistant tables
import coder.models  # noqa: F401 — coder agent chat tables
import model_ops.models  # noqa: F401 — model build/train tables
import workflows.models  # noqa: F401 — workflow automation tables
from database import create_db_and_tables
from llm_proxy.core.provider_config import clear_runtime_provider_bases
from llm_proxy.services.model_capabilities import clear_capability_cache
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
def _no_speech_backend_by_default(monkeypatch):
    """Match CI: no speech upstream is configured unless a test configures one.

    Same hazard as the master key above — a developer with SPEECH_API_BASE in
    `.env` gets a resolved speech backend (and its default voice) where the
    tests expect none.
    """
    for var in ("SPEECH_API_BASE", "SPEECH_API_KEY", "SPEECH_DEFAULT_VOICE", "SPEECH_DEFAULT_FORMAT"):
        monkeypatch.delenv(var, raising=False)


@pytest.fixture(autouse=True)
def _isolate_config_dir(tmp_path_factory, monkeypatch):
    """Keep CONFIG_DIR (config.yaml, .env, capability cache) inside the test's tmp dir.

    Without this the suite reads and writes the developer's real config dir, so a
    cached model_capabilities.json makes probe-dependent tests pass locally and
    fail on a clean checkout.
    """
    monkeypatch.setenv("CONFIG_DIR", str(tmp_path_factory.mktemp("config_dir")))
    clear_capability_cache(disk=False)
    yield
    clear_capability_cache(disk=False)


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
    base_url = (os.getenv("AGENT_PLATFORM_TEST_BASE_URL") or "").strip()
    if base_url:
        headers = {}
        key = (os.getenv("AGENT_PLATFORM_TEST_KEY") or "").strip()
        if key:
            headers["Authorization"] = f"Bearer {key}"
        # The mocks are placeholders: a live server runs its own executor, so a
        # test that asserts on them is not a parity test and should fail here.
        # TestClient follows redirects and a bare httpx.Client does not. Without
        # this, FastAPI's trailing-slash 307 fails the test against Python while
        # the Rust server — which answers both spellings — passes it.
        with httpx.Client(
            base_url=base_url, headers=headers, timeout=30.0, follow_redirects=True
        ) as c:
            yield c, MagicMock(), MagicMock()
        return

    mock_cls = MagicMock()
    mock_inst = MagicMock()
    mock_inst.plan = AsyncMock()
    mock_inst.execute_dag = AsyncMock()
    mock_cls.return_value = mock_inst
    monkeypatch.setattr("process_routes.DAGExecutor", mock_cls)

    with TestClient(app) as c:
        yield c, mock_cls, mock_inst
