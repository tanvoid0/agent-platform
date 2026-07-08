"""Tests for GET /v1/catalog provider discovery API."""

import json

import pytest

pytestmark = pytest.mark.contract

MASTER_KEY = "test-orch-key"


def _master_headers():
    return {"Authorization": f"Bearer {MASTER_KEY}"}


def _create_workspace(c):
    r = c.post("/workspaces/", json={"name": "CatalogWs"}, headers=_master_headers())
    assert r.status_code == 201, r.text
    return r.json()["id"]


def _create_workspace_token(c, workspace_id, **kwargs):
    body = {"name": "catalog-token", "scopes": ["chat:write"]}
    body.update(kwargs)
    r = c.post(
        f"/workspaces/{workspace_id}/api-tokens/",
        json=body,
        headers=_master_headers(),
    )
    assert r.status_code == 201, r.text
    return r.json()


def test_v1_catalog_requires_auth_when_master_key_set(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "secret-key")
    r = c.get("/v1/catalog")
    assert r.status_code == 401


def test_v1_catalog_accepts_workspace_token(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", MASTER_KEY)
    ws = _create_workspace(c)
    created = _create_workspace_token(c, ws)
    token_headers = {"Authorization": f"Bearer {created['token']}"}

    async def fake_get_with_retry(url, **kwargs):
        if "/api/version" in url or "/api/tags" in url:
            return type(
                "R",
                (),
                {"status_code": 200, "json": staticmethod(lambda: {"models": []})},
            )()
        return type("R", (), {"status_code": 404, "json": staticmethod(lambda: {})})()

    monkeypatch.setattr("llm_proxy.services.provider_catalog.get_with_retry", fake_get_with_retry)

    r = c.get("/v1/catalog?providers=ollama&live=false", headers=token_headers)
    assert r.status_code == 200
    assert r.json()["object"] == "catalog"


def test_v1_catalog_returns_provider_shape(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", MASTER_KEY)
    auth = _master_headers()

    async def fake_get_with_retry(url, **kwargs):
        if "/api/version" in url or "/api/tags" in url:
            return type(
                "R",
                (),
                {
                    "status_code": 200,
                    "json": staticmethod(
                        lambda: {
                            "models": [
                                {
                                    "name": "llama3.2:latest",
                                    "size": 2019393189,
                                    "details": {
                                        "family": "llama",
                                        "parameter_size": "3.2B",
                                        "quantization_level": "Q4_K_M",
                                    },
                                }
                            ]
                        }
                        if "/api/tags" in url
                        else {}
                    ),
                },
            )()
        return type("R", (), {"status_code": 200, "json": staticmethod(lambda: {"data": []})})()

    monkeypatch.setattr("llm_proxy.services.provider_catalog.get_with_retry", fake_get_with_retry)

    r = c.get("/v1/catalog?providers=all", headers=auth)
    assert r.status_code == 200
    body = r.json()
    assert body["object"] == "catalog"
    assert "resolved_defaults" in body
    assert "providers" in body
    assert len(body["providers"]) == 5
    ollama = next(p for p in body["providers"] if p["id"] == "ollama")
    assert ollama["label"] == "Ollama"
    assert "configured" in ollama
    assert "reachable" in ollama
    assert "default_model" in ollama
    assert "capabilities" in ollama
    assert "streaming" in ollama["capabilities"]
    assert "model_discovery" in ollama["capabilities"]
    assert isinstance(ollama["models"], list)


def test_v1_catalog_live_false_skips_upstream(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", MASTER_KEY)
    auth = _master_headers()

    async def boom(*args, **kwargs):
        raise AssertionError("upstream should not be called when live=false")

    monkeypatch.setattr("llm_proxy.services.upstream_http.get_with_retry", boom)

    r = c.get("/v1/catalog?providers=ollama&live=false", headers=auth)
    assert r.status_code == 200
    ollama = next(p for p in r.json()["providers"] if p["id"] == "ollama")
    for model in ollama["models"]:
        assert model["source"] == "alias"


def test_v1_catalog_unknown_provider_returns_400(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", MASTER_KEY)
    auth = _master_headers()
    r = c.get("/v1/catalog?providers=not-a-provider", headers=auth)
    assert r.status_code == 400


def test_v1_catalog_ollama_metadata_attached(client, monkeypatch):
    c, _mock_cls, _mock_inst = client
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", MASTER_KEY)
    auth = _master_headers()

    async def fake_get_with_retry(url, **kwargs):
        if "/api/tags" in url:
            return type(
                "R",
                (),
                {
                    "status_code": 200,
                    "json": staticmethod(
                        lambda: {
                            "models": [
                                {
                                    "name": "llama3.2:latest",
                                    "size": 42,
                                    "details": {
                                        "family": "llama",
                                        "parameter_size": "3.2B",
                                    },
                                }
                            ]
                        }
                    ),
                },
            )()
        if "/api/version" in url:
            return type("R", (), {"status_code": 200, "json": staticmethod(lambda: {})})()
        return type("R", (), {"status_code": 404, "json": staticmethod(lambda: {})})()

    monkeypatch.setattr("llm_proxy.services.provider_catalog.get_with_retry", fake_get_with_retry)

    r = c.get("/v1/catalog?providers=ollama", headers=auth)
    assert r.status_code == 200
    ollama = next(p for p in r.json()["providers"] if p["id"] == "ollama")
    live = next((m for m in ollama["models"] if m["id"] == "llama3.2:latest"), None)
    assert live is not None
    assert live["source"] == "live"
    assert live["metadata"]["family"] == "llama"
    assert live["metadata"]["parameter_size"] == "3.2B"
    assert live["metadata"]["size"] == 42
