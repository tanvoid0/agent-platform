"""Local Ollama/LM Studio discovery helpers."""

import os

from llm_proxy.core.provider_config import (
    clear_runtime_provider_bases,
    lm_studio_api_base,
    ollama_api_base,
    set_runtime_lm_studio_base,
    set_runtime_ollama_base,
)
from llm_proxy.services.local_backends import ollama_tag_matches


def test_ollama_tag_matches_exact_and_bare_name():
    tags = ["llama3:latest", "mistral:7b"]
    assert ollama_tag_matches(tags, "llama3:latest") is True
    assert ollama_tag_matches(tags, "llama3") is True
    assert ollama_tag_matches(tags, "mistral") is True
    assert ollama_tag_matches(tags, "phi3") is False


def _patch_not_inside_docker(monkeypatch):
    """Tests expect loopback URLs; avoid failing when pytest runs inside a container."""
    orig = os.path.exists

    def exists(p):
        if str(p) == "/.dockerenv":
            return False
        return orig(p)

    monkeypatch.setattr(os.path, "exists", exists)


def test_default_bases_when_env_and_runtime_unset(monkeypatch):
    _patch_not_inside_docker(monkeypatch)
    monkeypatch.delenv("OLLAMA_API_BASE", raising=False)
    monkeypatch.delenv("LM_STUDIO_API_BASE", raising=False)
    clear_runtime_provider_bases()
    assert ollama_api_base() == "http://127.0.0.1:11434"
    assert lm_studio_api_base() == "http://127.0.0.1:1234"


def test_runtime_bases_override_when_env_unset(monkeypatch):
    _patch_not_inside_docker(monkeypatch)
    monkeypatch.delenv("OLLAMA_API_BASE", raising=False)
    monkeypatch.delenv("LM_STUDIO_API_BASE", raising=False)
    clear_runtime_provider_bases()
    set_runtime_ollama_base("http://127.0.0.1:11434")
    set_runtime_lm_studio_base("http://127.0.0.1:1234")
    assert ollama_api_base() == "http://127.0.0.1:11434"
    assert lm_studio_api_base() == "http://127.0.0.1:1234"


def test_env_wins_over_runtime(monkeypatch):
    _patch_not_inside_docker(monkeypatch)
    monkeypatch.setenv("OLLAMA_API_BASE", "http://custom:11434")
    set_runtime_ollama_base("http://127.0.0.1:11434")
    assert ollama_api_base() == "http://custom:11434"


def test_lm_studio_loopback_rewrites_to_host_gateway_in_docker(monkeypatch):
    """Docker: localhost LM Studio URL must target the host, not the container."""
    orig = os.path.exists
    monkeypatch.delenv("LM_STUDIO_API_BASE", raising=False)
    monkeypatch.delenv("AGENT_PLATFORM_LOCAL_LLM_DOCKER_FIX", raising=False)
    clear_runtime_provider_bases()

    def exists(p):
        if str(p) == "/.dockerenv":
            return True
        return orig(p)

    monkeypatch.setattr(os.path, "exists", exists)
    assert lm_studio_api_base() == "http://host.docker.internal:1234"


def test_local_llm_docker_fix_opt_out(monkeypatch):
    orig = os.path.exists
    monkeypatch.setenv("AGENT_PLATFORM_LOCAL_LLM_DOCKER_FIX", "0")
    monkeypatch.delenv("LM_STUDIO_API_BASE", raising=False)
    clear_runtime_provider_bases()

    def exists(p):
        if str(p) == "/.dockerenv":
            return True
        return orig(p)

    monkeypatch.setattr(os.path, "exists", exists)
    assert lm_studio_api_base() == "http://127.0.0.1:1234"
