"""Loopback connect refusals are not retried: a local app that is not running will
not start within a retry backoff, and every probe of it stalled the provider catalog."""

from __future__ import annotations

import httpx

from llm_proxy.services.upstream_http import should_retry_transport


def _refused(url: str) -> httpx.ConnectError:
    return httpx.ConnectError("connection refused", request=httpx.Request("GET", url))


def test_loopback_refusal_is_not_retried():
    # Ollama and LM Studio are both addressed over loopback.
    for url in (
        "http://127.0.0.1:11434/api/tags",
        "http://localhost:1234/v1/models",
        "http://[::1]:1234/v1/models",
    ):
        assert should_retry_transport(_refused(url), 0, 3) is False, url


def test_remote_refusal_still_retries():
    # A refusal from a real upstream can be a restarting LB, so it keeps its budget.
    assert should_retry_transport(_refused("https://api.aimlapi.com/v1/models"), 0, 3) is True


def test_loopback_timeout_still_retries():
    # Only refusals are hopeless; a slow local model server is worth waiting on.
    timeout = httpx.ReadTimeout("slow", request=httpx.Request("GET", "http://127.0.0.1:11434/api/tags"))
    assert should_retry_transport(timeout, 0, 3) is True


def test_last_attempt_never_retries():
    assert should_retry_transport(_refused("https://api.aimlapi.com/v1/models"), 2, 3) is False
