"""Tests for BYOK (bring-your-own-key) header parsing and route forwarding."""

import pytest

from llm_proxy.core import byok
from llm_proxy.core.byok import parse_byok
from llm_proxy.core.errors import LlmProxyError

pytestmark = pytest.mark.contract


class _FakeHeaders:
    """Case-insensitive header lookup, like starlette's Headers."""

    def __init__(self, mapping):
        self._m = {k.lower(): v for k, v in mapping.items()}

    def get(self, key, default=None):
        return self._m.get(key.lower(), default)


class _FakeRequest:
    def __init__(self, headers):
        self.headers = _FakeHeaders(headers)


def _req(headers):
    return _FakeRequest(headers)


# --- unit: parse_byok ------------------------------------------------------


def test_no_provider_header_returns_none():
    assert parse_byok(_req({})) is None


def test_unknown_provider_raises_400():
    with pytest.raises(LlmProxyError) as exc:
        parse_byok(_req({"X-BYOK-Provider": "nope"}))
    assert exc.value.status_code == 400
    assert exc.value.code == "byok_unknown_provider"


def test_missing_key_raises_400():
    with pytest.raises(LlmProxyError) as exc:
        parse_byok(_req({"X-BYOK-Provider": "openai"}))
    assert exc.value.status_code == 400
    assert exc.value.code == "byok_missing_key"


def test_canonical_base_and_bearer_header():
    route = parse_byok(_req({"X-BYOK-Provider": "openai", "X-BYOK-Api-Key": "sk-abc"}))
    assert route is not None
    assert route.base == "https://api.openai.com/v1"
    assert route.url("chat") == "https://api.openai.com/v1/chat/completions"
    headers = route.outbound_headers()
    assert headers["Authorization"] == "Bearer sk-abc"
    assert "x-api-key" not in headers


def test_anthropic_sends_native_headers():
    route = parse_byok(
        _req({"X-BYOK-Provider": "anthropic", "X-BYOK-Api-Key": "sk-ant"})
    )
    headers = route.outbound_headers()
    assert headers["x-api-key"] == "sk-ant"
    assert headers["anthropic-version"] == "2023-06-01"
    assert headers["Authorization"] == "Bearer sk-ant"


def test_anthropic_version_override():
    route = parse_byok(
        _req(
            {
                "X-BYOK-Provider": "anthropic",
                "X-BYOK-Api-Key": "sk-ant",
                "X-BYOK-Anthropic-Version": "2024-10-01",
            }
        )
    )
    assert route.outbound_headers()["anthropic-version"] == "2024-10-01"


def test_custom_base_url_allowlisted_ok():
    route = parse_byok(
        _req(
            {
                "X-BYOK-Provider": "openai",
                "X-BYOK-Api-Key": "sk-abc",
                "X-BYOK-Base-Url": "https://api.openai.com/proxy/v1",
            }
        )
    )
    assert route.base == "https://api.openai.com/proxy/v1"


def test_custom_base_url_disallowed_host_raises_403():
    with pytest.raises(LlmProxyError) as exc:
        parse_byok(
            _req(
                {
                    "X-BYOK-Provider": "openai",
                    "X-BYOK-Api-Key": "sk-abc",
                    "X-BYOK-Base-Url": "https://internal.corp.local/v1",
                }
            )
        )
    assert exc.value.status_code == 403
    assert exc.value.code == "byok_host_not_allowed"


def test_custom_base_url_raw_ip_rejected():
    with pytest.raises(LlmProxyError) as exc:
        parse_byok(
            _req(
                {
                    "X-BYOK-Provider": "openai",
                    "X-BYOK-Api-Key": "sk-abc",
                    "X-BYOK-Base-Url": "https://169.254.169.254/v1",
                }
            )
        )
    assert exc.value.status_code == 403


def test_custom_base_url_non_https_rejected():
    with pytest.raises(LlmProxyError) as exc:
        parse_byok(
            _req(
                {
                    "X-BYOK-Provider": "openai",
                    "X-BYOK-Api-Key": "sk-abc",
                    "X-BYOK-Base-Url": "http://api.openai.com/v1",
                }
            )
        )
    assert exc.value.status_code == 400
    assert exc.value.code == "byok_invalid_base_url"


def test_operator_allowlist_extends_hosts(monkeypatch):
    monkeypatch.setattr(byok, "_extra_allowed_hosts", lambda: frozenset({"gw.example.com"}))
    route = parse_byok(
        _req(
            {
                "X-BYOK-Provider": "openai",
                "X-BYOK-Api-Key": "sk-abc",
                "X-BYOK-Base-Url": "https://gw.example.com/v1",
            }
        )
    )
    assert route.base == "https://gw.example.com/v1"


