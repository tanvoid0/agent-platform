"""Tests for per-model capability probing and request guards."""

import asyncio

import pytest

from llm_proxy.core.errors import LlmProxyError
from llm_proxy.services import model_capabilities as mc

pytestmark = pytest.mark.contract


def _run(coro):
    return asyncio.run(coro)


def test_normalize_ollama_capabilities_maps_tools_and_vision():
    caps = mc.normalize_ollama_capabilities(["completion", "tools", "vision"])
    assert caps["chat"] is True
    assert caps["tools"] is True
    assert caps["vision_input"] is True
    assert caps["probe_source"] == "ollama_show"


def test_infer_qwen3_coder_has_tools():
    caps = mc.infer_capabilities_from_model_name("qwen3-coder:30b", "ollama")
    assert caps["tools"] is True
    assert caps["probe_source"] == "heuristic"


def test_request_uses_tools_detects_tools_array():
    assert mc.request_uses_tools({"tools": [{"type": "function"}]}) is True
    assert mc.request_uses_tools({"tool_choice": "none"}) is False
    assert mc.request_uses_tools({}) is False


def test_messages_contain_vision_detects_image_url_part():
    body = {
        "messages": [
            {
                "role": "user",
                "content": [{"type": "text", "text": "hi"}, {"type": "image_url", "image_url": {"url": "x"}}],
            }
        ]
    }
    assert mc.messages_contain_vision(body["messages"]) is True


def test_require_model_capability_raises_structured_error():
    caps = mc.provider_default_capabilities("ollama")
    caps["tools"] = False
    with pytest.raises(LlmProxyError) as exc:
        mc.require_model_capability("ollama", "tinyllama", caps, "tools")
    assert exc.value.status_code == 501
    assert exc.value.code == "capability_unavailable"
    assert exc.value.extra["capability"] == "tools"
    assert exc.value.extra["model"] == "tinyllama"


def test_merge_capabilities_sticky_keeps_tools_when_fresh_denies():
    previous = {
        "chat": True,
        "tools": True,
        "vision_input": False,
        "embeddings": False,
        "image_generation": False,
        "streaming": True,
        "probe_source": "ollama_show",
        "probed_at": "2026-07-11T20:00:00+00:00",
    }
    fresh = dict(previous)
    fresh["tools"] = False
    merged = mc.merge_capabilities_sticky(previous, fresh)
    assert merged["tools"] is True


def test_resolve_model_capabilities_persists_and_skips_reprobe(tmp_path, monkeypatch):
    mc.clear_capability_cache(disk=True)
    monkeypatch.setenv("CONFIG_DIR", str(tmp_path))
    calls = {"count": 0}

    async def fake_show(url, **kwargs):
        calls["count"] += 1

        class Resp:
            status_code = 200

            def json(self):
                return {"capabilities": ["completion", "tools"]}

        return Resp()

    monkeypatch.setattr(mc, "post_with_retry", fake_show)
    monkeypatch.setattr(mc, "ollama_api_base", lambda: "http://127.0.0.1:11434")

    caps1 = _run(mc.resolve_model_capabilities("ollama", "qwen3-coder:30b", probe=True))
    caps2 = _run(mc.resolve_model_capabilities("ollama", "qwen3-coder:30b", probe=True))

    assert caps1["tools"] is True
    assert caps2["tools"] is True
    assert calls["count"] == 1
    assert (tmp_path / "model_capabilities.json").is_file()

    mc.clear_capability_cache(disk=False)
    caps3 = _run(mc.resolve_model_capabilities("ollama", "qwen3-coder:30b", probe=True))
    assert caps3["tools"] is True
    assert calls["count"] == 1


def test_resolve_model_capabilities_uses_ollama_show(monkeypatch, tmp_path):
    mc.clear_capability_cache(disk=True)
    monkeypatch.setenv("CONFIG_DIR", str(tmp_path))

    async def fake_show(url, **kwargs):
        assert "/api/show" in url
        body = kwargs.get("json_body") or {}
        assert body["name"] == "qwen3-coder:30b"

        class Resp:
            status_code = 200

            def json(self):
                return {"capabilities": ["completion", "tools"]}

        return Resp()

    monkeypatch.setattr(mc, "post_with_retry", fake_show)
    monkeypatch.setattr(mc, "ollama_api_base", lambda: "http://127.0.0.1:11434")

    caps = _run(mc.resolve_model_capabilities("ollama", "qwen3-coder:30b", probe=True))
    assert caps["tools"] is True
    assert caps["probe_source"] == "ollama_show"


def test_ensure_chat_request_supported_blocks_tools(monkeypatch, tmp_path):
    mc.clear_capability_cache(disk=True)
    monkeypatch.setenv("CONFIG_DIR", str(tmp_path))

    async def fake_resolve(provider, model, *, probe=True, ollama_base=None):
        return {
            "chat": True,
            "tools": False,
            "vision_input": False,
            "embeddings": False,
            "image_generation": False,
            "streaming": True,
            "probe_source": "ollama_show",
        }

    monkeypatch.setattr(mc, "resolve_model_capabilities", fake_resolve)

    with pytest.raises(LlmProxyError) as exc:
        _run(
            mc.ensure_chat_request_supported(
                "ollama",
                "tinyllama",
                {"messages": [], "tools": [{"type": "function"}]},
            )
        )
    assert exc.value.extra["capability"] == "tools"
