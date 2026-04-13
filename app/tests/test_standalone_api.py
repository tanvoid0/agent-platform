"""Versioned API, chat proxy, optional API key, and client_id scope."""

import json


def test_api_v1_processes_mirrors_legacy_post(client):
    c, _mock_cls, _mock_inst = client
    tr = c.get("/teams/")
    assert tr.status_code == 200
    tid = tr.json()["teams"][0]["id"]
    body = {"goal": "Test goal", "auto_approve": False, "team_template_id": tid}
    r1 = c.post("/processes", json=body)
    r2 = c.post("/api/v1/processes", json=body)
    assert r1.status_code == 200
    assert r2.status_code == 200
    j1, j2 = r1.json(), r2.json()
    assert set(j1.keys()) == set(j2.keys()) == {"process_id", "status"}
    assert j1["status"] == j2["status"]
    assert isinstance(j1["process_id"], int) and isinstance(j2["process_id"], int)


def test_api_v1_teams_list_matches_legacy(client):
    c, _mock_cls, _mock_inst = client
    r1 = c.get("/teams/")
    r2 = c.get("/api/v1/teams/")
    assert r1.status_code == 200
    assert r2.status_code == 200
    assert r1.json() == r2.json()


def test_chat_completions_proxies_upstream(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("ORCHESTRATOR_MASTER_KEY", "test-orch-key")

    class FakeResp:
        status_code = 200
        text = '{"choices":[{"message":{"content":"hi"}}]}'

        def json(self):
            return json.loads(self.text)

    class FakeClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *args):
            return None

        async def post(self, url, headers=None, json=None):
            assert "/chat/completions" in url
            assert headers and "Bearer test-orch-key" in headers.get("Authorization", "")
            assert json["messages"][0]["role"] == "user"
            return FakeResp()

    monkeypatch.setattr("chat_routes.httpx.AsyncClient", lambda *a, **k: FakeClient())

    r = c.post(
        "/api/v1/chat",
        json={"messages": [{"role": "user", "content": "Hello"}]},
    )
    assert r.status_code == 200
    data = r.json()
    assert data["choices"][0]["message"]["content"] == "hi"


def test_orchestrator_ready_proxies_upstream(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("ORCHESTRATOR_MASTER_KEY", "test-orch-key")

    class FakeResp:
        status_code = 200

    class FakeClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *args):
            return None

        async def get(self, url, headers=None):
            assert "/v1/models" in url
            assert headers and "Bearer test-orch-key" in headers.get("Authorization", "")
            return FakeResp()

    monkeypatch.setattr("chat_routes.httpx.AsyncClient", lambda *a, **k: FakeClient())

    r = c.get("/api/v1/orchestrator/ready")
    assert r.status_code == 200
    assert r.json() == {"ok": True}


def test_orchestrator_ready_requires_orchestrator_key(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.delenv("ORCHESTRATOR_MASTER_KEY", raising=False)

    r = c.get("/api/v1/orchestrator/ready")
    assert r.status_code == 503


def test_chat_requires_orchestrator_key(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.delenv("ORCHESTRATOR_MASTER_KEY", raising=False)

    r = c.post(
        "/api/v1/chat",
        json={"messages": [{"role": "user", "content": "x"}]},
    )
    assert r.status_code == 503


def test_api_key_required_when_set(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_API_KEY", "secret-apk")

    r = c.get("/processes")
    assert r.status_code == 401

    r2 = c.get("/processes", headers={"Authorization": "Bearer wrong"})
    assert r2.status_code == 401

    r3 = c.get("/processes", headers={"Authorization": "Bearer secret-apk"})
    assert r3.status_code == 200


def test_client_id_scope_list_and_access(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    tr = c.get("/teams/")
    tid = tr.json()["teams"][0]["id"]
    r = c.post(
        "/api/v1/processes",
        json={"goal": "Scoped", "client_id": "tenant-a", "team_template_id": tid},
    )
    assert r.status_code == 200
    pid = r.json()["process_id"]

    r_all = c.get("/api/v1/processes")
    assert r_all.status_code == 200
    ids_all = {p["id"] for p in r_all.json()["processes"]}
    assert pid in ids_all

    r_f = c.get("/api/v1/processes", params={"client_id": "tenant-a"})
    assert r_f.status_code == 200
    ids_f = {p["id"] for p in r_f.json()["processes"]}
    assert ids_f == {pid}

    r_no = c.get(f"/api/v1/processes/{pid}")
    assert r_no.status_code == 404

    r_ok = c.get(
        f"/api/v1/processes/{pid}",
        headers={"X-Agent-Platform-Client": "tenant-a"},
    )
    assert r_ok.status_code == 200


def test_require_client_id_env(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_REQUIRE_CLIENT_ID", "1")

    tr = c.get("/teams/")
    tid = tr.json()["teams"][0]["id"]
    r = c.post("/api/v1/processes", json={"goal": "No client", "team_template_id": tid})
    assert r.status_code == 400
    r2 = c.post(
        "/api/v1/processes",
        json={"goal": "With client", "client_id": "x", "team_template_id": tid},
    )
    assert r2.status_code == 200


def test_api_guide_page(client):
    c, _mock_cls, _mock_inst = client
    r = c.get("/api-guide")
    assert r.status_code == 200
    assert b"/api/v1/chat" in r.content
    assert b"Agent Platform HTTP API" in r.content
