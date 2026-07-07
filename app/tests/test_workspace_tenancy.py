"""Workspace tenant: isolation, /me/workspace, master-key admin, token binding."""

import pytest

MASTER_KEY = "test-master-key"


def _master_headers():
    return {"Authorization": f"Bearer {MASTER_KEY}"}


@pytest.fixture(autouse=True)
def _master_key_env(monkeypatch):
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", MASTER_KEY)


def _create_workspace(c, name, slug):
    r = c.post("/workspaces/", json={"name": name, "slug": slug}, headers=_master_headers())
    assert r.status_code == 201, r.text
    return r.json()["id"]


def _create_project(c, workspace_id, name):
    r = c.post(
        "/projects/", json={"name": name, "workspace_id": workspace_id}, headers=_master_headers()
    )
    assert r.status_code == 201, r.text
    return r.json()["id"]


def _create_token(c, workspace_id):
    r = c.post(
        f"/workspaces/{workspace_id}/api-tokens/",
        json={"name": "t", "scopes": ["*"]},
        headers=_master_headers(),
    )
    assert r.status_code == 201, r.text
    return r.json()["token"]


def test_token_cannot_reach_other_workspace_project(client, test_engine):
    c, *_ = client
    ws_a = _create_workspace(c, "A", "a")
    ws_b = _create_workspace(c, "B", "b")
    proj_b = _create_project(c, ws_b, "pb")
    token_a = _create_token(c, ws_a)
    h = {"Authorization": f"Bearer {token_a}"}

    # Cross-workspace project read → 404 isolation.
    assert c.get(f"/projects/{proj_b}", headers=h).status_code == 404


def test_list_projects_scoped_to_token_workspace(client, test_engine):
    c, *_ = client
    ws_a = _create_workspace(c, "A", "a")
    ws_b = _create_workspace(c, "B", "b")
    _create_project(c, ws_a, "pa1")
    _create_project(c, ws_a, "pa2")
    _create_project(c, ws_b, "pb1")
    token_a = _create_token(c, ws_a)
    h = {"Authorization": f"Bearer {token_a}"}

    rows = c.get("/projects/", headers=h).json()["projects"]
    assert {p["name"] for p in rows} == {"pa1", "pa2"}
    assert all(p["workspace_id"] == ws_a for p in rows)


def test_master_key_sees_all_workspaces_and_projects(client, test_engine):
    c, *_ = client
    ws_a = _create_workspace(c, "A", "a")
    ws_b = _create_workspace(c, "B", "b")
    _create_project(c, ws_a, "pa1")
    _create_project(c, ws_b, "pb1")

    ws = c.get("/workspaces/", headers=_master_headers()).json()["workspaces"]
    slugs = {w["slug"] for w in ws}
    assert {"a", "b", "default"} <= slugs

    projects = c.get("/projects/", headers=_master_headers()).json()["projects"]
    assert {"pa1", "pb1"} <= {p["name"] for p in projects}


def test_create_api_token_binds_to_path_workspace(client, test_engine):
    c, *_ = client
    ws = _create_workspace(c, "A", "a")
    r = c.post(
        f"/workspaces/{ws}/api-tokens/",
        json={"name": "t", "scopes": ["*"]},
        headers=_master_headers(),
    )
    assert r.status_code == 201
    assert r.json()["workspace_id"] == ws


def test_me_workspace_returns_token_workspace(client, test_engine):
    c, *_ = client
    ws = _create_workspace(c, "Acme", "acme")
    token = _create_token(c, ws)
    h = {"Authorization": f"Bearer {token}"}

    r = c.get("/me/workspace", headers=h)
    assert r.status_code == 200
    body = r.json()
    assert body["id"] == ws
    assert body["slug"] == "acme"


def test_me_workspace_master_key_400(client, test_engine):
    c, *_ = client
    r = c.get("/me/workspace", headers=_master_headers())
    assert r.status_code == 400


_ROSTER = {"roles": [{"id": "a", "name": "Writer", "description": "writes", "modality": "text", "parent_id": None}]}


def _create_team(c, headers, name, workspace_id=None):
    body = {"name": name, "roster": _ROSTER}
    if workspace_id is not None:
        body["workspace_id"] = workspace_id
    r = c.post("/teams/", json=body, headers=headers)
    assert r.status_code == 201, r.text
    return r.json()


def test_global_team_visible_to_workspace_token(client, test_engine):
    c, *_ = client
    ws = _create_workspace(c, "A", "a")
    token = _create_token(c, ws)
    h = {"Authorization": f"Bearer {token}"}
    # Master creates a global team (no workspace_id).
    glob = _create_team(c, _master_headers(), "GlobalTeam")
    assert glob["workspace_id"] is None

    names = {t["name"] for t in c.get("/teams/", headers=h).json()["teams"]}
    assert "GlobalTeam" in names
    assert c.get(f"/teams/{glob['id']}", headers=h).status_code == 200


def test_workspace_team_isolated_and_owned(client, test_engine):
    c, *_ = client
    ws_a = _create_workspace(c, "A", "a")
    ws_b = _create_workspace(c, "B", "b")
    tok_a = {"Authorization": f"Bearer {_create_token(c, ws_a)}"}
    tok_b = {"Authorization": f"Bearer {_create_token(c, ws_b)}"}

    # Workspace A token creates its own team (workspace_id forced to A).
    team = _create_team(c, tok_a, "TeamA")
    assert team["workspace_id"] == ws_a

    # Visible to A, invisible to B.
    assert "TeamA" in {t["name"] for t in c.get("/teams/", headers=tok_a).json()["teams"]}
    assert "TeamA" not in {t["name"] for t in c.get("/teams/", headers=tok_b).json()["teams"]}
    assert c.get(f"/teams/{team['id']}", headers=tok_b).status_code == 404

    # B cannot modify or delete A's team.
    assert c.patch(f"/teams/{team['id']}", json={"name": "x"}, headers=tok_b).status_code == 404
    assert c.delete(f"/teams/{team['id']}", headers=tok_b).status_code == 404


def test_workspace_token_cannot_modify_global_team(client, test_engine):
    c, *_ = client
    ws = _create_workspace(c, "A", "a")
    h = {"Authorization": f"Bearer {_create_token(c, ws)}"}
    glob = _create_team(c, _master_headers(), "GlobalTeam")
    # Visible but read-only for a workspace token.
    assert c.patch(f"/teams/{glob['id']}", json={"name": "x"}, headers=h).status_code == 404
    assert c.delete(f"/teams/{glob['id']}", headers=h).status_code == 404


def test_workspace_token_cannot_manage_workspaces(client, test_engine):
    c, *_ = client
    ws = _create_workspace(c, "A", "a")
    token = _create_token(c, ws)
    h = {"Authorization": f"Bearer {token}"}
    assert c.get("/workspaces/", headers=h).status_code == 403
    assert c.post("/workspaces/", json={"name": "x"}, headers=h).status_code == 403


def test_assistant_isolated_by_workspace(client, test_engine):
    c, *_ = client
    ws_a = _create_workspace(c, "A", "a")
    ws_b = _create_workspace(c, "B", "b")
    proj_b = _create_project(c, ws_b, "pb")
    token_a = _create_token(c, ws_a)
    h = {"Authorization": f"Bearer {token_a}"}

    assert c.get(f"/api/v1/assistant/dashboard?project_id={proj_b}", headers=h).status_code == 404
