"""Tests for the standalone chat Playground API (project-less)."""

import json

from sqlmodel import Session, select

from playground.models import PlaygroundChatThread


def _fake_llm_client(monkeypatch, reply: str = "hi there", usage: dict | None = None):
    body: dict = {"choices": [{"message": {"content": reply}}]}
    if usage is not None:
        body["usage"] = usage

    class FakeResp:
        status_code = 200
        text = json.dumps(body)

        def json(self):
            return json.loads(self.text)

    class FakeClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *args):
            return None

        async def post(self, url, headers=None, json=None):
            assert "/chat/completions" in url
            return FakeResp()

    monkeypatch.setattr("playground.service.httpx.AsyncClient", lambda *a, **k: FakeClient())


def _fake_llm_stream(monkeypatch, tokens: list[str], usage: dict | None = None):
    """Patch httpx.AsyncClient so playground.service streams OpenAI-style SSE chunks."""

    lines: list[str] = []
    for tok in tokens:
        chunk = {"choices": [{"delta": {"content": tok}}]}
        lines.append(f"data: {json.dumps(chunk)}")
    if usage is not None:
        lines.append(f"data: {json.dumps({'choices': [], 'usage': usage})}")
    lines.append("data: [DONE]")

    class FakeStreamCtx:
        status_code = 200

        async def __aenter__(self):
            return self

        async def __aexit__(self, *args):
            return None

        async def aread(self):
            return b""

        async def aiter_lines(self):
            for line in lines:
                yield line

    class FakeClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *args):
            return None

        def stream(self, method, url, headers=None, json=None):
            assert "/chat/completions" in url
            assert json.get("stream") is True
            return FakeStreamCtx()

    monkeypatch.setattr("playground.service.httpx.AsyncClient", lambda *a, **k: FakeClient())


def _parse_sse(text: str) -> list[tuple[str, dict]]:
    events: list[tuple[str, dict]] = []
    for block in text.strip().split("\n\n"):
        event = ""
        data = ""
        for line in block.splitlines():
            if line.startswith("event:"):
                event = line[len("event:") :].strip()
            elif line.startswith("data:"):
                data = line[len("data:") :].strip()
        if event:
            events.append((event, json.loads(data) if data else {}))
    return events


def test_playground_stream_emits_deltas_and_persists(client, monkeypatch, test_engine):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "test-key")
    auth = {"Authorization": "Bearer test-key"}
    usage = {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}
    _fake_llm_stream(monkeypatch, tokens=["Hel", "lo", "!"], usage=usage)

    created = c.post("/api/v1/playground/chat/threads", json={}, headers=auth)
    thread_id = created.json()["thread_id"]

    r = c.post(
        "/api/v1/playground/chat/stream",
        json={"message": "Say hi", "thread_id": thread_id, "model": "test-model"},
        headers=auth,
    )
    assert r.status_code == 200
    assert r.headers["content-type"].startswith("text/event-stream")

    events = _parse_sse(r.text)
    deltas = [e[1]["content"] for e in events if e[0] == "delta"]
    assert "".join(deltas) == "Hello!"
    done = [e[1] for e in events if e[0] == "done"]
    assert len(done) == 1
    assert done[0]["thread_id"] == thread_id
    assert done[0]["title"] == "Say hi"
    assert done[0]["context_window"] > 0
    assert [m["role"] for m in done[0]["messages"]] == ["user", "assistant"]
    assert done[0]["messages"][1]["content"] == "Hello!"
    assert done[0]["messages"][1]["usage"] == usage
    assert done[0]["usage"]["total_tokens"] == 8
    assert done[0]["context_usage"]["categories"]["system_prompt"] > 0

    # Reload returns the same persisted data (survives connection loss / refresh).
    fetched = c.get(f"/api/v1/playground/chat/thread?thread_id={thread_id}", headers=auth)
    fetched_body = fetched.json()
    assert [m["content"] for m in fetched_body["messages"]] == ["Say hi", "Hello!"]
    assert fetched_body["model"] == "test-model"

    with Session(test_engine) as session:
        rows = session.exec(select(PlaygroundChatThread)).all()
        assert len(rows) == 1
        assert rows[0].get_messages()[1]["content"] == "Hello!"


