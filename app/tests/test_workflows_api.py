"""Workflows: CRUD, run with templates, failure path, action steps, scheduler."""

import asyncio

import pytest

import workflows.engine as wf_engine
from workflows.engine import StepError, resolve_templates


# === Template resolver (pure) ===

def test_resolve_templates():
    ctx = {
        "trigger": {"body": {"name": "ada", "n": 2}},
        "steps": {"fetch": {"output": {"status": 200, "body": {"items": [10, 20]}}}},
    }
    # whole-string template keeps type
    assert resolve_templates("{{trigger.body.n}}", ctx) == 2
    assert resolve_templates("{{steps.fetch.output.body.items.1}}", ctx) == 20
    # embedded template stringifies
    assert resolve_templates("hello {{trigger.body.name}}!", ctx) == "hello ada!"
    # nested structures
    resolved = resolve_templates({"a": ["{{trigger.body.n}}"], "b": 1}, ctx)
    assert resolved == {"a": [2], "b": 1}
    # missing path raises
    with pytest.raises(StepError):
        resolve_templates("{{steps.nope.output}}", ctx)


# === Helpers ===

def _fake_http(outputs: dict[str, dict], calls: list):
    """Patch _execute_http; route responses by URL."""

    async def fake(params):
        calls.append(params)
        out = outputs.get(params["url"])
        if out is None:
            raise StepError(f"http request failed: no fake for {params['url']}")
        return out

    return fake


def _create_workflow(c, steps, **kwargs):
    resp = c.post("/api/v1/workflows", json={"name": "wf", "steps": steps, **kwargs})
    assert resp.status_code == 200, resp.text
    return resp.json()


# === API ===

def test_workflow_crud_and_validation(client):
    c, _, _ = client
    wf = _create_workflow(c, [{"id": "a", "type": "http", "params": {"url": "http://x/1"}}])
    assert wf["enabled"] is True
    assert wf["steps"][0]["id"] == "a"

    # duplicate step ids rejected
    resp = c.post("/api/v1/workflows", json={
        "name": "bad",
        "steps": [
            {"id": "a", "type": "http", "params": {"url": "http://x"}},
            {"id": "a", "type": "http", "params": {"url": "http://y"}},
        ],
    })
    assert resp.status_code == 400
    assert "duplicate" in resp.text

    # http step without url rejected by schema
    resp = c.post("/api/v1/workflows", json={
        "name": "bad", "steps": [{"id": "a", "type": "http", "params": {}}],
    })
    assert resp.status_code == 422

    # update + list + delete
    resp = c.put(f"/api/v1/workflows/{wf['id']}", json={"enabled": False})
    assert resp.json()["enabled"] is False
    assert len(c.get("/api/v1/workflows").json()["workflows"]) == 1
    assert c.delete(f"/api/v1/workflows/{wf['id']}").json()["success"] is True
    assert c.get(f"/api/v1/workflows/{wf['id']}").status_code == 404


def test_run_workflow_with_templates(client, monkeypatch):
    c, _, _ = client
    calls = []
    monkeypatch.setattr(wf_engine, "_execute_http", _fake_http({
        "http://api/users/ada": {"status": 200, "body": {"email": "ada@x.io"}},
        "http://notify": {"status": 200, "body": "ok"},
    }, calls))

    wf = _create_workflow(c, [
        {"id": "fetch", "type": "http",
         "params": {"url": "http://api/users/{{trigger.body.user}}"}},
        {"id": "notify", "type": "http",
         "params": {"url": "http://notify", "method": "POST",
                    "body": {"to": "{{steps.fetch.output.body.email}}"}}},
    ])

    resp = c.post(f"/api/v1/workflows/{wf['id']}/run", json={"user": "ada"})
    assert resp.status_code == 200, resp.text
    run = resp.json()
    assert run["status"] == "succeeded"
    assert run["trigger"] == "api"
    assert [s["status"] for s in run["steps"]] == ["succeeded", "succeeded"]
    # templates resolved from trigger body and prior step output
    assert calls[0]["url"] == "http://api/users/ada"
    assert calls[1]["body"] == {"to": "ada@x.io"}

    # run recorded and fetchable
    runs = c.get(f"/api/v1/workflows/{wf['id']}/runs").json()["runs"]
    assert len(runs) == 1
    assert c.get(f"/api/v1/workflows/{wf['id']}/runs/{runs[0]['id']}").json()["status"] == "succeeded"


