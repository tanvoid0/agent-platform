"""LM Studio provider wiring (OpenAI-compatible local server)."""

from llm_proxy.core.provider_config import (
    DEFAULT_LM_STUDIO_BASE,
    clear_runtime_provider_bases,
    default_model_for_provider,
    lm_studio_api_base,
    lm_studio_configured,
    provider_configured,
)


def test_lm_studio_default_base_when_env_unset(monkeypatch):
    monkeypatch.delenv("LM_STUDIO_API_BASE", raising=False)
    monkeypatch.delenv("LM_STUDIO_API_KEY", raising=False)
    clear_runtime_provider_bases()
    assert lm_studio_api_base() == DEFAULT_LM_STUDIO_BASE
    assert lm_studio_configured() is True
    assert provider_configured("lm_studio") is True


def test_lm_studio_explicit_env_overrides_default(monkeypatch):
    monkeypatch.setenv("LM_STUDIO_API_BASE", "http://192.168.1.10:1234")
    assert lm_studio_api_base() == "http://192.168.1.10:1234"
    assert lm_studio_configured() is True


def test_default_model_for_provider():
    assert default_model_for_provider("ollama") == "llama3"
    assert default_model_for_provider("lm_studio") == "google/gemma-4-e4b"
    assert default_model_for_provider("gemini") == "gemini-2.0-flash"
    assert default_model_for_provider("aimlapi") == "openai/gpt-4.1-mini"
