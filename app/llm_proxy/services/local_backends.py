"""Discover local Ollama / LM Studio URLs at startup and coerce unknown models to a working id."""

from __future__ import annotations

import asyncio
import logging
import os
import shutil
from typing import Any

from llm_proxy.core.errors import LlmProxyError
from llm_proxy.core.provider_config import (
    DEFAULT_LM_STUDIO_BASE,
    DEFAULT_OLLAMA_BASE,
    lm_studio_api_base,
    lm_studio_api_base_from_env_only,
    lm_studio_api_key,
    ollama_api_base,
    ollama_api_base_from_env_only,
    set_runtime_lm_studio_base,
    set_runtime_ollama_base,
)
from llm_proxy.services.upstream_http import get_with_retry, post_with_retry

logger = logging.getLogger("llm_proxy")


def _env_flag(name: str, default: bool = True) -> bool:
    v = os.environ.get(name, "").strip().lower()
    if not v:
        return default
    return v in ("1", "true", "yes", "on")


def ollama_tag_matches(available: list[str], requested: str) -> bool:
    """True if ``requested`` matches an Ollama tag name (including bare name before ``:``)."""
    m = (requested or "").strip()
    if not m:
        return False
    for n in available:
        if n == m or n.split(":")[0] == m.split(":")[0]:
            return True
    return False


async def _ollama_http_version_ok(base: str) -> bool:
    url = f"{base.rstrip('/')}/api/version"
    try:
        r = await get_with_retry(url, timeout=4.0, context="discover_ollama_version")
        return r.status_code == 200
    except LlmProxyError:
        return False


async def _ollama_cli_list_ok() -> bool:
    exe = shutil.which("ollama")
    if not exe:
        return False
    try:
        proc = await asyncio.create_subprocess_exec(
            exe,
            "list",
            stdout=asyncio.subprocess.DEVNULL,
            stderr=asyncio.subprocess.DEVNULL,
        )
        await asyncio.wait_for(proc.wait(), timeout=12.0)
        return proc.returncode == 0
    except (asyncio.TimeoutError, OSError, ProcessLookupError):
        return False


async def _lm_studio_openai_models_ok(base: str) -> bool:
    url = f"{base.rstrip('/')}/v1/models"
    headers: dict[str, str] = {}
    key = lm_studio_api_key()
    if key:
        headers["Authorization"] = f"Bearer {key}"
    try:
        r = await get_with_retry(
            url,
            headers=headers,
            timeout=4.0,
            context="discover_lm_studio_v1_models",
        )
        return r.status_code == 200
    except LlmProxyError:
        return False


async def discover_local_llm_bases() -> None:
    """
    When ``OLLAMA_API_BASE`` / ``LM_STUDIO_API_BASE`` are unset, probe default loopback URLs.

    Ollama: requires HTTP ``/api/version``; if ``ollama`` is on PATH, ``ollama list`` must succeed too.
    LM Studio: HTTP ``/v1/models`` must succeed (same as OpenAI compatibility probe).
    """
    if not _env_flag("LOCAL_LLM_AUTO_DISCOVER", True):
        return

    if not ollama_api_base_from_env_only().strip():
        http_ok = await _ollama_http_version_ok(DEFAULT_OLLAMA_BASE)
        if not http_ok:
            logger.debug("Local Ollama not detected at %s", DEFAULT_OLLAMA_BASE)
        elif shutil.which("ollama") and not await _ollama_cli_list_ok():
            logger.warning(
                "Ollama responds at %s but `ollama list` failed; set OLLAMA_API_BASE explicitly or fix CLI.",
                DEFAULT_OLLAMA_BASE,
            )
        else:
            set_runtime_ollama_base(DEFAULT_OLLAMA_BASE)
            logger.info(
                "Using discovered Ollama at %s (OLLAMA_API_BASE unset)",
                DEFAULT_OLLAMA_BASE,
            )

    if not lm_studio_api_base_from_env_only().strip():
        if await _lm_studio_openai_models_ok(DEFAULT_LM_STUDIO_BASE):
            set_runtime_lm_studio_base(DEFAULT_LM_STUDIO_BASE)
            logger.info(
                "Using discovered LM Studio at %s (LM_STUDIO_API_BASE unset)",
                DEFAULT_LM_STUDIO_BASE,
            )
        else:
            logger.debug("LM Studio not detected at %s", DEFAULT_LM_STUDIO_BASE)


async def _fetch_ollama_tag_names(base: str) -> list[str]:
    url = f"{base.rstrip('/')}/api/tags"
    try:
        r = await get_with_retry(url, timeout=12.0, context="local_backends_ollama_tags")
    except LlmProxyError:
        return []
    if r.status_code != 200:
        return []
    try:
        payload = r.json()
    except ValueError:
        return []
    models = payload.get("models") if isinstance(payload, dict) else None
    if not isinstance(models, list):
        return []
    names: list[str] = []
    for item in models:
        if isinstance(item, dict):
            mid = item.get("name")
            if isinstance(mid, str) and mid.strip():
                names.append(mid.strip())
    return names


