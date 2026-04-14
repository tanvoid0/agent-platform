"""
Aggregate LLM provider status and model lists for the Flow UI (`GET /api/v1/llm/ui-catalog`).

Chat model ids match the embedded proxy (YAML aliases + live Ollama / LM Studio lists).
Gemini media defaults mirror `web/model-config.ts` so pickers stay consistent when the UI loads this API.
"""

from __future__ import annotations

from typing import Any

import httpx

from llm_proxy.routes.llm import collect_full_catalog_model_rows, get_resolved_proxy_defaults
from llm_proxy.core.provider_config import (
    PROVIDER_LABELS,
    SUPPORTED_PROVIDER_IDS,
    default_model_for_provider,
    aimlapi_api_key,
    aimlapi_configured,
    aimlapi_openai_base,
    gemini_configured,
    lm_studio_api_base,
    lm_studio_configured,
    ollama_api_base,
    ollama_configured,
    provider_configured,
)

_PROVIDER_ORDER = SUPPORTED_PROVIDER_IDS

# Gemini-only media defaults (keep in sync with web/model-config.ts gemini.*).
_GEMINI_MEDIA: dict[str, dict[str, Any]] = {
    "image": {
        "default_model": "gemini-3.1-flash-image-preview",
        "options": [
            "gemini-3.1-flash-image-preview",
            "gemini-3-pro-image-preview",
            "gemini-2.5-flash-image",
        ],
    },
    "music": {
        "default_model": "lyria-3-clip-preview",
        "options": ["lyria-3-clip-preview", "lyria-3-pro-preview"],
    },
    "video": {
        "default_model": "veo-3.1-lite-generate-preview",
        "options": [
            "veo-3.1-lite-generate-preview",
            "veo-3.1-fast-generate-preview",
            "veo-3.1-generate-preview",
        ],
    },
}


async def _probe_tcp_ok(url: str, *, timeout: float = 2.0) -> bool:
    try:
        async with httpx.AsyncClient(timeout=timeout) as client:
            r = await client.get(url)
        return r.status_code < 500
    except httpx.HTTPError:
        return False


async def _provider_reachable(pid: str) -> bool | None:
    if pid == "ollama":
        if not ollama_configured():
            return False
        base = ollama_api_base().rstrip("/")
        return await _probe_tcp_ok(f"{base}/api/version")
    if pid == "lm_studio":
        if not lm_studio_configured():
            return False
        base = lm_studio_api_base().rstrip("/")
        return await _probe_tcp_ok(f"{base}/v1/models")
    if pid == "gemini":
        # Intentionally do not call Gemini/Google from this route: `ui-catalog` loads at
        # app startup and meta/model probes are billable. "Reachable" = API key is set.
        return gemini_configured()
    if pid == "aimlapi":
        if not aimlapi_configured():
            return False
        try:
            async with httpx.AsyncClient(timeout=4.0) as client:
                r = await client.get(
                    f"{aimlapi_openai_base().rstrip('/')}/models",
                    headers={"Authorization": f"Bearer {aimlapi_api_key()}"},
                )
            return r.status_code < 500
        except httpx.HTTPError:
            return False
    return None


def _chat_slice_for_provider(
    pid: str,
    resolved: dict[str, str],
    rows: list[dict[str, str]],
) -> dict[str, Any]:
    ids = sorted(
        {r["id"] for r in rows if r.get("owned_by") == pid},
        key=lambda s: s.lower(),
    )
    if resolved.get("provider") == pid and (resolved.get("model") or "").strip():
        dm = resolved["model"].strip()
    else:
        dm = default_model_for_provider(pid)
    ordered: list[str] = []
    seen: set[str] = set()
    for x in [dm, *ids]:
        if x not in seen:
            seen.add(x)
            ordered.append(x)
    if not ordered:
        ordered = [dm]
    return {"default_model": dm, "options": ordered}


async def build_llm_ui_catalog_response() -> dict[str, Any]:
    resolved = get_resolved_proxy_defaults()
    rows = await collect_full_catalog_model_rows()
    out_providers: list[dict[str, Any]] = []
    for pid in _PROVIDER_ORDER:
        cfg = provider_configured(pid)
        reachable: bool | None = None
        if cfg:
            reachable = await _provider_reachable(pid)
        chat = _chat_slice_for_provider(pid, resolved, rows)
        out_providers.append(
            {
                "id": pid,
                "label": PROVIDER_LABELS.get(pid, pid),
                "configured": cfg,
                "reachable": reachable,
                "chat": chat,
            }
        )
    return {
        "resolved_defaults": resolved,
        "providers": out_providers,
        "gemini_media": _GEMINI_MEDIA,
    }
