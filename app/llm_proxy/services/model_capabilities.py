"""Per-model capability discovery and request guards.

Ollama exposes authoritative capability flags via ``POST /api/show`` (e.g.
``tools``, ``vision``, ``completion``). Results are persisted to disk and
treated as sticky: once a model is observed to support a capability it is not
downgraded on later probes (model capabilities do not change at runtime).
"""

from __future__ import annotations

import asyncio
import json
import os
import re
import threading
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from llm_proxy.core.capabilities import modality_map, provider_supports
from llm_proxy.core.errors import LlmProxyError
from llm_proxy.core.provider_config import ollama_api_base
from llm_proxy.services.upstream_http import post_with_retry
from platform_config import resolved_config_dir

CAPABILITY_KEYS = (
    "chat",
    "tools",
    "vision_input",
    "embeddings",
    "image_generation",
    "streaming",
)

_AUTHORITATIVE_SOURCES = frozenset({"ollama_show", "cached"})

_memory_cache: dict[str, dict[str, Any]] = {}
_cache_lock = asyncio.Lock()
_disk_lock = threading.Lock()
_disk_loaded = False

_OLLAMA_CAP_MAP = {
    "completion": "chat",
    "tools": "tools",
    "vision": "vision_input",
    "embedding": "embeddings",
    "embeddings": "embeddings",
}

_TOOL_NAME_HINTS = re.compile(
    r"(?:^|[/_-])(qwen3-coder|qwen2\.5-coder|qwen-coder|deepseek-coder|codestral|"
    r"devstral|mistral-nemo|llama3\.1|llama3\.2|llama3\.3|functionary|hermes-3|"
    r"firefunction|command-r)(?:[:\-_/]|$)",
    re.IGNORECASE,
)

_VISION_NAME_HINTS = re.compile(
    r"(?:^|[/_-])(llava|bakllava|moondream|minicpm-v|gemma3.*vision|vision|"
    r"llama3\.2-vision|qwen2-vl|qwen-vl)(?:[:\-_/]|$)",
    re.IGNORECASE,
)

_EMBED_NAME_HINTS = re.compile(
    r"(?:^|[/_-])(nomic-embed|mxbai-embed|bge-|embed|embedding|text-embedding)(?:[:\-_/]|$)",
    re.IGNORECASE,
)


def _cache_file_path() -> Path:
    return resolved_config_dir() / "model_capabilities.json"


def _cache_key(provider: str, model: str) -> str:
    return f"{provider.strip().lower()}::{model.strip().lower()}"


def _now_iso() -> str:
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat()


def provider_default_capabilities(provider: str) -> dict[str, Any]:
    """Best-effort defaults before a model-specific probe."""
    modalities = modality_map(provider)
    return {
        "chat": modalities.get("chat", True),
        "tools": provider_supports(provider, "chat") and provider != "gemini",
        "vision_input": modalities.get("vision_input", False),
        "embeddings": modalities.get("embeddings", False),
        "image_generation": modalities.get("image_generation", False),
        "streaming": True,
        "probe_source": "provider_default",
    }


def _empty_capabilities() -> dict[str, Any]:
    return {key: False for key in CAPABILITY_KEYS} | {
        "streaming": True,
        "probe_source": "unknown",
    }


def normalize_ollama_capabilities(raw: list[Any] | None) -> dict[str, Any]:
    caps = _empty_capabilities()
    caps["probe_source"] = "ollama_show"
    for item in raw or []:
        if not isinstance(item, str):
            continue
        mapped = _OLLAMA_CAP_MAP.get(item.strip().lower())
        if mapped:
            caps[mapped] = True
    if not any(caps[k] for k in ("chat", "tools", "vision_input", "embeddings")):
        caps["chat"] = True
    return caps


def infer_capabilities_from_model_name(model_id: str, provider: str) -> dict[str, Any]:
    caps = provider_default_capabilities(provider)
    caps["probe_source"] = "heuristic"
    name = model_id.strip().lower()
    if _TOOL_NAME_HINTS.search(name):
        caps["tools"] = True
    if _VISION_NAME_HINTS.search(name):
        caps["vision_input"] = True
    if _EMBED_NAME_HINTS.search(name):
        caps["embeddings"] = True
    return caps


def merge_capabilities(
    base: dict[str, Any],
    override: dict[str, Any] | None,
) -> dict[str, Any]:
    merged = dict(base)
    if not override:
        return merged
    for key in CAPABILITY_KEYS:
        if key in override:
            merged[key] = bool(override[key])
    source = override.get("probe_source")
    if isinstance(source, str) and source:
        merged["probe_source"] = source
    return merged


