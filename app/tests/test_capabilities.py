"""Tests for the capability (modality) contract and capability-based routing."""

import pytest

from llm_proxy.core import capabilities as cap

pytestmark = pytest.mark.contract

MASTER_KEY = "test-orch-key"


def _master_headers():
    return {"Authorization": f"Bearer {MASTER_KEY}"}


# --- unit: modality declarations ------------------------------------------


def test_modality_map_has_all_modalities():
    m = cap.modality_map("ollama")
    assert set(m) == set(cap.MODALITIES)
    assert m["chat"] is True
    assert m["vision_input"] is True
    assert m["embeddings"] is False
    assert m["image_generation"] is False


def test_unregistered_provider_defaults_chat_only():
    m = cap.modality_map("some-future-provider")
    assert m["chat"] is True
    assert m["embeddings"] is False
    assert m["image_generation"] is False


def test_image_local_declares_image_generation():
    assert cap.provider_supports("image_local", "image_generation") is True
    assert cap.provider_supports("image_local", "chat") is False


def test_image_generation_unresolved_when_backend_unconfigured(monkeypatch):
    # No IMAGE_API_BASE => image_local not configured => nothing resolves.
    monkeypatch.setattr(cap, "image_provider_configured", lambda p: False)
    assert cap.providers_for_capability("image_generation") == []
    assert cap.resolve_provider_for_capability("image_generation") is None


def test_image_generation_resolves_when_backend_configured(monkeypatch):
    monkeypatch.setattr(cap, "image_provider_configured", lambda p: p == "image_local")
    assert cap.resolve_provider_for_capability("image_generation") == "image_local"


def test_anthropic_has_no_embeddings():
    assert cap.provider_supports("anthropic", "chat") is True
    assert cap.provider_supports("anthropic", "embeddings") is False


# --- unit: capability router ----------------------------------------------


def test_resolve_prefers_local_backend(monkeypatch):
    # Only lm_studio configured -> it resolves for embeddings.
    monkeypatch.setattr(cap, "provider_configured", lambda p: p == "lm_studio")
    assert cap.resolve_provider_for_capability("embeddings") == "lm_studio"


def test_require_raises_501_when_no_provider(monkeypatch):
    monkeypatch.setattr(cap, "provider_configured", lambda p: False)
    with pytest.raises(cap.LlmProxyError) as exc:
        cap.require_provider_for_capability("image_generation")
    assert exc.value.status_code == 501
    assert exc.value.code == "capability_unavailable"
    assert exc.value.extra["capability"] == "image_generation"


def test_require_preferred_unsupported_raises_501(monkeypatch):
    monkeypatch.setattr(cap, "provider_configured", lambda p: True)
    with pytest.raises(cap.LlmProxyError) as exc:
        cap.require_provider_for_capability("embeddings", preferred="anthropic")
    assert exc.value.status_code == 501


def test_require_preferred_unconfigured_raises_503(monkeypatch):
    monkeypatch.setattr(cap, "provider_configured", lambda p: False)
    with pytest.raises(cap.LlmProxyError) as exc:
        cap.require_provider_for_capability("chat", preferred="ollama")
    assert exc.value.status_code == 503


# --- route: /v1/capabilities ----------------------------------------------


def test_capabilities_endpoint_shape(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", MASTER_KEY)
    r = c.get("/v1/capabilities", headers=_master_headers())
    assert r.status_code == 200
    body = r.json()
    assert body["object"] == "capabilities"
    assert "image_generation" in body["modalities"]
    assert set(body["providers"]) == set(
        ["ollama", "lm_studio", "aimlapi", "anthropic", "gemini", "image_local"]
    )
    assert "chat" in body["providers"]["ollama"]
    assert "configured" in body["providers"]["ollama"]
    assert body["providers"]["image_local"]["image_generation"] is True
    # No IMAGE_API_BASE in test env => image backend unconfigured, unresolved.
    assert body["resolved"]["image_generation"] is None


def test_catalog_capabilities_include_modalities(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", MASTER_KEY)

    async def fake_get_with_retry(url, **kwargs):
        if "/api/version" in url or "/api/tags" in url:
            return type(
                "R", (), {"status_code": 200, "json": staticmethod(lambda: {"models": []})}
            )()
        return type("R", (), {"status_code": 200, "json": staticmethod(lambda: {"data": []})})()

    monkeypatch.setattr(
        "llm_proxy.services.provider_catalog.get_with_retry", fake_get_with_retry
    )
    r = c.get("/v1/catalog?providers=ollama&live=false", headers=_master_headers())
    assert r.status_code == 200
    ollama = next(p for p in r.json()["providers"] if p["id"] == "ollama")
    assert "modalities" in ollama["capabilities"]
    assert ollama["capabilities"]["modalities"]["chat"] is True


# --- route: /v1/images/generations ----------------------------------------


def test_images_501_when_no_backend_configured(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", MASTER_KEY)
    monkeypatch.delenv("IMAGE_API_BASE", raising=False)
    r = c.post(
        "/v1/images/generations",
        json={"prompt": "a red fox", "model": "flux.1-schnell"},
        headers=_master_headers(),
    )
    assert r.status_code == 501
    assert r.json()["error"]["code"] == "capability_unavailable"


def test_images_forwarded_when_backend_configured(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", MASTER_KEY)
    monkeypatch.setenv("IMAGE_API_BASE", "http://127.0.0.1:9999")

    captured = {}

    async def fake_post_with_retry(url, **kwargs):
        captured["url"] = url
        captured["body"] = kwargs.get("json_body")
        return type(
            "R",
            (),
            {
                "status_code": 200,
                "content": b'{"data":[{"b64_json":"x"}]}',
                "headers": {"content-type": "application/json"},
            },
        )()

    monkeypatch.setattr("llm_proxy.routes.llm.post_with_retry", fake_post_with_retry)

    r = c.post(
        "/v1/images/generations",
        json={"prompt": "a red fox"},
        headers=_master_headers(),
    )
    assert r.status_code == 200
    assert captured["url"] == "http://127.0.0.1:9999/v1/images/generations"
    # Default model applied when omitted.
    assert captured["body"]["model"] == "flux.1-schnell"
    assert captured["body"]["prompt"] == "a red fox"
