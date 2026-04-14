"""
Resolve embedded LLM proxy settings: OpenAI-compatible base URL and bearer API key.

The Agent Platform process serves `/v1/*`; internal HTTP calls use ``LLM_ORCHESTRATOR_BASE_URL``
(legacy name) defaulting to the same host.

URL: inside Docker, http://127.0.0.1:... points at the container; when /.dockerenv is present,
rewrite localhost/127.0.0.1 to host.docker.internal (opt-out: AGENT_PLATFORM_ORCHESTRATOR_DOCKER_FIX=0).

LM Studio / Ollama on the Docker **host**: ``OLLAMA_API_BASE`` / ``LM_STUDIO_API_BASE`` use the same
rewrite when running in Docker (opt-out: AGENT_PLATFORM_LOCAL_LLM_DOCKER_FIX=0). Compose keeps
``AGENT_PLATFORM_ORCHESTRATOR_DOCKER_FIX=0`` so the API can still call its own :18410 inside the container.

API key: AGENT_PLATFORM_MASTER_KEY (bearer for embedded LLM proxy /v1/*).
"""

from __future__ import annotations

import os
from urllib.parse import urlparse, urlunparse


def llm_proxy_master_key() -> str:
    """Bearer token for Authorization when calling the embedded OpenAI-compatible LLM proxy."""
    return (os.getenv("AGENT_PLATFORM_MASTER_KEY") or "").strip()


def llm_proxy_http_timeout_seconds() -> float:
    """
    Wall-clock timeout for HTTP calls from Agent Platform to the embedded LLM proxy (chat/completions, tools).

    Local Ollama runs often exceed a few minutes; keep this aligned with the Flow UI chat timeout
    (``VITE_API_CHAT_TIMEOUT_MS``). If this is too short, the client may close the connection while
    Ollama is still generating — Ollama logs: "aborting completion request due to client closing the connection".
    """
    raw = (os.getenv("AGENT_PLATFORM_ORCHESTRATOR_TIMEOUT_SECONDS") or "").strip()
    if not raw:
        return 600.0
    try:
        v = float(raw)
        return max(10.0, min(86400.0, v))
    except ValueError:
        return 600.0


def llm_proxy_base_url_v1() -> str:
    raw = (os.getenv("LLM_ORCHESTRATOR_BASE_URL") or "http://127.0.0.1:18410/v1").strip().rstrip("/")
    # OpenAI-compatible calls use POST {base}/chat/completions; the embedded proxy serves /v1/chat/completions.
    # Some docs use a host-only base (no /v1), which would otherwise hit /chat/completions and return 404.
    if not raw.endswith("/v1"):
        raw = f"{raw}/v1"
    if _docker_fix_disabled():
        return raw
    return _rewrite_localhost_for_docker(raw)


def _docker_fix_disabled() -> bool:
    v = (os.getenv("AGENT_PLATFORM_ORCHESTRATOR_DOCKER_FIX") or "1").strip().lower()
    return v in ("0", "false", "no", "off")


def _rewrite_localhost_for_docker(url: str) -> str:
    if not os.path.exists("/.dockerenv"):
        return url
    parsed = urlparse(url)
    if parsed.hostname not in ("127.0.0.1", "localhost"):
        return url
    port = parsed.port
    netloc = "host.docker.internal" if port is None else f"host.docker.internal:{port}"
    rebuilt = urlunparse(
        (parsed.scheme, netloc, parsed.path, parsed.params, parsed.query, parsed.fragment)
    )
    return rebuilt.rstrip("/")


def _local_llm_docker_fix_disabled() -> bool:
    v = (os.getenv("AGENT_PLATFORM_LOCAL_LLM_DOCKER_FIX") or "1").strip().lower()
    return v in ("0", "false", "no", "off")


def rewrite_upstream_localhost_for_docker(url: str) -> str:
    """
    Map loopback LM Studio / Ollama bases to the Docker host.

    Same process as ``_rewrite_localhost_for_docker``, but controlled by
    ``AGENT_PLATFORM_LOCAL_LLM_DOCKER_FIX`` so Compose can leave
    ``AGENT_PLATFORM_ORCHESTRATOR_DOCKER_FIX=0`` for self-calls to :18410.
    """
    if not (url or "").strip():
        return url
    if _local_llm_docker_fix_disabled():
        return url
    return _rewrite_localhost_for_docker(url.strip())
