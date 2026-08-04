"""Auth must leave `workspace_id` visible to log records emitted inside handlers.

`verify_project_api_token._dependency` writes a contextvar via
`update_request_context`. It stays `async def` and pushes only its blocking DB work
through `asyncio.to_thread` for exactly this reason: a plain `def` dependency is
dispatched to a worker thread, anyio hands that thread a *copy* of the context, and
the write is discarded when the thread returns. Tenant attribution would silently
vanish from every handler-emitted log line.

The `request.completed` line from RequestLoggingMiddleware would not catch this —
it re-derives the workspace from `request.state`. Only a record logged from inside
a handler does, which is what this asserts.
"""

from __future__ import annotations

import logging

import pytest
from fastapi import Depends, FastAPI
from fastapi.testclient import TestClient

from api_tokens.auth import require_valid_token
from observability import StructuredContextFilter

MASTER_KEY = "test-master-key"


@pytest.fixture(autouse=True)
def _master_key_env(monkeypatch):
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", MASTER_KEY)


def _mint_token(c) -> str:
    master = {"Authorization": f"Bearer {MASTER_KEY}"}
    r = c.post("/api/v1/workspaces/", json={"name": "ctx", "slug": "ctx"}, headers=master)
    assert r.status_code == 201, r.text
    workspace_id = r.json()["id"]

    r = c.post(
        f"/api/v1/workspaces/{workspace_id}/api-tokens/",
        json={"name": "ctx", "scopes": ["*"]},
        headers=master,
    )
    assert r.status_code == 201, r.text
    return r.json()["token"], workspace_id


def test_handler_logs_carry_workspace_id_from_the_auth_dependency(client, test_engine):
    c, *_ = client
    token, workspace_id = _mint_token(c)

    records: list[logging.LogRecord] = []

    class _Capture(logging.Handler):
        def emit(self, record):
            records.append(record)

    handler = _Capture()
    handler.addFilter(StructuredContextFilter())  # what the real JSON handler uses
    probe_logger = logging.getLogger("test.auth_context_probe")
    probe_logger.addHandler(handler)
    probe_logger.setLevel(logging.INFO)

    probe_app = FastAPI()

    @probe_app.get("/probe", dependencies=[Depends(require_valid_token)])
    def probe():
        probe_logger.info("logged from inside the handler")
        return {"ok": True}

    try:
        with TestClient(probe_app) as probe_client:
            r = probe_client.get("/probe", headers={"Authorization": f"Bearer {token}"})
        assert r.status_code == 200, r.text
    finally:
        probe_logger.removeHandler(handler)

    assert len(records) == 1
    assert records[0].workspace_id == str(workspace_id), (
        "workspace_id did not survive into the handler's context — has the auth "
        "dependency been changed to a plain `def` (and so moved to a threadpool)?"
    )
