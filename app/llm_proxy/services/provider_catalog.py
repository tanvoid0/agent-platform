"""Normalized provider catalog for config/admin/test surfaces."""

from __future__ import annotations

import os
from typing import Any

from fastapi import HTTPException

from llm_proxy.core.capabilities import modality_map
from llm_proxy.core.config_cache import load_config_yaml_dict, read_env_file_parsed, read_llm_proxy_ui_fallbacks
from llm_proxy.core.errors import LlmProxyError
from llm_proxy.core.provider_config import (
    PROVIDER_LABELS,
    SUPPORTED_PROVIDER_IDS,
    aimlapi_api_key,
    aimlapi_configured,
    aimlapi_openai_base,
    anthropic_api_key,
    anthropic_configured,
    anthropic_openai_base,
    anthropic_version_header,
    default_model_for_provider,
    first_configured_provider,
    gemini_api_key,
    gemini_configured,
    is_supported_provider,
    lm_studio_api_base,
    lm_studio_api_key,
    lm_studio_configured,
    ollama_api_base,
    ollama_configured,
    provider_configured,
)
from llm_proxy.services.upstream_http import get_with_retry

ProviderId = str

_LOCAL_PROVIDERS = frozenset({"ollama", "lm_studio"})
_DISCOVERY_DEFAULTS: dict[ProviderId, dict[str, Any]] = {
    "ollama": {
        "mode": "dynamic",
        "primary_source": "ollama_tags",
        "fallback_sources": ["config_aliases", "ui_fallback_models", "provider_default"],
    },
    "lm_studio": {
        "mode": "dynamic",
        "primary_source": "lm_studio_models",
        "fallback_sources": [
            "lm_studio_native_models",
            "config_aliases",
            "ui_fallback_models",
            "provider_default",
        ],
    },
    "aimlapi": {
        "mode": "dynamic",
        "primary_source": "upstream_models",
        "fallback_sources": ["config_aliases", "ui_fallback_models", "provider_default"],
    },
    "anthropic": {
        "mode": "dynamic",
        "primary_source": "upstream_models",
        "fallback_sources": ["config_aliases", "ui_fallback_models", "provider_default"],
    },
    "gemini": {
        "mode": "dynamic",
        "primary_source": "upstream_models",
        "fallback_sources": ["config_aliases", "ui_fallback_models", "provider_default"],
    },
}


def _dedupe(values: list[str]) -> list[str]:
    out: list[str] = []
    seen: set[str] = set()
    for value in values:
        item = str(value or "").strip()
        if not item or item in seen:
            continue
        seen.add(item)
        out.append(item)
    return out


def _default_provider_env_or_dotenv() -> str:
    from_file = (read_env_file_parsed().get("DEFAULT_PROVIDER") or "").strip().lower()
    if from_file:
        return from_file
    return os.environ.get("DEFAULT_PROVIDER", "").strip().lower()


def _default_model_env_or_dotenv() -> str:
    from_file = (read_env_file_parsed().get("DEFAULT_MODEL") or "").strip()
    if from_file:
        return from_file
    return os.environ.get("DEFAULT_MODEL", "").strip()


def _defaults_from_config(data: dict[str, Any]) -> tuple[str, str]:
    defaults = data.get("defaults") if isinstance(data.get("defaults"), dict) else {}
    provider = str(defaults.get("provider") or "").strip().lower()
    model = str(defaults.get("model") or "").strip()
    if not is_supported_provider(provider):
        provider = ""
    if provider and not provider_configured(provider):
        return "", ""
    return provider, model


def get_resolved_defaults() -> dict[str, str]:
    data = load_config_yaml_dict()
    config_provider, config_model = _defaults_from_config(data)
    default_provider = _default_provider_env_or_dotenv()
    default_model = _default_model_env_or_dotenv()
    provider = default_provider if is_supported_provider(default_provider) else config_provider
    model = config_model if provider == config_provider else ""
    if not is_supported_provider(provider) or not provider_configured(provider):
        provider = first_configured_provider()
        model = ""
    if default_model:
        model = default_model
    elif not model:
        model = default_model_for_provider(provider)
    return {"provider": provider, "model": model}


def get_persisted_defaults() -> dict[str, str]:
    data = load_config_yaml_dict()
    config_provider, config_model = _defaults_from_config(data)
    env = read_env_file_parsed()
    provider = str(env.get("DEFAULT_PROVIDER") or "").strip().lower() or config_provider
    if not is_supported_provider(provider):
        provider = ""
    model = str(env.get("DEFAULT_MODEL") or "").strip()
    if not model and provider == config_provider:
        model = config_model
    return {"provider": provider, "model": model}


