"""Upstream rate-limit retry: many-agents bursts get retried with backoff, not surfaced raw."""

from __future__ import annotations

import asyncio

import httpx

from llm_proxy.services.upstream_http import UpstreamHttpClient, is_rate_limited_response


def test_is_rate_limited_response_status_codes():
    assert is_rate_limited_response(429, None) is True
    assert is_rate_limited_response(503, None) is True
    assert is_rate_limited_response(200, None) is False


def test_is_rate_limited_response_body_phrase():
    assert is_rate_limited_response(400, "too many concurrent requests") is True
    assert is_rate_limited_response(400, "TOO MANY CONCURRENT REQUESTS") is True
    assert is_rate_limited_response(400, "model not found") is False
    assert is_rate_limited_response(200, "too many concurrent requests") is False  # only matters on error status


class _FakeResponse:
    def __init__(self, status_code, text=""):
        self.status_code = status_code
        self.text = text
        self.headers = {}


def test_post_retries_until_success_within_budget(monkeypatch):
    client = UpstreamHttpClient(rate_limit_max_retries=3, rate_limit_backoff_ms=1)

    calls = {"n": 0}

    class FakeAsyncClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *a):
            return None

        async def post(self, *a, **kw):
            calls["n"] += 1
            if calls["n"] < 3:
                return _FakeResponse(429, "too many concurrent requests")
            return _FakeResponse(200, "ok")

    monkeypatch.setattr(httpx, "AsyncClient", lambda **kw: FakeAsyncClient())

    resp = asyncio.run(client.post("http://upstream/x", context="test"))
    assert resp.status_code == 200
    assert calls["n"] == 3


def test_post_gives_up_after_budget_exhausted(monkeypatch):
    client = UpstreamHttpClient(rate_limit_max_retries=2, rate_limit_backoff_ms=1)

    calls = {"n": 0}

    class FakeAsyncClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *a):
            return None

        async def post(self, *a, **kw):
            calls["n"] += 1
            return _FakeResponse(429, "too many concurrent requests")

    monkeypatch.setattr(httpx, "AsyncClient", lambda **kw: FakeAsyncClient())

    resp = asyncio.run(client.post("http://upstream/x", context="test"))
    # Exhausts the rate_limit_max_retries=2 budget, then returns the last (still 429) response
    # rather than retrying forever — caller decides what to surface to the agent.
    assert resp.status_code == 429
    assert calls["n"] == 2
