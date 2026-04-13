"""
Resolve llm-orchestrator settings: OpenAI-compatible base URL and bearer API key.

URL: inside Docker, http://127.0.0.1:... points at the container; when /.dockerenv is present,
rewrite localhost/127.0.0.1 to host.docker.internal (opt-out: AGENT_PLATFORM_ORCHESTRATOR_DOCKER_FIX=0).

API key: ORCHESTRATOR_MASTER_KEY (must match llm-orchestrator).
"""

from __future__ import annotations

import os
from urllib.parse import urlparse, urlunparse


def orchestrator_master_key() -> str:
    """Bearer token for Authorization when calling llm-orchestrator (OpenAI-compatible API)."""
    return (os.getenv("ORCHESTRATOR_MASTER_KEY") or "").strip()


def orchestrator_http_timeout_seconds() -> float:
    """
    Wall-clock timeout for HTTP calls from Agent Platform to llm-orchestrator (chat/completions, tools).

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


def orchestrator_base_url_v1() -> str:
    raw = (os.getenv("LLM_ORCHESTRATOR_BASE_URL") or "http://127.0.0.1:18408/v1").strip().rstrip("/")
    # OpenAI-compatible calls use POST {base}/chat/completions; llm-orchestrator serves /v1/chat/completions.
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
