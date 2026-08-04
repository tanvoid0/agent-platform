"""Workflows: CRUD, run with templates, failure path, action steps, scheduler."""

import asyncio

import pytest

import workflows.engine as wf_engine
from workflows.engine import StepError, resolve_templates


# === AI assist ===

def _mock_llm(monkeypatch, content: str):
    async def fake(messages, **kwargs):
        _mock_llm.messages = messages
        return content, 10, 0.0

    import workflows.assist as assist_mod
    monkeypatch.setattr(assist_mod, "call_llm", fake)


def test_assist_generates_steps(client, monkeypatch):
    c, _, _ = client
    _mock_llm(monkeypatch, """{"reply": "Added a health check.",
        "steps": [{"id": "ping", "type": "http", "params": {"url": "http://x/health"}}]}""")
    resp = c.post("/api/v1/workflows/assist", json={"message": "check health of x"})
    assert resp.status_code == 200, resp.text
    data = resp.json()
    assert data["reply"] == "Added a health check."
    assert data["steps"][0]["id"] == "ping"
    # the draft context reached the model
    assert "check health of x" in _mock_llm.messages[1]["content"]


def test_assist_review_without_changes_and_invalid_steps(client, monkeypatch):
    c, _, _ = client
    # review verdict, no steps
    _mock_llm(monkeypatch, '{"reply": "Looks fine.", "steps": null}')
    data = c.post("/api/v1/workflows/assist", json={
        "message": "review this",
        "steps": [{"id": "a", "type": "http", "params": {"url": "http://x"}}],
    }).json()
    assert data["reply"] == "Looks fine."
    assert data["steps"] is None

    # invalid model output is neutered, not 500ed
    _mock_llm(monkeypatch, '{"reply": "Here.", "steps": [{"id": "BAD ID", "type": "http"}]}')
    data = c.post("/api/v1/workflows/assist", json={"message": "x"}).json()
    assert data["steps"] is None
    assert "discarded" in data["reply"]

    # non-JSON output becomes a plain reply
    _mock_llm(monkeypatch, "Sorry, I can only help with workflows.")
    data = c.post("/api/v1/workflows/assist", json={"message": "x"}).json()
    assert data["steps"] is None
    assert "Sorry" in data["reply"]


def test_assist_strips_markdown_fences(client, monkeypatch):
    c, _, _ = client
    _mock_llm(monkeypatch, """```json
{"reply": "ok", "steps": [{"id": "a", "type": "http", "params": {"url": "http://x"}}]}
```""")
    data = c.post("/api/v1/workflows/assist", json={"message": "x"}).json()
    assert data["steps"] is not None


def test_assist_strips_reasoning_think_block(client, monkeypatch):
    c, _, _ = client
    _mock_llm(monkeypatch, """<think>User wants a health check. One http step.</think>
{"reply": "ok", "steps": [{"id": "a", "type": "http", "params": {"url": "http://x"}}]}""")
    data = c.post("/api/v1/workflows/assist", json={"message": "x"}).json()
    assert data["steps"] is not None
    assert "think" not in data["reply"]


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


def test_complex_chain_templates_headers_and_mixed_steps(client, monkeypatch):
    """Five steps: http → template into url/header/body, an action step fed from
    an earlier output, list indexing, and an embedded-template string."""
    c, _, _ = client
    resp = c.post("/api/v1/action-sets", json={
        "name": "notify",
        "actions": [{"action_id": "alert", "name": "Alert", "description": "d",
                     "execution_mode": "server", "endpoint": "http://svc/alert"}],
    })
    set_id = resp.json()["id"]

    calls = []
    monkeypatch.setattr(wf_engine, "_execute_http", _fake_http({
        "http://api/users": {"status": 200, "body": {"users": [
            {"name": "ada", "token": "tk-1"}, {"name": "bob", "token": "tk-2"}]}},
        "http://api/users/bob/orders": {"status": 200, "body": {"orders": [{"total": 42}]}},
        "http://audit/log": {"status": 200, "body": "ok"},
        "http://svc/alert": {"status": 200, "body": {"sent": True}},
    }, calls))

    wf = _create_workflow(c, [
        {"id": "users", "type": "http", "params": {"url": "http://api/users"}},
        # list indexing + embedded template building a url
        {"id": "orders", "type": "http",
         "params": {"url": "http://api/users/{{steps.users.output.body.users.1.name}}/orders",
                    "headers": {"Authorization": "Bearer {{steps.users.output.body.users.1.token}}"}}},
        # whole-value template keeps the number type
        {"id": "audit", "type": "http",
         "params": {"url": "http://audit/log", "method": "POST",
                    "body": {"user": "{{trigger.body.requested_by}}",
                             "total": "{{steps.orders.output.body.orders.0.total}}"}}},
        # action step consuming an earlier step's output
        {"id": "notify", "type": "action",
         "params": {"action_set_id": set_id, "action_id": "alert",
                    "arguments": {"text": "order total {{steps.orders.output.body.orders.0.total}}"}}},
    ])
    run = c.post(f"/api/v1/workflows/{wf['id']}/run", json={"requested_by": "cli"}).json()
    assert run["status"] == "succeeded", run

    by_url = {call["url"]: call for call in calls}
    assert by_url["http://api/users/bob/orders"]["headers"] == {"Authorization": "Bearer tk-2"}
    assert by_url["http://audit/log"]["body"] == {"user": "cli", "total": 42}  # int preserved
    assert by_url["http://svc/alert"]["body"] == {"text": "order total 42"}


def test_missing_template_path_fails_that_step(client, monkeypatch):
    c, _, _ = client
    monkeypatch.setattr(wf_engine, "_execute_http", _fake_http({
        "http://ok": {"status": 200, "body": {}},
    }, []))
    wf = _create_workflow(c, [
        {"id": "first", "type": "http", "params": {"url": "http://ok"}},
        {"id": "bad", "type": "http",
         "params": {"url": "http://ok", "body": {"x": "{{steps.first.output.body.nope}}"}}},
    ])
    run = c.post(f"/api/v1/workflows/{wf['id']}/run").json()
    assert run["status"] == "failed"
    assert "template path not found" in run["error"]
    assert {s["id"]: s["status"] for s in run["steps"]} == {"first": "succeeded", "bad": "failed"}


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