def _model_aliases_by_provider() -> dict[str, list[str]]:
    data = load_config_yaml_dict()
    out: dict[str, list[str]] = {pid: [] for pid in SUPPORTED_PROVIDER_IDS}
    for block in data.get("providers") or []:
        if not isinstance(block, dict):
            continue
        provider = str(block.get("name") or "").strip().lower()
        if not is_supported_provider(provider):
            continue
        for item in block.get("models") or []:
            if isinstance(item, str) and item.strip():
                out[provider].append(item.strip())
            elif isinstance(item, dict):
                model_name = item.get("model_name")
                if isinstance(model_name, str) and model_name.strip():
                    out[provider].append(model_name.strip())
    for entry in data.get("model_list") or []:
        if not isinstance(entry, dict):
            continue
        provider = str(entry.get("provider") or "").strip().lower()
        model_name = entry.get("model_name")
        model = str(entry.get("model") or "").strip()
        if is_supported_provider(provider) and isinstance(model_name, str) and model_name.strip() and model:
            out[provider].append(model_name.strip())
    return {provider: _dedupe(values) for provider, values in out.items()}


async def _fetch_ollama_models() -> list[str] | None:
    base = ollama_api_base().rstrip("/")
    if not base:
        return None
    try:
        response = await get_with_retry(f"{base}/api/tags", timeout=12.0, context="provider_catalog_ollama")
    except LlmProxyError:
        return None
    if response.status_code != 200:
        return None
    try:
        payload = response.json()
    except ValueError:
        return None
    names: list[str] = []
    for item in payload.get("models") or []:
        if isinstance(item, dict):
            name = item.get("name")
            if isinstance(name, str) and name.strip():
                names.append(name.strip())
    return _dedupe(names) or None


async def _fetch_lm_studio_openai_models() -> list[str] | None:
    base = lm_studio_api_base().rstrip("/")
    if not base:
        return None
    headers: dict[str, str] = {}
    key = lm_studio_api_key()
    if key:
        headers["Authorization"] = f"Bearer {key}"
    try:
        response = await get_with_retry(
            f"{base}/v1/models",
            headers=headers,
            timeout=12.0,
            context="provider_catalog_lm_studio_openai",
        )
    except LlmProxyError:
        return None
    if response.status_code != 200:
        return None
    try:
        payload = response.json()
    except ValueError:
        return None
    names: list[str] = []
    for item in payload.get("data") or []:
        if isinstance(item, dict):
            model_id = item.get("id")
            if isinstance(model_id, str) and model_id.strip():
                names.append(model_id.strip())
    return _dedupe(names) or None


async def _fetch_lm_studio_native_models() -> list[str] | None:
    base = lm_studio_api_base().rstrip("/")
    if not base:
        return None
    headers: dict[str, str] = {}
    key = lm_studio_api_key()
    if key:
        headers["Authorization"] = f"Bearer {key}"
    try:
        response = await get_with_retry(
            f"{base}/api/v1/models",
            headers=headers,
            timeout=12.0,
            context="provider_catalog_lm_studio_native",
        )
    except LlmProxyError:
        return None
    if response.status_code != 200:
        return None
    try:
        payload = response.json()
    except ValueError:
        return None
    names: list[str] = []
    for item in payload.get("models") or []:
        if not isinstance(item, dict):
            continue
        if str(item.get("type") or "").strip().lower() != "llm":
            continue
        key_name = item.get("key")
        if isinstance(key_name, str) and key_name.strip():
            names.append(key_name.strip())
    return _dedupe(names) or None


async def _fetch_openai_model_ids(url: str, headers: dict[str, str], context: str) -> list[str] | None:
    try:
        response = await get_with_retry(url, headers=headers, timeout=15.0, context=context)
    except LlmProxyError:
        return None
    if response.status_code != 200:
        return None
    try:
        payload = response.json()
    except ValueError:
        return None
    names: list[str] = []
    for item in payload.get("data") or []:
        if isinstance(item, dict):
            model_id = item.get("id")
            if isinstance(model_id, str) and model_id.strip():
                names.append(model_id.strip())
    return _dedupe(names) or None


