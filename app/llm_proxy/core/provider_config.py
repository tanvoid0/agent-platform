"""Whether each upstream provider is usable (env / .env present and sufficient).

Add new providers: implement ``*_configured()`` and register it in ``_PROVIDER_CHECKS``.
Unknown provider names in YAML are treated as available until registered (forward-compatible).
"""

from __future__ import annotations

import os
from collections.abc import Callable
from dataclasses import dataclass

from llm_proxy_env import rewrite_upstream_localhost_for_docker

_runtime_ollama_base: str | None = None
_runtime_lm_studio_base: str | None = None

ProviderId = str
SUPPORTED_PROVIDER_IDS: tuple[ProviderId, ...] = ("ollama", "lm_studio", "aimlapi", "gemini")
PROVIDER_LABELS: dict[ProviderId, str] = {
    "ollama": "Ollama",
    "lm_studio": "LM Studio",
    "aimlapi": "AIMLAPI",
    "gemini": "Cloud",
}
PROVIDER_LOCAL_SORT_ORDER: dict[ProviderId, int] = {
    "ollama": 0,
    "lm_studio": 1,
    "aimlapi": 2,
    "gemini": 3,
}

# Standard loopback URLs when env / dotenv omit these keys (matches LOCAL_LLM_AUTO_DISCOVER probes).
DEFAULT_OLLAMA_BASE = "http://127.0.0.1:11434"
DEFAULT_LM_STUDIO_BASE = "http://127.0.0.1:1234"


def _from_env_or_dotenv(key: str) -> str:
    k = os.environ.get(key, "").strip()
    if k:
        return k
    from llm_proxy.core.config_cache import read_env_file_parsed

    return (read_env_file_parsed().get(key) or "").strip()


def ollama_api_base() -> str:
    """Ollama HTTP base. Env / dotenv wins; else startup discovery; else ``DEFAULT_OLLAMA_BASE``."""
    k = _from_env_or_dotenv("OLLAMA_API_BASE")
    if k:
        return rewrite_upstream_localhost_for_docker(k)
    rt = (_runtime_ollama_base or "").strip()
    if rt:
        return rewrite_upstream_localhost_for_docker(rt)
    return rewrite_upstream_localhost_for_docker(DEFAULT_OLLAMA_BASE)


def ollama_api_base_from_env_only() -> str:
    """Explicit ``OLLAMA_API_BASE`` from env or dotenv (excludes startup discovery)."""
    return _from_env_or_dotenv("OLLAMA_API_BASE")


def set_runtime_ollama_base(url: str | None) -> None:
    """Set discovered base when env is unset (used by local backend probe)."""
    global _runtime_ollama_base
    _runtime_ollama_base = (url or "").strip() or None


def ollama_configured() -> bool:
    return bool(ollama_api_base())


def gemini_api_key() -> str:
    return _from_env_or_dotenv("GEMINI_API_KEY")


def gemini_configured() -> bool:
    return bool(gemini_api_key())


def aimlapi_api_key() -> str:
    return _from_env_or_dotenv("AIMLAPI_API_KEY")


def aimlapi_openai_base() -> str:
    base = _from_env_or_dotenv("AIMLAPI_OPENAI_BASE")
    if base:
        return base.rstrip("/")
    return "https://api.aimlapi.com/v1"


def aimlapi_configured() -> bool:
    return bool(aimlapi_api_key())


def lm_studio_api_base() -> str:
    """LM Studio base (no ``/v1``). Env / dotenv wins; else discovery; else ``DEFAULT_LM_STUDIO_BASE``."""
    k = _from_env_or_dotenv("LM_STUDIO_API_BASE")
    if k:
        return rewrite_upstream_localhost_for_docker(k)
    rt = (_runtime_lm_studio_base or "").strip()
    if rt:
        return rewrite_upstream_localhost_for_docker(rt)
    return rewrite_upstream_localhost_for_docker(DEFAULT_LM_STUDIO_BASE)


def lm_studio_api_base_from_env_only() -> str:
    """Explicit ``LM_STUDIO_API_BASE`` from env or dotenv (excludes startup discovery)."""
    return _from_env_or_dotenv("LM_STUDIO_API_BASE")


def set_runtime_lm_studio_base(url: str | None) -> None:
    """Set discovered base when env is unset (used by local backend probe)."""
    global _runtime_lm_studio_base
    _runtime_lm_studio_base = (url or "").strip() or None


def clear_runtime_provider_bases() -> None:
    """Reset discovery overrides (tests)."""
    set_runtime_ollama_base(None)
    set_runtime_lm_studio_base(None)


def lm_studio_api_key() -> str:
    """Optional API key when LM Studio is configured to require Bearer auth."""
    return _from_env_or_dotenv("LM_STUDIO_API_KEY")


def lm_studio_configured() -> bool:
    return bool(lm_studio_api_base())


@dataclass(frozen=True)
class ProviderSpec:
    configured: Callable[[], bool]
    default_model: str


PROVIDER_SPECS: dict[ProviderId, ProviderSpec] = {
    "ollama": ProviderSpec(configured=ollama_configured, default_model="llama3"),
    "lm_studio": ProviderSpec(configured=lm_studio_configured, default_model="google/gemma-4-e4b"),
    "aimlapi": ProviderSpec(configured=aimlapi_configured, default_model="openai/gpt-4.1-mini"),
    "gemini": ProviderSpec(configured=gemini_configured, default_model="gemini-2.0-flash"),
}


def is_supported_provider(provider: str) -> bool:
    return (provider or "").strip().lower() in PROVIDER_SPECS


def provider_configured(provider: str) -> bool:
    """True if this provider name is registered and its requirements are met."""
    name = (provider or "").strip().lower()
    if not name or name == "other":
        return True
    spec = PROVIDER_SPECS.get(name)
    if spec is not None:
        return spec.configured()
    # Registered later in schema: show routes until a check exists
    return True


def first_configured_provider() -> str:
    """Prefer local backends first, then cloud when requested is unavailable."""
    if ollama_configured():
        return "ollama"
    if lm_studio_configured():
        return "lm_studio"
    if aimlapi_configured():
        return "aimlapi"
    if gemini_configured():
        return "gemini"
    return "lm_studio"


def default_model_for_provider(provider: str) -> str:
    """
    Canonical first default model id per provider when ``DEFAULT_MODEL`` env and config are empty.
    Matches the first entry in the LLM proxy settings UI lists.
    """
    p = (provider or "").strip().lower()
    spec = PROVIDER_SPECS.get(p)
    if spec is not None:
        return spec.default_model
    return "google/gemma-4-e4b"