def test_run_stops_on_failure(client, monkeypatch):
    c, _, _ = client
    monkeypatch.setattr(wf_engine, "_execute_http", _fake_http({}, []))
    wf = _create_workflow(c, [
        {"id": "boom", "type": "http", "params": {"url": "http://down"}},
        {"id": "after", "type": "http", "params": {"url": "http://never"}},
    ])
    run = c.post(f"/api/v1/workflows/{wf['id']}/run").json()
    assert run["status"] == "failed"
    assert "boom" in run["error"]
    by_id = {s["id"]: s["status"] for s in run["steps"]}
    assert by_id == {"boom": "failed", "after": "skipped"}


def test_action_step_requires_server_mode(client, monkeypatch):
    c, _, _ = client
    resp = c.post("/api/v1/action-sets", json={
        "name": "ops",
        "actions": [
            {"action_id": "ping", "name": "Ping", "description": "d",
             "execution_mode": "server", "endpoint": "http://svc/ping"},
            {"action_id": "local", "name": "Local", "description": "d",
             "execution_mode": "client"},
        ],
    })
    assert resp.status_code == 200, resp.text
    set_id = resp.json()["id"]

    calls = []
    monkeypatch.setattr(wf_engine, "_execute_http", _fake_http({
        "http://svc/ping": {"status": 200, "body": {"pong": True}},
    }, calls))

    wf = _create_workflow(c, [
        {"id": "s1", "type": "action",
         "params": {"action_set_id": set_id, "action_id": "ping", "arguments": {"a": 1}}},
    ])
    run = c.post(f"/api/v1/workflows/{wf['id']}/run").json()
    assert run["status"] == "succeeded", run
    assert calls[0] == {"url": "http://svc/ping", "method": "POST", "body": {"a": 1}}

    # client-mode action cannot run server-side
    wf2 = _create_workflow(c, [
        {"id": "s1", "type": "action",
         "params": {"action_set_id": set_id, "action_id": "local"}},
    ])
    run2 = c.post(f"/api/v1/workflows/{wf2['id']}/run").json()
    assert run2["status"] == "failed"
    assert "not server-executable" in run2["error"]


def test_disabled_workflow_rejects_run(client):
    c, _, _ = client
    wf = _create_workflow(
        c, [{"id": "a", "type": "http", "params": {"url": "http://x"}}], enabled=False
    )
    assert c.post(f"/api/v1/workflows/{wf['id']}/run").status_code == 400


def test_run_real_http_roundtrip(client):
    """No mocks: the engine's httpx path against a live local server, with a
    template flowing from step 1's real response into step 2's request."""
    import http.server
    import json as jsonlib
    import threading

    received = {}

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            body = jsonlib.dumps({"token": "t-123"}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(body)

        def do_POST(self):
            length = int(self.headers.get("Content-Length", 0))
            received["post"] = jsonlib.loads(self.rfile.read(length))
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"{}")

        def log_message(self, *args):
            pass

    server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_port}"
    try:
        c, _, _ = client
        wf = _create_workflow(c, [
            {"id": "get", "type": "http", "params": {"url": f"{base}/data"}},
            {"id": "post", "type": "http",
             "params": {"url": f"{base}/submit", "method": "POST",
                        "body": {"auth": "{{steps.get.output.body.token}}"}}},
        ])
        run = c.post(f"/api/v1/workflows/{wf['id']}/run", json={}).json()
        assert run["status"] == "succeeded", run
        assert run["steps"][0]["output"]["body"] == {"token": "t-123"}
        assert received["post"] == {"auth": "t-123"}
    finally:
        server.shutdown()


def test_scheduler_runs_due_workflows(client, monkeypatch):
    c, _, _ = client
    monkeypatch.setattr(wf_engine, "_execute_http", _fake_http({
        "http://x": {"status": 200, "body": "ok"},
    }, []))
    wf = _create_workflow(
        c, [{"id": "a", "type": "http", "params": {"url": "http://x"}}], interval_seconds=3600
    )
    # force due now
    c.put(f"/api/v1/workflows/{wf['id']}", json={"interval_seconds": 60})
    import database
    from sqlmodel import Session as DbSession
    from time_utils import utc_now_naive
    from workflows.models import Workflow
    from datetime import timedelta
    with DbSession(database.engine) as s:
        row = s.get(Workflow, wf["id"])
        row.next_run_at = utc_now_naive() - timedelta(seconds=1)
        s.add(row)
        s.commit()

    asyncio.run(wf_engine._run_due_workflows())

    runs = c.get(f"/api/v1/workflows/{wf['id']}/runs").json()["runs"]
    assert len(runs) == 1
    assert runs[0]["trigger"] == "schedule"
    assert runs[0]["status"] == "succeeded"
    # next_run_at advanced into the future
    detail = c.get(f"/api/v1/workflows/{wf['id']}").json()
    assert detail["next_run_at"] > utc_now_naive().isoformat()