def merge_capabilities_sticky(
    previous: dict[str, Any] | None,
    fresh: dict[str, Any],
) -> dict[str, Any]:
    """Keep confirmed capabilities — positives never downgrade."""
    if not previous:
        return dict(fresh)
    merged = merge_capabilities(fresh, previous)
    for key in CAPABILITY_KEYS:
        if previous.get(key):
            merged[key] = True
    prev_source = previous.get("probe_source")
    fresh_source = fresh.get("probe_source")
    if prev_source in _AUTHORITATIVE_SOURCES:
        merged["probe_source"] = prev_source
    elif fresh_source in _AUTHORITATIVE_SOURCES:
        merged["probe_source"] = fresh_source
    probed_at = previous.get("probed_at")
    if isinstance(probed_at, str) and probed_at:
        merged["probed_at"] = probed_at
    return merged


def _entry_from_record(record: dict[str, Any]) -> dict[str, Any]:
    caps = record.get("capabilities")
    if isinstance(caps, dict):
        out = dict(caps)
        probed_at = record.get("probed_at")
        if isinstance(probed_at, str) and probed_at:
            out["probed_at"] = probed_at
        out["probe_source"] = out.get("probe_source") or "cached"
        return out
    return {}


def _load_disk_cache_unlocked() -> dict[str, dict[str, Any]]:
    path = _cache_file_path()
    if not path.is_file():
        return {}
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError, ValueError):
        return {}
    entries = payload.get("entries")
    if not isinstance(entries, dict):
        return {}
    out: dict[str, dict[str, Any]] = {}
    for key, record in entries.items():
        if isinstance(key, str) and isinstance(record, dict):
            out[key] = _entry_from_record(record)
    return out


def _ensure_disk_loaded() -> None:
    global _disk_loaded
    if _disk_loaded:
        return
    with _disk_lock:
        if _disk_loaded:
            return
        _memory_cache.update(_load_disk_cache_unlocked())
        _disk_loaded = True


def _persist_entry(key: str, capabilities: dict[str, Any]) -> None:
    path = _cache_file_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    with _disk_lock:
        entries: dict[str, Any]
        if path.is_file():
            try:
                payload = json.loads(path.read_text(encoding="utf-8"))
                entries = payload.get("entries") if isinstance(payload.get("entries"), dict) else {}
            except (OSError, json.JSONDecodeError, ValueError):
                entries = {}
        else:
            entries = {}
        stored_caps = {k: capabilities[k] for k in CAPABILITY_KEYS if k in capabilities}
        source = capabilities.get("probe_source")
        if isinstance(source, str) and source:
            stored_caps["probe_source"] = source
        entries[key] = {
            "capabilities": stored_caps,
            "probed_at": capabilities.get("probed_at") or _now_iso(),
        }
        path.write_text(
            json.dumps({"version": 1, "entries": entries}, ensure_ascii=False, indent=2),
            encoding="utf-8",
        )


def _should_skip_probe(cached: dict[str, Any] | None) -> bool:
    if not cached:
        return False
    if cached.get("probe_source") in _AUTHORITATIVE_SOURCES:
        return True
    probed_at = cached.get("probed_at")
    return isinstance(probed_at, str) and bool(probed_at)


async def _fetch_ollama_show_capabilities(base: str, model: str) -> dict[str, Any] | None:
    try:
        response = await post_with_retry(
            f"{base.rstrip('/')}/api/show",
            json_body={"name": model},
            timeout=10.0,
            context="v1_catalog_ollama_show",
        )
    except LlmProxyError:
        return None
    if response.status_code != 200:
        return None
    try:
        payload = response.json()
    except ValueError:
        return None
    raw = payload.get("capabilities")
    if not isinstance(raw, list):
        return None
    return normalize_ollama_capabilities(raw)


def _finalize_capabilities(
    key: str,
    previous: dict[str, Any] | None,
    fresh: dict[str, Any],
) -> dict[str, Any]:
    caps = merge_capabilities_sticky(previous, fresh)
    if "probed_at" not in caps or not caps.get("probed_at"):
        caps["probed_at"] = _now_iso()
    _memory_cache[key] = caps
    _persist_entry(key, caps)
    return dict(caps)