async def _fetch_gemini_models() -> list[str] | None:
    api_key = gemini_api_key().strip()
    if not api_key:
        return None
    try:
        response = await get_with_retry(
            "https://generativelanguage.googleapis.com/v1beta/models",
            params={"key": api_key},
            timeout=20.0,
            context="provider_catalog_gemini",
        )
    except LlmProxyError:
        return None
    if response.status_code != 200:
        return None
    try:
        payload = response.json()
    except ValueError:
        return None
    names: list[str] = []
    for item in payload.get("models") or []:
        if not isinstance(item, dict):
            continue
        name = item.get("name")
        if not isinstance(name, str) or not name.startswith("models/"):
            continue
        short_name = name.removeprefix("models/").strip()
        if not short_name or "embed" in short_name.lower():
            continue
        names.append(short_name)
    return _dedupe(names) or None


async def _fetch_provider_models(provider: str) -> tuple[list[str] | None, str | None]:
    if provider == "ollama":
        return await _fetch_ollama_models(), "ollama_tags"
    if provider == "lm_studio":
        models = await _fetch_lm_studio_openai_models()
        if models:
            return models, "lm_studio_models"
        models = await _fetch_lm_studio_native_models()
        if models:
            return models, "lm_studio_native_models"
        return None, None
    if provider == "aimlapi" and aimlapi_configured():
        return (
            await _fetch_openai_model_ids(
                f"{aimlapi_openai_base().rstrip('/')}/models",
                {"Authorization": f"Bearer {aimlapi_api_key()}"},
                "provider_catalog_aimlapi",
            ),
            "upstream_models",
        )
    if provider == "anthropic" and anthropic_configured():
        return (
            await _fetch_openai_model_ids(
                f"{anthropic_openai_base().rstrip('/')}/models",
                {
                    "x-api-key": anthropic_api_key(),
                    "anthropic-version": anthropic_version_header(),
                },
                "provider_catalog_anthropic",
            ),
            "upstream_models",
        )
    if provider == "gemini":
        return await _fetch_gemini_models(), "upstream_models"
    return None, None


def _fallback_models_for_provider(provider: str, aliases: list[str]) -> list[str]:
    fallback_map = read_llm_proxy_ui_fallbacks()
    values = list(fallback_map.get(provider, []))
    for alias in aliases:
        values.extend(fallback_map.get(alias, []))
    return _dedupe(values)


def _provider_capabilities(provider: str) -> dict[str, Any]:
    discovery = _DISCOVERY_DEFAULTS[provider]
    return {
        "streaming": True,
        "tools": provider != "gemini",
        "json_mode": True,
        "modalities": modality_map(provider),
        "model_discovery": {
            "mode": discovery["mode"],
            "primary_source": discovery["primary_source"],
            "fallback_sources": list(discovery["fallback_sources"]),
        },
    }


async def _fetch_ollama_tag_entries() -> tuple[list[dict[str, Any]], bool]:
    base = ollama_api_base().rstrip("/")
    if not base:
        return [], False
    reachable = False
    try:
        version = await get_with_retry(
            f"{base}/api/version",
            timeout=12.0,
            context="v1_catalog_ollama_version",
        )
        reachable = version.status_code == 200
    except LlmProxyError:
        return [], False
    if not reachable:
        return [], False
    try:
        response = await get_with_retry(
            f"{base}/api/tags",
            timeout=12.0,
            context="v1_catalog_ollama_tags",
        )
    except LlmProxyError:
        return [], reachable
    if response.status_code != 200:
        return [], reachable
    try:
        payload = response.json()
    except ValueError:
        return [], reachable
    rows: list[dict[str, Any]] = []
    for item in payload.get("models") or []:
        if not isinstance(item, dict):
            continue
        name = item.get("name")
        if not isinstance(name, str) or not name.strip():
            continue
        details = item.get("details") if isinstance(item.get("details"), dict) else {}
        metadata: dict[str, Any] = {}
        for key in ("family", "parameter_size", "quantization_level"):
            value = details.get(key)
            if value is not None:
                metadata[key] = value
        if item.get("size") is not None:
            metadata["size"] = item.get("size")
        rows.append({"id": name.strip(), "source": "live", "metadata": metadata})
    return rows, reachable


def _alias_model_rows(aliases: list[str]) -> list[dict[str, Any]]:
    return [{"id": alias, "source": "alias"} for alias in aliases]


