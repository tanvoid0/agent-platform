"""Workspace tenant: isolation, /me/workspace, master-key admin, token binding."""

import pytest

MASTER_KEY = "test-master-key"

pytestmark = pytest.mark.contract


def _master_headers():
    return {"Authorization": f"Bearer {MASTER_KEY}"}


@pytest.fixture(autouse=True)
def _master_key_env(monkeypatch):
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", MASTER_KEY)


def _create_workspace(c, name, slug):
    r = c.post("/api/v1/workspaces/", json={"name": name, "slug": slug}, headers=_master_headers())
    assert r.status_code == 201, r.text
    return r.json()["id"]


def _create_project(c, workspace_id, name):
    r = c.post(
        "/api/v1/projects/", json={"name": name, "workspace_id": workspace_id}, headers=_master_headers()
    )
    assert r.status_code == 201, r.text
    return r.json()["id"]


def _create_token(c, workspace_id):
    r = c.post(
        f"/api/v1/workspaces/{workspace_id}/api-tokens/",
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
    assert c.get(f"/api/v1/projects/{proj_b}", headers=h).status_code == 404


def test_list_projects_scoped_to_token_workspace(client, test_engine):
    c, *_ = client
    ws_a = _create_workspace(c, "A", "a")
    ws_b = _create_workspace(c, "B", "b")
    _create_project(c, ws_a, "pa1")
    _create_project(c, ws_a, "pa2")
    _create_project(c, ws_b, "pb1")
    token_a = _create_token(c, ws_a)
    h = {"Authorization": f"Bearer {token_a}"}

    rows = c.get("/api/v1/projects/", headers=h).json()["projects"]
    assert {p["name"] for p in rows} == {"pa1", "pa2"}
    assert all(p["workspace_id"] == ws_a for p in rows)


def test_master_key_sees_all_workspaces_and_projects(client, test_engine):
    c, *_ = client
    ws_a = _create_workspace(c, "A", "a")
    ws_b = _create_workspace(c, "B", "b")
    _create_project(c, ws_a, "pa1")
    _create_project(c, ws_b, "pb1")

    ws = c.get("/api/v1/workspaces/", headers=_master_headers()).json()["workspaces"]
    slugs = {w["slug"] for w in ws}
    assert {"a", "b", "default"} <= slugs

    projects = c.get("/api/v1/projects/", headers=_master_headers()).json()["projects"]
    assert {"pa1", "pb1"} <= {p["name"] for p in projects}


def test_create_api_token_binds_to_path_workspace(client, test_engine):
    c, *_ = client
    ws = _create_workspace(c, "A", "a")
    r = c.post(
        f"/api/v1/workspaces/{ws}/api-tokens/",
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

    r = c.get("/api/v1/me/workspace", headers=h)
    assert r.status_code == 200
    body = r.json()
    assert body["id"] == ws
    assert body["slug"] == "acme"


def test_me_workspace_master_key_400(client, test_engine):
    c, *_ = client
    r = c.get("/api/v1/me/workspace", headers=_master_headers())
    assert r.status_code == 400


_ROSTER = {"roles": [{"id": "a", "name": "Writer", "description": "writes", "modality": "text", "parent_id": None}]}


def _create_team(c, headers, name, workspace_id=None):
    body = {"name": name, "roster": _ROSTER}
    if workspace_id is not None:
        body["workspace_id"] = workspace_id
    r = c.post("/api/v1/teams/", json=body, headers=headers)
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

    names = {t["name"] for t in c.get("/api/v1/teams/", headers=h).json()["teams"]}
    assert "GlobalTeam" in names
    assert c.get(f"/api/v1/teams/{glob['id']}", headers=h).status_code == 200


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
    assert "TeamA" in {t["name"] for t in c.get("/api/v1/teams/", headers=tok_a).json()["teams"]}
    assert "TeamA" not in {t["name"] for t in c.get("/api/v1/teams/", headers=tok_b).json()["teams"]}
    assert c.get(f"/api/v1/teams/{team['id']}", headers=tok_b).status_code == 404

    # B cannot modify or delete A's team.
    assert c.patch(f"/api/v1/teams/{team['id']}", json={"name": "x"}, headers=tok_b).status_code == 404
    assert c.delete(f"/api/v1/teams/{team['id']}", headers=tok_b).status_code == 404


def test_workspace_token_cannot_modify_global_team(client, test_engine):
    c, *_ = client
    ws = _create_workspace(c, "A", "a")
    h = {"Authorization": f"Bearer {_create_token(c, ws)}"}
    glob = _create_team(c, _master_headers(), "GlobalTeam")
    # Visible but read-only for a workspace token.
    assert c.patch(f"/api/v1/teams/{glob['id']}", json={"name": "x"}, headers=h).status_code == 404
    assert c.delete(f"/api/v1/teams/{glob['id']}", headers=h).status_code == 404


def test_workspace_token_cannot_manage_workspaces(client, test_engine):
    c, *_ = client
    ws = _create_workspace(c, "A", "a")
    token = _create_token(c, ws)
    h = {"Authorization": f"Bearer {token}"}
    assert c.get("/api/v1/workspaces/", headers=h).status_code == 403
    assert c.post("/api/v1/workspaces/", json={"name": "x"}, headers=h).status_code == 403


def test_archive_workspace_revokes_tokens_and_hides_tenant(client, test_engine):
    c, *_ = client
    ws = _create_workspace(c, "ArchiveMe", "archive-me")
    _create_project(c, ws, "proj")
    token = _create_token(c, ws)
    team = _create_team(c, _master_headers(), "OwnedTeam", workspace_id=ws)

    r = c.delete(f"/api/v1/workspaces/{ws}", headers=_master_headers())
    assert r.status_code == 200, r.text
    body = r.json()
    assert body["ok"] is True
    assert body["tokens_revoked"] >= 1
    assert body["teams_removed"] >= 1

    listed = c.get("/api/v1/workspaces/", headers=_master_headers()).json()["workspaces"]
    assert all(w["id"] != ws for w in listed)

    assert c.get(f"/api/v1/workspaces/{ws}", headers=_master_headers()).status_code == 404
    assert c.patch(
        f"/api/v1/workspaces/{ws}", json={"name": "x"}, headers=_master_headers()
    ).status_code == 404

    h = {"Authorization": f"Bearer {token}"}
    assert c.get("/api/v1/projects/", headers=h).status_code == 401
    assert c.get(f"/api/v1/teams/{team['id']}", headers=h).status_code == 401

    projects = c.get("/api/v1/projects/", headers=_master_headers()).json()["projects"]
    assert all(p["workspace_id"] != ws for p in projects)


def test_archive_default_workspace_rejected(client, test_engine):
    c, *_ = client
    default = next(
        w for w in c.get("/api/v1/workspaces/", headers=_master_headers()).json()["workspaces"]
        if w["slug"] == "default"
    )
    r = c.delete(f"/api/v1/workspaces/{default['id']}", headers=_master_headers())
    assert r.status_code == 400


def test_update_workspace_name_and_description(client, test_engine):
    c, *_ = client
    ws = _create_workspace(c, "Before", "before-edit")
    r = c.patch(
        f"/api/v1/workspaces/{ws}",
        json={"name": "After", "description": "updated note"},
        headers=_master_headers(),
    )
    assert r.status_code == 200, r.text
    body = r.json()
    assert body["name"] == "After"
    assert body["description"] == "updated note"
    assert body["slug"] == "before-edit"


def test_assistant_isolated_by_workspace(client, test_engine):
    c, *_ = client
    ws_a = _create_workspace(c, "A", "a")
    ws_b = _create_workspace(c, "B", "b")
    proj_b = _create_project(c, ws_b, "pb")
    token_a = _create_token(c, ws_a)
    h = {"Authorization": f"Bearer {token_a}"}

    assert c.get(f"/api/v1/assistant/dashboard?project_id={proj_b}", headers=h).status_code == 404


def test_action_sets_isolated_by_workspace(client, test_engine):
    """Action sets carry only a client_id; a workspace token must not read across it."""
    c, *_ = client
    ws_a = _create_workspace(c, "A", "a")
    ws_b = _create_workspace(c, "B", "b")
    ha = {"Authorization": f"Bearer {_create_token(c, ws_a)}"}
    hb = {"Authorization": f"Bearer {_create_token(c, ws_b)}"}

    set_b = c.post("/api/v1/action-sets", json={"name": "b-secret", "actions": []}, headers=hb)
    assert set_b.status_code == 200, set_b.text
    set_b_id = set_b.json()["id"]

    assert c.get(f"/api/v1/action-sets/{set_b_id}", headers=hb).status_code == 200
    assert c.get(f"/api/v1/action-sets/{set_b_id}", headers=ha).status_code == 403
    assert c.delete(f"/api/v1/action-sets/{set_b_id}", headers=ha).status_code == 403

    names_a = {s["name"] for s in c.get("/api/v1/action-sets", headers=ha).json()["action_sets"]}
    assert "b-secret" not in names_a
    # Seeded sets have no owner and stay shared with every tenant.
    assert "todo-board-ops" in names_a


def test_workspace_token_cannot_reach_server_config(client, test_engine):
    """A tenant credential must not read or rewrite the server's own .env/config.yaml."""
    c, *_ = client
    ws_a = _create_workspace(c, "A", "a")
    h = {"Authorization": f"Bearer {_create_token(c, ws_a)}"}

    assert c.get("/api/v1/llm-proxy/env", headers=h).status_code == 403
    assert c.get("/api/v1/llm-proxy/snippet", headers=h).status_code == 403
    assert c.get("/api/v1/llm-proxy/config-yaml", headers=h).status_code == 403
    assert (
        c.post("/api/v1/llm-proxy/env", json={"DEFAULT_MODEL": "x"}, headers=h).status_code == 403
    )
    assert (
        c.post(
            "/api/v1/llm-proxy/config-yaml", json={"content": "version: 1\n"}, headers=h
        ).status_code
        == 403
    )
    # The model catalog stays reachable so a tenant UI can still render its picker.
    assert c.get("/api/v1/llm-proxy/ui/providers", headers=h).status_code == 200


def test_assistant_id_addressed_writes_isolated_by_workspace(client, test_engine):
    """Routes taking a bare item/review id must resolve the owner, not trust the id."""
    c, *_ = client
    ws_a = _create_workspace(c, "A", "a")
    ws_b = _create_workspace(c, "B", "b")
    proj_b = _create_project(c, ws_b, "pb")
    board_b = c.post(
        f"/api/v1/todos/boards?project_id={proj_b}",
        json={"name": "BoardB"},
        headers=_master_headers(),
    ).json()["id"]
    item_b = c.post(
        f"/api/v1/todos/boards/{board_b}/items",
        json={"title": "secret"},
        headers=_master_headers(),
    ).json()["id"]
    token_a = _create_token(c, ws_a)
    h = {"Authorization": f"Bearer {token_a}"}

    assert (
        c.post(f"/api/v1/assistant/items/{item_b}/complete", json={}, headers=h).status_code == 404
    )
    # Review ids resolve through their project; a missing one must not leak either.
    assert (
        c.post(f"/api/v1/assistant/reviews/1/apply", json={"actions": []}, headers=h).status_code
        == 404
    )
    assert c.post(f"/api/v1/assistant/reviews/1/dismiss", headers=h).status_code == 404


def test_todos_board_isolated_by_workspace(client, test_engine):
    c, *_ = client
    ws_a = _create_workspace(c, "A", "a")
    ws_b = _create_workspace(c, "B", "b")
    proj_b = _create_project(c, ws_b, "pb")
    board_b = c.post(
        f"/api/v1/todos/boards?project_id={proj_b}",
        json={"name": "BoardB"},
        headers=_master_headers(),
    ).json()["id"]
    token_a = _create_token(c, ws_a)
    h = {"Authorization": f"Bearer {token_a}"}

    assert c.get(f"/api/v1/todos/boards/{board_b}", headers=h).status_code == 404


def test_archived_workspace_project_hidden_from_master_key(client, test_engine):
    c, *_ = client
    ws = _create_workspace(c, "ArchiveProj", "archive-proj")
    proj = _create_project(c, ws, "hidden")
    c.delete(f"/api/v1/workspaces/{ws}", headers=_master_headers())

    assert c.get(f"/api/v1/projects/{proj}", headers=_master_headers()).status_code == 404
    assert (
        c.patch(f"/api/v1/projects/{proj}", json={"name": "x"}, headers=_master_headers()).status_code
        == 404
    )