def test_playground_stream_requires_master_key(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.delenv("AGENT_PLATFORM_MASTER_KEY", raising=False)

    r = c.post("/api/v1/playground/chat/stream", json={"message": "hi"})
    assert r.status_code == 503


def test_playground_thread_created_lazily_on_get(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "test-key")
    auth = {"Authorization": "Bearer test-key"}

    r = c.get("/api/v1/playground/chat/thread", headers=auth)
    assert r.status_code == 200
    data = r.json()
    assert data["thread_id"] > 0
    assert data["messages"] == []


def test_playground_send_persists_and_lists(client, monkeypatch, test_engine):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "test-key")
    auth = {"Authorization": "Bearer test-key"}
    usage = {"prompt_tokens": 12, "completion_tokens": 4, "total_tokens": 16}
    _fake_llm_client(monkeypatch, reply="Hello, world!", usage=usage)

    created = c.post("/api/v1/playground/chat/threads", json={}, headers=auth)
    assert created.status_code == 200
    thread_id = created.json()["thread_id"]

    r = c.post(
        "/api/v1/playground/chat/send",
        json={
            "message": "Say hi",
            "thread_id": thread_id,
            "model": "test-model",
            "temperature": 0.5,
            "top_p": 0.9,
            "max_tokens": 256,
        },
        headers=auth,
    )
    assert r.status_code == 200
    body = r.json()
    assert body["thread_id"] == thread_id
    assert body["title"] == "Say hi"
    assert body["context_window"] > 0
    assert [m["role"] for m in body["messages"]] == ["user", "assistant"]
    assert body["messages"][1]["content"] == "Hello, world!"
    assert body["messages"][1]["usage"] == usage
    assert body["usage"]["total_tokens"] == 16
    assert body["context_usage"]["categories"]["conversation"] > 0

    fetched = c.get(f"/api/v1/playground/chat/thread?thread_id={thread_id}", headers=auth)
    assert fetched.status_code == 200
    fetched_body = fetched.json()
    assert len(fetched_body["messages"]) == 2
    assert fetched_body["model"] == "test-model"

    listed = c.get("/api/v1/playground/chat/threads", headers=auth)
    assert listed.status_code == 200
    threads = listed.json()["threads"]
    assert any(t["id"] == thread_id and t["message_count"] == 2 for t in threads)

    with Session(test_engine) as session:
        rows = session.exec(select(PlaygroundChatThread)).all()
        assert len(rows) == 1
        assert rows[0].id == thread_id
        assert rows[0].model == "test-model"


def test_playground_thread_delete(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "test-key")
    auth = {"Authorization": "Bearer test-key"}

    created = c.post("/api/v1/playground/chat/threads", json={}, headers=auth)
    thread_id = created.json()["thread_id"]

    r = c.delete(f"/api/v1/playground/chat/thread/{thread_id}", headers=auth)
    assert r.status_code == 200
    assert r.json() == {"thread_id": thread_id, "deleted": True}

    r2 = c.get(f"/api/v1/playground/chat/thread?thread_id={thread_id}", headers=auth)
    assert r2.status_code == 404


def test_playground_send_requires_master_key(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.delenv("AGENT_PLATFORM_MASTER_KEY", raising=False)

    r = c.post("/api/v1/playground/chat/send", json={"message": "hi"})
    assert r.status_code == 503


def test_playground_send_llm_failure_keeps_user_message(client, monkeypatch, test_engine):
    """Upstream failure must not lose the user's message (matches stream behavior)."""
    import httpx

    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "test-key")
    auth = {"Authorization": "Bearer test-key"}

    class FailClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *args):
            return None

        async def post(self, url, headers=None, json=None):
            raise httpx.ConnectError("connection refused")

    monkeypatch.setattr("playground.service.httpx.AsyncClient", lambda *a, **k: FailClient())

    created = c.post("/api/v1/playground/chat/threads", json={}, headers=auth)
    thread_id = created.json()["thread_id"]

    r = c.post(
        "/api/v1/playground/chat/send",
        json={"message": "Say hi", "thread_id": thread_id},
        headers=auth,
    )
    assert r.status_code == 502

    fetched = c.get(f"/api/v1/playground/chat/thread?thread_id={thread_id}", headers=auth)
    msgs = fetched.json()["messages"]
    assert [m["role"] for m in msgs] == ["user"]
    assert msgs[0]["content"] == "Say hi"
    assert fetched.json()["title"] == "Say hi"