async def resolve_model_capabilities(
    provider: str,
    model_id: str,
    *,
    probe: bool = True,
    ollama_base: str | None = None,
) -> dict[str, Any]:
    """Resolve capabilities for a provider/model pair (memory + disk cache)."""
    prov = (provider or "").strip().lower()
    model = (model_id or "").strip()
    if not prov or not model:
        return provider_default_capabilities(prov or "ollama")

    _ensure_disk_loaded()
    key = _cache_key(prov, model)

    async with _cache_lock:
        previous = _memory_cache.get(key)
        if _should_skip_probe(previous):
            return dict(previous)

    fresh = provider_default_capabilities(prov)
    if probe:
        if prov == "ollama":
            base = (ollama_base or ollama_api_base()).strip()
            if base:
                shown = await _fetch_ollama_show_capabilities(base, model)
                if shown is not None:
                    fresh = merge_capabilities(fresh, shown)
                else:
                    fresh = infer_capabilities_from_model_name(model, prov)
            else:
                fresh = infer_capabilities_from_model_name(model, prov)
        else:
            fresh = infer_capabilities_from_model_name(model, prov)
    else:
        fresh = infer_capabilities_from_model_name(model, prov)

    async with _cache_lock:
        previous = _memory_cache.get(key)
        return _finalize_capabilities(key, previous, fresh)


async def enrich_model_rows(
    provider: str,
    rows: list[dict[str, Any]],
    *,
    probe: bool = True,
    ollama_base: str | None = None,
) -> None:
    """Attach ``capabilities`` to catalog model rows in place."""
    if not rows:
        return
    _ensure_disk_loaded()
    sem = asyncio.Semaphore(4)

    async def attach(row: dict[str, Any]) -> None:
        model_id = str(row.get("id") or "").strip()
        if not model_id:
            row["capabilities"] = provider_default_capabilities(provider)
            return
        key = _cache_key(provider, model_id)
        cached = _memory_cache.get(key)
        if _should_skip_probe(cached):
            row["capabilities"] = dict(cached)
            return
        async with sem:
            row["capabilities"] = await resolve_model_capabilities(
                provider,
                model_id,
                probe=probe and row.get("source") == "live",
                ollama_base=ollama_base,
            )

    await asyncio.gather(*(attach(row) for row in rows))


def request_uses_tools(body: dict[str, Any]) -> bool:
    tools = body.get("tools")
    if isinstance(tools, list) and len(tools) > 0:
        return True
    tool_choice = body.get("tool_choice")
    if tool_choice is None:
        return False
    if isinstance(tool_choice, str):
        return tool_choice.strip().lower() not in ("", "none")
    return True


def messages_contain_vision(messages: Any) -> bool:
    if not isinstance(messages, list):
        return False
    for message in messages:
        if not isinstance(message, dict):
            continue
        content = message.get("content")
        if isinstance(content, list):
            for part in content:
                if isinstance(part, dict) and part.get("type") == "image_url":
                    return True
        elif isinstance(content, str) and "data:image/" in content:
            return True
    return False


def _capability_label(capability: str) -> str:
    return {
        "tools": "tool / function calling",
        "vision_input": "vision (image input)",
        "embeddings": "embeddings",
        "image_generation": "image generation",
        "chat": "chat",
    }.get(capability, capability.replace("_", " "))


def require_model_capability(
    provider: str,
    model: str,
    capabilities: dict[str, Any],
    capability: str,
) -> None:
    if capabilities.get(capability):
        return
    label = _capability_label(capability)
    source = capabilities.get("probe_source") or "unknown"
    raise LlmProxyError(
        501,
        "capability_unavailable",
        (
            f"Model '{model}' on {provider} does not support {label}. "
            f"Choose a model that supports this operation (catalog probe_source={source}). "
            f"For Ollama coding models like qwen3-coder:30b, run `ollama show <model>` and "
            f"confirm Capabilities includes 'tools' (requires Ollama 0.12+ and RENDERER/PARSER qwen3-coder in the Modelfile)."
        ),
        extra={
            "capability": capability,
            "provider": provider,
            "model": model,
            "model_capabilities": {k: capabilities.get(k) for k in CAPABILITY_KEYS},
            "probe_source": source,
        },
    )


async def ensure_chat_request_supported(
    provider: str,
    model: str,
    body: dict[str, Any],
    *,
    probe: bool = True,
) -> None:
    """Validate a chat/completions body against resolved model capabilities."""
    caps = await resolve_model_capabilities(provider, model, probe=probe)
    if request_uses_tools(body):
        require_model_capability(provider, model, caps, "tools")
    if messages_contain_vision(body.get("messages")):
        require_model_capability(provider, model, caps, "vision_input")


def clear_capability_cache(*, disk: bool = False) -> None:
    """Clear in-memory cache; optionally remove persisted entries (tests/admin)."""
    global _disk_loaded
    _memory_cache.clear()
    _disk_loaded = False
    if disk:
        path = _cache_file_path()
        with _disk_lock:
            if path.is_file():
                path.unlink(missing_ok=True)
