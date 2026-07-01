"""Project-scoped API token issuance, auth, usage, revoke/hold, rate limit, isolation."""

import pytest

from api_tokens.rate_limiter import _windows as _rate_limit_windows

MASTER_KEY = "test-master-key"


def _master_headers():
    return {"Authorization": f"Bearer {MASTER_KEY}"}


def _create_project(c, name="TokenProj"):
    r = c.post("/projects/", json={"name": name}, headers=_master_headers())
    assert r.status_code == 201, r.text
    return r.json()["id"]


def _create_token(c, project_id, **kwargs):
    body = {"name": "ext-token", "scopes": ["process:read", "process:write", "chat:write"]}
    body.update(kwargs)
    r = c.post(f"/projects/{project_id}/api-tokens/", json=body, headers=_master_headers())
    assert r.status_code == 201, r.text
    return r.json()


@pytest.fixture(autouse=True)
def _master_key_env(monkeypatch):
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", MASTER_KEY)


@pytest.fixture(autouse=True)
def _reset_rate_limit_windows():
    _rate_limit_windows.clear()
    yield
    _rate_limit_windows.clear()


def test_token_lifecycle_use_then_revoke(client, test_engine):
    c, _mock_cls, _mock_inst = client
    project_id = _create_project(c)
    created = _create_token(c, project_id)
    token_headers = {"Authorization": f"Bearer {created['token']}"}

    r = c.get(f"/projects/{project_id}", headers=token_headers)
    assert r.status_code == 200

    r_revoke = c.post(
        f"/projects/{project_id}/api-tokens/{created['id']}/revoke",
        json={"reason": "rotated"},
        headers=_master_headers(),
    )
    assert r_revoke.status_code == 200
    assert r_revoke.json()["status"] == "revoked"

    r2 = c.get(f"/projects/{project_id}", headers=token_headers)
    assert r2.status_code == 401
    assert r2.json()["error"]["code"] == "TOKEN_REVOKED"


def test_token_hold_blocks_with_403_then_unhold_restores(client, test_engine):
    c, _mock_cls, _mock_inst = client
    project_id = _create_project(c)
    created = _create_token(c, project_id)
    token_headers = {"Authorization": f"Bearer {created['token']}"}

    r_hold = c.post(
        f"/projects/{project_id}/api-tokens/{created['id']}/hold",
        json={"reason": "suspicious activity"},
        headers=_master_headers(),
    )
    assert r_hold.status_code == 200
    assert r_hold.json()["status"] == "held"

    r = c.get(f"/projects/{project_id}", headers=token_headers)
    assert r.status_code == 403
    assert r.json()["error"]["code"] == "TOKEN_HELD"

    r_unhold = c.post(
        f"/projects/{project_id}/api-tokens/{created['id']}/unhold",
        headers=_master_headers(),
    )
    assert r_unhold.status_code == 200
    assert r_unhold.json()["status"] == "active"

    r2 = c.get(f"/projects/{project_id}", headers=token_headers)
    assert r2.status_code == 200


def test_token_rate_limit_returns_429(client, test_engine):
    c, _mock_cls, _mock_inst = client
    project_id = _create_project(c)
    created = _create_token(c, project_id, rate_limit_per_minute=2)
    token_headers = {"Authorization": f"Bearer {created['token']}"}

    r1 = c.get(f"/projects/{project_id}", headers=token_headers)
    r2 = c.get(f"/projects/{project_id}", headers=token_headers)
    r3 = c.get(f"/projects/{project_id}", headers=token_headers)
    assert r1.status_code == 200
    assert r2.status_code == 200
    assert r3.status_code == 429
    assert r3.json()["error"]["code"] == "RATE_LIMIT_EXCEEDED"


def test_token_cannot_manage_other_tokens(client, test_engine):
    c, _mock_cls, _mock_inst = client
    project_id = _create_project(c)
    created = _create_token(c, project_id)
    token_headers = {"Authorization": f"Bearer {created['token']}"}

    r = c.get(f"/projects/{project_id}/api-tokens/", headers=token_headers)
    assert r.status_code == 403


def test_token_scoped_process_isolated_across_projects(client, test_engine):
    c, _mock_cls, _mock_inst = client
    project_a = _create_project(c, "A")
    project_b = _create_project(c, "B")
    token_a = _create_token(c, project_a)
    token_a_headers = {"Authorization": f"Bearer {token_a['token']}"}

    tr = c.get("/teams/", headers=_master_headers())
    tid = tr.json()["teams"][0]["id"]

    # Token for project A cannot start a process under project B.
    r_bad = c.post(
        "/processes",
        json={"goal": "g", "team_template_id": tid, "project_id": project_b},
        headers=token_a_headers,
    )
    assert r_bad.status_code == 404

    # Token for project A can start a process under its own project.
    r_ok = c.post(
        "/processes",
        json={"goal": "g", "team_template_id": tid, "project_id": project_a},
        headers=token_a_headers,
    )
    assert r_ok.status_code == 200
    process_id = r_ok.json()["process_id"]

    r_get = c.get(f"/processes/{process_id}", headers=token_a_headers)
    assert r_get.status_code == 200

    # A token minted for project B cannot read project A's process (404, not 401 — isolation, not auth failure).
    token_b = _create_token(c, project_b)
    token_b_headers = {"Authorization": f"Bearer {token_b['token']}"}
    r_cross = c.get(f"/processes/{process_id}", headers=token_b_headers)
    assert r_cross.status_code == 404


def test_token_missing_scope_returns_403(client, test_engine):
    c, _mock_cls, _mock_inst = client
    project_id = _create_project(c)
    created = _create_token(c, project_id, scopes=["process:read"])
    token_headers = {"Authorization": f"Bearer {created['token']}"}

    tr = c.get("/teams/", headers=_master_headers())
    tid = tr.json()["teams"][0]["id"]

    r = c.post(
        "/processes",
        json={"goal": "g", "team_template_id": tid, "project_id": project_id},
        headers=token_headers,
    )
    assert r.status_code == 403
    assert r.json()["error"]["code"] == "INSUFFICIENT_SCOPE"


def test_token_usage_recorded_on_process_run(client, test_engine):
    c, _mock_cls, _mock_inst = client
    project_id = _create_project(c)
    created = _create_token(c, project_id)
    token_headers = {"Authorization": f"Bearer {created['token']}"}

    tr = c.get("/teams/", headers=_master_headers())
    tid = tr.json()["teams"][0]["id"]

    r = c.post(
        "/processes",
        json={"goal": "g", "team_template_id": tid, "project_id": project_id},
        headers=token_headers,
    )
    assert r.status_code == 200

    from models import Process

    process_id = r.json()["process_id"]
    from sqlmodel import Session

    with Session(test_engine) as session:
        proc = session.get(Process, process_id)
        assert proc.token_id == created["id"]
