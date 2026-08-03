"""POST /api/v1/chat with stream=true passes the proxy's SSE body through.

The bug this guards: the route used to call `.json()` on every response, so a
streaming request came back as one `{"raw": "<the entire SSE dump>"}` blob after
the full completion — strictly worse than not supporting streaming at all.
"""

import pytest

import chat_routes

pytestmark = pytest.mark.contract

FRAMES = [
    b'data: {"choices":[{"delta":{"role":"assistant"}}]}\n\n',
    b'data: {"choices":[{"delta":{"content":"Hello"}}]}\n\n',
    b'data: {"choices":[{"delta":{"content":" there"}}]}\n\n',
    b"data: [DONE]\n\n",
]


class _FakeStream:
    """The subset of httpx.Response that `_stream_completion` touches."""

    def __init__(self, status_code=200, chunks=(), body=b""):
        self.status_code = status_code
        self.headers = {"content-type": "text/event-stream"}
        self._chunks = list(chunks)
        self._body = body

    async def aiter_bytes(self):
        for c in self._chunks:
            yield c

    async def aread(self):
        return self._body

    async def aclose(self):
        pass


def _patch_upstream(monkeypatch, response):
    import llm_proxy.services.upstream_http as upstream

    captured = {}

    async def fake_open_stream(url, *, headers=None, json_body=None, **kwargs):
        captured["url"] = url
        captured["body"] = json_body
        return response, None

    monkeypatch.setattr(upstream, "stream_chat_completion", fake_open_stream)
    return captured


def test_stream_true_returns_sse_frames_not_a_json_blob(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "k")
    captured = _patch_upstream(monkeypatch, _FakeStream(chunks=FRAMES))

    r = c.post(
        "/api/v1/chat",
        json={"messages": [{"role": "user", "content": "hi"}], "stream": True},
        headers={"Authorization": "Bearer k"},
    )

    assert r.status_code == 200, r.text
    assert r.headers["content-type"].startswith("text/event-stream")
    assert r.content == b"".join(FRAMES)
    # The flag has to reach the upstream, or it answers with a single JSON body.
    assert captured["body"]["stream"] is True


def test_stream_error_status_comes_back_as_the_upstream_body(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "k")
    _patch_upstream(
        monkeypatch,
        _FakeStream(status_code=429, body=b'{"error":{"message":"slow down"}}'),
    )

    r = c.post(
        "/api/v1/chat",
        json={"messages": [{"role": "user", "content": "hi"}], "stream": True},
        headers={"Authorization": "Bearer k"},
    )

    assert r.status_code == 429
    assert b"slow down" in r.content


def test_the_concurrency_slot_is_released_after_a_stream(client, monkeypatch):
    """A leaked semaphore permit would wedge the route after N streams."""
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "k")
    _patch_upstream(monkeypatch, _FakeStream(chunks=FRAMES))
    before = chat_routes._llm_semaphore._value

    for _ in range(3):
        r = c.post(
            "/api/v1/chat",
            json={"messages": [{"role": "user", "content": "hi"}], "stream": True},
            headers={"Authorization": "Bearer k"},
        )
        assert r.status_code == 200

    assert chat_routes._llm_semaphore._value == before