def test_require_unsupported_capability_raises_501():
    route = parse_byok(
        _req({"X-BYOK-Provider": "anthropic", "X-BYOK-Api-Key": "sk-ant"})
    )
    with pytest.raises(LlmProxyError) as exc:
        route.require("embeddings")
    assert exc.value.status_code == 501
    assert exc.value.code == "capability_unavailable"


# --- route: /v1/chat/completions with BYOK ---------------------------------


def _byok_headers():
    return {"X-BYOK-Provider": "openai", "X-BYOK-Api-Key": "sk-client"}


def test_chat_completions_byok_forwards_with_client_key(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    captured = {}

    async def fake_post_with_retry(url, **kwargs):
        captured["url"] = url
        captured["headers"] = kwargs.get("headers")
        captured["body"] = kwargs.get("json_body")
        return type(
            "R",
            (),
            {
                "status_code": 200,
                "content": b'{"choices":[{"message":{"content":"hi"}}]}',
                "headers": {"content-type": "application/json"},
            },
        )()

    monkeypatch.setattr("llm_proxy.routes.llm.post_with_retry", fake_post_with_retry)

    r = c.post(
        "/v1/chat/completions",
        json={"model": "gpt-4o-mini", "messages": [{"role": "user", "content": "hi"}]},
        headers=_byok_headers(),
    )
    assert r.status_code == 200
    assert captured["url"] == "https://api.openai.com/v1/chat/completions"
    assert captured["headers"]["Authorization"] == "Bearer sk-client"
    # Model passed through untouched, provider hint stripped.
    assert captured["body"]["model"] == "gpt-4o-mini"
    assert "provider" not in captured["body"]


def test_chat_completions_byok_requires_model(client):
    c, _mock_cls, _mock_inst = client
    r = c.post(
        "/v1/chat/completions",
        json={"messages": [{"role": "user", "content": "hi"}]},
        headers=_byok_headers(),
    )
    assert r.status_code == 400


def test_chat_completions_byok_unknown_provider_400(client):
    c, _mock_cls, _mock_inst = client
    r = c.post(
        "/v1/chat/completions",
        json={"model": "x", "messages": []},
        headers={"X-BYOK-Provider": "bogus", "X-BYOK-Api-Key": "k"},
    )
    assert r.status_code == 400
    assert r.json()["error"]["code"] == "byok_unknown_provider"


def test_embeddings_byok_forwards_with_client_key(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    captured = {}

    async def fake_post_with_retry(url, **kwargs):
        captured["url"] = url
        captured["headers"] = kwargs.get("headers")
        captured["body"] = kwargs.get("json_body")
        return type(
            "R",
            (),
            {
                "status_code": 200,
                "content": b'{"data":[]}',
                "headers": {"content-type": "application/json"},
            },
        )()

    monkeypatch.setattr("llm_proxy.routes.llm.post_with_retry", fake_post_with_retry)

    r = c.post(
        "/v1/embeddings",
        json={"model": "text-embedding-3-small", "input": "hello"},
        headers=_byok_headers(),
    )
    assert r.status_code == 200
    assert captured["url"] == "https://api.openai.com/v1/embeddings"
    assert captured["headers"]["Authorization"] == "Bearer sk-client"
    assert captured["body"]["model"] == "text-embedding-3-small"


def test_capabilities_advertises_byok(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "test-orch-key")
    r = c.get("/v1/capabilities", headers={"Authorization": "Bearer test-orch-key"})
    assert r.status_code == 200
    byok_block = r.json()["byok"]
    assert byok_block["enabled"] is True
    assert byok_block["transport"] == "headers"
    assert byok_block["headers"]["provider"] == "X-BYOK-Provider"
    ids = {p["id"] for p in byok_block["providers"]}
    assert {"openai", "anthropic", "gemini"} <= ids
    anthropic = next(p for p in byok_block["providers"] if p["id"] == "anthropic")
    assert anthropic["modalities"] == ["chat"]
    assert anthropic["canonical_host"] == "api.anthropic.com"


def test_embeddings_byok_unsupported_provider_501(client):
    c, _mock_cls, _mock_inst = client
    # Claude's OpenAI-compat surface has no embeddings endpoint.
    r = c.post(
        "/v1/embeddings",
        json={"model": "claude-x", "input": "hi"},
        headers={"X-BYOK-Provider": "anthropic", "X-BYOK-Api-Key": "sk-ant"},
    )
    assert r.status_code == 501
    assert r.json()["error"]["code"] == "capability_unavailable"