async def _fetch_lm_studio_openai_ids(base: str) -> list[str]:
    url = f"{base.rstrip('/')}/v1/models"
    headers: dict[str, str] = {}
    key = lm_studio_api_key()
    if key:
        headers["Authorization"] = f"Bearer {key}"
    try:
        r = await get_with_retry(
            url,
            headers=headers,
            timeout=12.0,
            context="local_backends_lm_studio_openai",
        )
    except LlmProxyError:
        return []
    if r.status_code != 200:
        return []
    try:
        payload = r.json()
    except ValueError:
        return []
    data = payload.get("data") if isinstance(payload, dict) else None
    if not isinstance(data, list):
        return []
    ids: list[str] = []
    for item in data:
        if isinstance(item, dict):
            mid = item.get("id")
            if isinstance(mid, str) and mid.strip():
                ids.append(mid.strip())
    return ids


async def _fetch_lm_studio_native_llm_keys(base: str) -> list[str]:
    """Fallback catalog from LM Studio native REST when OpenAI list is empty."""
    url = f"{base.rstrip('/')}/api/v1/models"
    headers: dict[str, str] = {}
    key = lm_studio_api_key()
    if key:
        headers["Authorization"] = f"Bearer {key}"
    try:
        r = await get_with_retry(
            url,
            headers=headers,
            timeout=12.0,
            context="local_backends_lm_studio_native",
        )
    except LlmProxyError:
        return []
    if r.status_code != 200:
        return []
    try:
        payload = r.json()
    except ValueError:
        return []
    raw = payload.get("models") if isinstance(payload, dict) else None
    if not isinstance(raw, list):
        return []
    keys: list[str] = []
    for item in raw:
        if not isinstance(item, dict):
            continue
        if str(item.get("type") or "").lower() != "llm":
            continue
        k = item.get("key")
        if isinstance(k, str) and k.strip():
            keys.append(k.strip())
    return keys


async def lm_studio_request_load_model(base: str, model_id: str) -> bool:
    """
    POST /api/v1/models/load — loads the model into memory when supported (LM Studio 0.4+).
    Returns True when the server reports success.
    """
    url = f"{base.rstrip('/')}/api/v1/models/load"
    headers: dict[str, str] = {"Content-Type": "application/json"}
    key = lm_studio_api_key()
    if key:
        headers["Authorization"] = f"Bearer {key}"
    timeout = float(os.environ.get("LM_STUDIO_LOAD_TIMEOUT_SEC", "600"))
    try:
        r = await post_with_retry(
            url,
            headers=headers,
            json_body={"model": model_id},
            timeout=max(30.0, timeout),
            context="lm_studio_models_load",
        )
    except LlmProxyError as e:
        logger.debug("LM Studio load API failed: %s", e.message)
        return False
    if r.status_code >= 400:
        logger.debug("LM Studio load returned HTTP %s: %s", r.status_code, r.text[:300])
        return False
    try:
        body: dict[str, Any] = r.json()
    except ValueError:
        return r.status_code == 200
    status = body.get("status")
    return status == "loaded" or r.status_code == 200


async def coerce_local_model_if_needed(provider: str, model: str) -> str:
    """
    If the requested model is absent from the local catalog, use the first available id.
    For LM Studio, optionally call the load API when switching to a fallback id.
    """
    if not _env_flag("LOCAL_LLM_MODEL_FALLBACK", True):
        return model
    if provider == "ollama":
        base = ollama_api_base().strip()
        if not base:
            return model
        names = await _fetch_ollama_tag_names(base)
        if not names:
            return model
        if ollama_tag_matches(names, model):
            return model
        pick = names[0]
        logger.warning(
            "Ollama model %r not in local tags; falling back to %r",
            model,
            pick,
        )
        return pick

    if provider == "lm_studio":
        base = lm_studio_api_base().strip()
        if not base:
            return model
        ids = await _fetch_lm_studio_openai_ids(base)
        if not ids:
            ids = await _fetch_lm_studio_native_llm_keys(base)
        if not ids:
            return model
        want = (model or "").strip()
        if want in ids:
            if _env_flag("LM_STUDIO_TRY_LOAD_MODEL", True) and _env_flag(
                "LM_STUDIO_PRELOAD_MATCHED_MODEL", False
            ):
                await lm_studio_request_load_model(base, want)
            return want
        pick = ids[0]
        logger.warning(
            "LM Studio model %r not listed; falling back to %r",
            model,
            pick,
        )
        if _env_flag("LM_STUDIO_TRY_LOAD_MODEL", True):
            await lm_studio_request_load_model(base, pick)
        return pick

    return model