async def build_v1_provider_catalog(
    *,
    allowed_providers: set[str] | None,
    include_live: bool = True,
) -> dict[str, Any]:
    """OpenAI-style provider registry for ``GET /v1/catalog``."""
    aliases_by_provider = _model_aliases_by_provider()
    resolved_defaults = get_resolved_defaults()
    provider_ids = list(SUPPORTED_PROVIDER_IDS)
    if allowed_providers is not None:
        provider_ids = [provider for provider in provider_ids if provider in allowed_providers]

    providers: list[dict[str, Any]] = []
    for provider in provider_ids:
        configured = provider_configured(provider)
        aliases = aliases_by_provider.get(provider, [])
        reachable: bool | None = False if not configured else None
        models: list[dict[str, Any]] = []
        default_model = default_model_for_provider(provider)

        if configured and include_live:
            if provider == "ollama":
                models, reachable = await _fetch_ollama_tag_entries()
            else:
                discovered, _source = await _fetch_provider_models(provider)
                reachable = bool(discovered)
                if discovered:
                    models = [{"id": model_id, "source": "live"} for model_id in discovered]

        if not models:
            models = _alias_model_rows(aliases)
            if configured and reachable is None:
                reachable = False

        if not models:
            fallback = _fallback_models_for_provider(provider, aliases)
            models = _alias_model_rows(fallback)
        if not models and default_model:
            models = [{"id": default_model, "source": "alias"}]

        if resolved_defaults["provider"] == provider and resolved_defaults["model"].strip():
            default_model = resolved_defaults["model"].strip()
        elif models:
            default_model = models[0]["id"]

        providers.append(
            {
                "id": provider,
                "label": PROVIDER_LABELS.get(provider, provider),
                "configured": configured,
                "reachable": reachable,
                "default_model": default_model,
                "capabilities": _provider_capabilities(provider),
                "models": models,
            }
        )

    return {
        "object": "catalog",
        "resolved_defaults": resolved_defaults,
        "providers": providers,
    }


async def build_provider_catalog(*, include_unconfigured: bool = True) -> dict[str, Any]:
    aliases_by_provider = _model_aliases_by_provider()
    resolved_defaults = get_resolved_defaults()
    persisted_defaults = get_persisted_defaults()
    providers: list[dict[str, Any]] = []

    for provider in SUPPORTED_PROVIDER_IDS:
        configured = provider_configured(provider)
        if not include_unconfigured and not configured:
            continue
        aliases = aliases_by_provider.get(provider, [])
        discovered, source = await _fetch_provider_models(provider) if configured else (None, None)
        model_source = source
        warning: str | None = None
        fallback_note: str | None = None
        models = discovered or []
        if not models:
            models = list(aliases)
            if models:
                model_source = "config_aliases"
                fallback_note = "Provider catalog unavailable; using config.yaml aliases."
        if not models:
            models = _fallback_models_for_provider(provider, aliases)
            if models:
                model_source = "ui_fallback_models"
                fallback_note = "Provider catalog unavailable; using configured UI fallback models."
        default_model = default_model_for_provider(provider)
        if not models and default_model:
            models = [default_model]
            model_source = "provider_default"
            fallback_note = "Provider catalog unavailable; using the provider default model."
        if configured and not discovered and not fallback_note:
            warning = "Provider did not return a model catalog; fallback values are being used."
        if not model_source:
            model_source = "unavailable"
        selected_model = default_model
        if resolved_defaults["provider"] == provider and resolved_defaults["model"].strip():
            selected_model = resolved_defaults["model"].strip()
        elif models:
            selected_model = models[0]
        ordered_models = _dedupe([selected_model, *models])
        providers.append(
            {
                "id": provider,
                "label": PROVIDER_LABELS.get(provider, provider),
                "configured": configured,
                "local": provider in _LOCAL_PROVIDERS,
                "capabilities": _provider_capabilities(provider),
                "models": {
                    "options": ordered_models,
                    "selected_model": selected_model,
                    "default_model": selected_model,
                    "source": model_source,
                    "warning": warning,
                    "fallback_note": fallback_note,
                },
            }
        )

    return {
        "persisted_defaults": persisted_defaults,
        "resolved_defaults": resolved_defaults,
        "providers": providers,
    }


async def get_provider_catalog_entry(provider_or_alias: str) -> dict[str, Any]:
    token = (provider_or_alias or "").strip()
    if is_supported_provider(token):
        provider_id = token.lower()
    else:
        aliases = _model_aliases_by_provider()
        provider_id = next(
            (provider for provider, values in aliases.items() if token in values),
            "",
        )
    if not provider_id:
        raise HTTPException(status_code=404, detail="Unknown provider")
    catalog = await build_provider_catalog(include_unconfigured=True)
    for entry in catalog["providers"]:
        if entry["id"] == provider_id:
            return entry
    raise HTTPException(status_code=404, detail="Unknown provider")
