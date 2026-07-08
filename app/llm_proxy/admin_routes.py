"""JSON API for embedded LLM proxy configuration (Flow UI and REST)."""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any

import httpx
import yaml
from jsonschema import Draft202012Validator, ValidationError
from fastapi import APIRouter, HTTPException, Request
from fastapi.responses import Response, StreamingResponse
from pydantic import BaseModel, Field

from llm_proxy.core.config_cache import (
    env_file_path,
    load_config_yaml_dict,
    read_env_file_parsed,
    read_llm_proxy_ui_fallbacks,
    resolved_config_yaml_path,
)
from llm_proxy.core.errors import LlmProxyError
from llm_proxy.core.provider_config import (
    DEFAULT_LM_STUDIO_BASE,
    DEFAULT_OLLAMA_BASE,
    PROVIDER_LOCAL_SORT_ORDER,
    aimlapi_api_key,
    aimlapi_configured,
    aimlapi_openai_base,
    gemini_api_key,
    gemini_configured,
    lm_studio_api_base,
    lm_studio_api_key,
    lm_studio_configured,
    ollama_api_base,
    ollama_configured,
    provider_configured,
    is_supported_provider,
)
from llm_proxy.services.provider_catalog import (
    build_provider_catalog,
    get_persisted_defaults,
    get_provider_catalog_entry,
    get_resolved_defaults,
)
from llm_proxy.services.upstream_http import (
    aclose_stream,
    classify_httpx_error,
    get_with_retry,
    post_with_retry,
    sse_error_chunk,
    stream_chat_completion,
)

CONFIG_DIR = Path(os.environ.get("CONFIG_DIR", "/data"))
PROXY_INTERNAL_URL = os.environ.get("ORCHESTRATOR_INTERNAL_URL", "http://127.0.0.1:18410").rstrip("/")

MASTER_KEY_ENV = "AGENT_PLATFORM_MASTER_KEY"

ENV_KEYS = [
    MASTER_KEY_ENV,
    "GEMINI_API_KEY",
    "AIMLAPI_API_KEY",
    "AIMLAPI_OPENAI_BASE",
    "OLLAMA_API_BASE",
    "LM_STUDIO_API_BASE",
    "LM_STUDIO_API_KEY",
    "DEFAULT_PROVIDER",
    "DEFAULT_MODEL",
]

router = APIRouter(tags=["llm-proxy-admin"])


def _mask_secret(value: str) -> str:
    if not value:
        return ""
    if len(value) <= 4:
        return "****"
    return "****" + value[-4:]


# Keys whose values are secrets — only these are masked in GET /env; others return plaintext ``value``.
_SENSITIVE_ENV_KEYS = frozenset(
    {MASTER_KEY_ENV, "GEMINI_API_KEY", "AIMLAPI_API_KEY", "LM_STUDIO_API_KEY"}
)


def _write_env_file(values: dict[str, str]) -> None:
    lines = [
        "# Generated / updated by Agent Platform (LLM proxy settings). Do not commit to git.",
        "",
    ]
    for k in ENV_KEYS:
        v = values.get(k, "")
        if any(ch in v for ch in " \n\t\"'\\"):
            escaped = v.replace("\\", "\\\\").replace('"', '\\"')
            lines.append(f'{k}="{escaped}"')
        else:
            lines.append(f"{k}={v}")
    lines.append("")
    ep = env_file_path()
    ep.parent.mkdir(parents=True, exist_ok=True)
    ep.write_text("\n".join(lines), encoding="utf-8")


def master_key_from_env(env: dict[str, str]) -> str:
    return (env.get(MASTER_KEY_ENV) or "").strip()


def _config_yaml_defaults() -> dict[str, str]:
    data = load_config_yaml_dict()
    d = data.get("defaults") if isinstance(data.get("defaults"), dict) else {}
    p = str(d.get("provider") or "").strip().lower()
    m = str(d.get("model") or "").strip()
    if not is_supported_provider(p):
        p = ""
    if p and not provider_configured(p):
        p = ""
        m = ""
    return {"provider": p, "model": m}


def _pick_default_model_from_list(models: list[str], env: dict[str, str]) -> str | None:
    if not models:
        return None
    dm = (env.get("DEFAULT_MODEL") or "").strip()
    if dm and dm in models:
        return dm
    cd = _config_yaml_defaults()
    cm = (cd.get("model") or "").strip()
    if cm and cm in models:
        return cm
    return models[0]


def _config_schema_path() -> Path | None:
    candidates = [
        CONFIG_DIR / "config.schema.json",
        Path(__file__).resolve().parent / "config.schema.json",
        Path(__file__).resolve().parent.parent / "config.schema.json",
    ]
    for p in candidates:
        if p.is_file():
            return p
    return None


def _validate_config_dict(data: Any) -> None:
    if data is None:
        data = {}
    if not isinstance(data, dict):
        raise ValueError("config root must be a mapping")
    path = _config_schema_path()
    if path is None:
        return
    schema = json.loads(path.read_text(encoding="utf-8"))
    Draft202012Validator(schema).validate(data)


def _infer_provider_kind(backend_model: str) -> str:
    m = (backend_model or "").strip().lower()
    if m.startswith("ollama/") or m.startswith("ollama_chat/"):
        return "ollama"
    if m.startswith("lm_studio/") or m.startswith("lmstudio/"):
        return "lm_studio"
    if m.startswith("gemini") or m.startswith("gemini/"):
        return "gemini"
    if m.startswith("aimlapi/") or m.startswith("openai/"):
        return "aimlapi"
    return "other"


def _model_names_from_config_yaml() -> list[str]:
    data = load_config_yaml_dict()
    out: list[str] = []
    for block in data.get("providers") or []:
        if not isinstance(block, dict):
            continue
        prov = str(block.get("name") or "").strip().lower()
        if not is_supported_provider(prov):
            continue
        if not provider_configured(prov):
            continue
        for item in block.get("models") or []:
            if isinstance(item, str) and item.strip():
                out.append(item.strip())
            elif isinstance(item, dict):
                mn = item.get("model_name")
                if isinstance(mn, str) and mn.strip():
                    out.append(mn.strip())
    for entry in data.get("model_list") or []:
        if not isinstance(entry, dict):
            continue
        name = entry.get("model_name")
        if not isinstance(name, str) or not name.strip():
            continue
        prov = str(entry.get("provider") or "").strip().lower()
        backend = str(entry.get("model") or "").strip()
        if is_supported_provider(prov) and backend:
            kind = prov
        else:
            lp = entry.get("litellm_params") if isinstance(entry.get("litellm_params"), dict) else {}
            litellm_model = lp.get("model") if isinstance(lp, dict) else None
            lm = str(litellm_model or "")
            kind = _infer_provider_kind(lm)
        if is_supported_provider(kind) and not provider_configured(kind):
            continue
        out.append(name.strip())
    return out


def _model_list_entries() -> list[dict[str, Any]]:
    data = load_config_yaml_dict()
    out: list[dict[str, Any]] = []
    for block in data.get("providers") or []:
        if not isinstance(block, dict):
            continue
        prov = str(block.get("name") or "").strip().lower()
        if not is_supported_provider(prov):
            continue
        for item in block.get("models") or []:
            if isinstance(item, str) and item.strip():
                alias = item.strip()
                out.append(
                    {
                        "id": alias,
                        "label": alias,
                        "kind": prov,
                        "litellm_model": alias,
                    }
                )
            elif isinstance(item, dict):
                name = item.get("model_name")
                backend = item.get("model")
                if (
                    isinstance(name, str)
                    and name.strip()
                    and isinstance(backend, str)
                    and backend.strip()
                ):
                    out.append(
                        {
                            "id": name.strip(),
                            "label": name.strip(),
                            "kind": prov,
                            "litellm_model": backend.strip(),
                        }
                    )
    for entry in data.get("model_list") or []:
        if not isinstance(entry, dict):
            continue
        name = entry.get("model_name")
        if not isinstance(name, str) or not name.strip():
            continue
        prov = str(entry.get("provider") or "").strip().lower()
        backend = str(entry.get("model") or "").strip()
        if is_supported_provider(prov) and backend:
            kind = prov
            lm = backend
        else:
            lp = entry.get("litellm_params") if isinstance(entry.get("litellm_params"), dict) else {}
            litellm_model = lp.get("model") if isinstance(lp, dict) else None
            lm = str(litellm_model or "")
            kind = _infer_provider_kind(lm)
        out.append(
            {
                "id": name.strip(),
                "label": name.strip(),
                "kind": kind,
                "litellm_model": lm,
            }
        )
    out = [e for e in out if provider_configured(str(e.get("kind") or "other"))]
    return out


def _provider_local_sort_order(kind: str) -> int:
    k = (kind or "").strip().lower()
    return PROVIDER_LOCAL_SORT_ORDER.get(k, len(PROVIDER_LOCAL_SORT_ORDER))


def _sort_providers_local_first(entries: list[dict[str, Any]]) -> list[dict[str, Any]]:
    tagged = [(i, e) for i, e in enumerate(entries)]
    tagged.sort(
        key=lambda p: (_provider_local_sort_order(str(p[1].get("kind") or "other")), p[0])
    )
    return [e for _, e in tagged]


async def _fetch_ollama_tags(api_base: str) -> list[str] | None:
    base = api_base.strip().rstrip("/")
    if not base:
        return None
    url = f"{base}/api/tags"
    try:
        r = await get_with_retry(url, timeout=15.0, context="ollama_tags")
    except LlmProxyError:
        return None
    if r.status_code != 200:
        return None
    try:
        payload = r.json()
    except ValueError:
        return None
    models = payload.get("models") if isinstance(payload, dict) else None
    if not isinstance(models, list):
        return None
    names: list[str] = []
    for item in models:
        if isinstance(item, dict):
            mid = item.get("name")
            if isinstance(mid, str) and mid.strip():
                names.append(mid.strip())
    return names or None


async def _fetch_gemini_remote_models(api_key: str) -> list[str] | None:
    if not api_key.strip():
        return None
    url = "https://generativelanguage.googleapis.com/v1beta/models"
    try:
        r = await get_with_retry(
            url,
            params={"key": api_key.strip()},
            timeout=30.0,
            context="gemini_models",
        )
    except LlmProxyError:
        return None
    if r.status_code != 200:
        return None
    try:
        payload = r.json()
    except ValueError:
        return None
    models = payload.get("models") if isinstance(payload, dict) else None
    if not isinstance(models, list):
        return None
    names: list[str] = []
    for item in models:
        if not isinstance(item, dict):
            continue
        name = item.get("name")
        if not isinstance(name, str) or not name.startswith("models/"):
            continue
        short = name.removeprefix("models/")
        if "embed" in short.lower() or "embedding" in short.lower():
            continue
        if short.strip():
            names.append(short.strip())
    return names or None


async def _fetch_lm_studio_openai_models(api_base: str) -> list[str] | None:
    base = api_base.strip().rstrip("/")
    if not base:
        return None
    url = f"{base}/v1/models"
    headers: dict[str, str] = {}
    key = lm_studio_api_key()
    if key:
        headers["Authorization"] = f"Bearer {key}"
    try:
        r = await get_with_retry(
            url,
            headers=headers,
            timeout=15.0,
            context="lm_studio_models",
        )
    except LlmProxyError:
        return None
    if r.status_code != 200:
        return None
    try:
        payload = r.json()
    except ValueError:
        return None
    data = payload.get("data") if isinstance(payload, dict) else None
    if not isinstance(data, list):
        return None
    names: list[str] = []
    for item in data:
        if isinstance(item, dict):
            mid = item.get("id")
            if isinstance(mid, str) and mid.strip():
                names.append(mid.strip())
    return names or None


async def _fetch_proxy_v1_models(
    master: str,
    *,
    provider: str | None = None,
) -> list[str] | None:
    if not master.strip():
        return None
    headers = {"Authorization": f"Bearer {master.strip()}"}
    url = f"{PROXY_INTERNAL_URL}/v1/models"
    if provider and is_supported_provider(provider):
        url = f"{url}?providers={provider}"
    try:
        r = await get_with_retry(
            url,
            headers=headers,
            timeout=30.0,
            context="proxy_v1_models",
        )
    except LlmProxyError:
        return None
    if r.status_code != 200:
        return None
    try:
        payload = r.json()
    except ValueError:
        return None
    models: list[str] = []
    for item in payload.get("data") or []:
        if isinstance(item, dict):
            mid = item.get("id")
            if isinstance(mid, str) and mid.strip():
                models.append(mid.strip())
    return models or None


class EnvUpdate(BaseModel):
    AGENT_PLATFORM_MASTER_KEY: str | None = None
    GEMINI_API_KEY: str | None = None
    AIMLAPI_API_KEY: str | None = None
    AIMLAPI_OPENAI_BASE: str | None = None
    OLLAMA_API_BASE: str | None = None
    LM_STUDIO_API_BASE: str | None = None
    LM_STUDIO_API_KEY: str | None = None
    DEFAULT_PROVIDER: str | None = None
    DEFAULT_MODEL: str | None = None


class ConfigYamlBody(BaseModel):
    content: str = Field(..., min_length=1)


class ProxyTestBody(BaseModel):
    model: str = Field(..., min_length=1)
    message: str = "Say OK in one word."
    system: str | None = None
    thinking: bool = False
    messages: list[dict[str, Any]] | None = None
    tools: list[dict[str, Any]] | None = None
    tool_choice: str | dict[str, Any] | None = None
    max_tokens: int | None = Field(default=None, ge=1, le=128000)


class EmbeddingTestBody(BaseModel):
    model: str = Field(..., min_length=1)
    input: str = "hello"


def _public_base() -> str:
    return os.environ.get("PROXY_PUBLIC_URL", "http://127.0.0.1:18410").rstrip("/")


@router.get("/snippet")
async def integration_snippet() -> dict[str, str]:
    """OpenAI-compatible client wiring (no secrets)."""
    env = read_env_file_parsed()
    master = master_key_from_env(env)
    public_base = _public_base()
    snippet = (
        f"export OPENAI_BASE_URL={public_base}/v1\n"
        f'export OPENAI_API_KEY="{master}"\n'
        "# model: config.yaml aliases; raw Ollama tags or LM Studio ids (defaults.provider)"
    )
    return {"public_base": public_base, "snippet": snippet}


@router.get("/env")
async def api_env() -> dict:
    env = read_env_file_parsed()
    out: dict[str, object] = {}
    for k in ENV_KEYS:
        val = env.get(k, "")
        if k in _SENSITIVE_ENV_KEYS:
            out[k] = {"set": bool(val), "masked": _mask_secret(val) if val else ""}
        else:
            out[k] = {"set": bool(val), "value": val}
    return {
        "keys": out,
        "effective_defaults": {
            "OLLAMA_API_BASE": DEFAULT_OLLAMA_BASE,
            "LM_STUDIO_API_BASE": DEFAULT_LM_STUDIO_BASE,
            "AIMLAPI_OPENAI_BASE": "https://api.aimlapi.com/v1",
        },
        "persisted_defaults": get_persisted_defaults(),
        "resolved_defaults": get_resolved_defaults(),
    }


@router.post("/env")
async def api_env_update(body: EnvUpdate) -> dict:
    existing = read_env_file_parsed()
    merged = dict(existing)
    for k, v in body.model_dump(exclude_none=True).items():
        if k not in ENV_KEYS:
            continue
        if k in ("GEMINI_API_KEY", "AIMLAPI_API_KEY", "LM_STUDIO_API_KEY", MASTER_KEY_ENV) and (
            v is None or (isinstance(v, str) and v.strip() == "")
        ):
            continue
        merged[k] = v.strip() if isinstance(v, str) else v
    out = {k: merged.get(k, "") for k in ENV_KEYS}
    _write_env_file(out)
    return {
        "ok": True,
        "message": "Saved .env. Restart the Agent Platform process (or container) to apply env-based auth and providers.",
    }


@router.get("/config-yaml")
async def api_get_yaml() -> dict:
    yp = resolved_config_yaml_path()
    if not yp.is_file():
        raise HTTPException(status_code=404, detail="config.yaml not found")
    return {"content": yp.read_text(encoding="utf-8")}


@router.post("/config-yaml")
async def api_post_yaml(body: ConfigYamlBody) -> dict:
    try:
        parsed = yaml.safe_load(body.content)
    except yaml.YAMLError as e:
        raise HTTPException(status_code=400, detail=f"Invalid YAML: {e}") from e
    if parsed is None:
        parsed = {}
    try:
        _validate_config_dict(parsed)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
    except ValidationError as e:
        raise HTTPException(status_code=400, detail=f"Config schema: {e.message}") from e
    yp = resolved_config_yaml_path()
    yp.parent.mkdir(parents=True, exist_ok=True)
    yp.write_text(body.content, encoding="utf-8")
    return {"ok": True, "message": "Saved config.yaml. Restart the Agent Platform process if your deployment caches YAML."}


@router.get("/health-proxy")
async def api_health_proxy() -> dict:
    r = await get_with_retry(
        f"{PROXY_INTERNAL_URL}/v1/health",
        timeout=10.0,
        context="health_proxy",
    )
    return {"status_code": r.status_code, "body": r.text[:2000]}


@router.get("/health-readiness")
async def api_health_readiness() -> dict:
    r = await get_with_retry(
        f"{PROXY_INTERNAL_URL}/v1/health/readiness",
        timeout=10.0,
        context="health_readiness",
    )
    return {"status_code": r.status_code, "body": r.text[:2000]}


@router.get("/ui/providers")
async def api_ui_providers() -> dict:
    return await build_provider_catalog(include_unconfigured=True)


@router.get("/ui/env-model-options")
async def api_ui_env_model_options() -> dict:
    return {"models": _model_names_from_config_yaml()}


@router.get("/ui/providers/{provider_or_alias}/models")
async def api_ui_provider_models(provider_or_alias: str) -> dict:
    entry = await get_provider_catalog_entry(provider_or_alias)
    return {
        "provider": entry["id"],
        "label": entry["label"],
        "configured": entry["configured"],
        "capabilities": entry["capabilities"],
        "models": entry["models"]["options"],
        "default": entry["models"]["default_model"],
        "source": entry["models"]["source"],
        "warning": entry["models"].get("warning"),
        "fallback_note": entry["models"].get("fallback_note"),
    }


@router.get("/proxy/models")
async def api_proxy_models(request: Request) -> dict:
    env = read_env_file_parsed()
    master = master_key_from_env(env)
    headers = {"Authorization": f"Bearer {master}"}
    q = request.url.query
    path = f"{PROXY_INTERNAL_URL}/v1/models"
    if q:
        path = f"{path}?{q}"
    r = await get_with_retry(
        path,
        headers=headers,
        timeout=30.0,
        context="proxy_models",
    )
    return {"status_code": r.status_code, "body": r.text[:64000]}


@router.get("/test/model-options")
async def api_test_model_options(provider: str | None = None) -> dict:
    catalog = await build_provider_catalog(include_unconfigured=True)
    selected_provider = (provider or catalog["resolved_defaults"]["provider"]).strip().lower()
    if not is_supported_provider(selected_provider):
        selected_provider = catalog["resolved_defaults"]["provider"]
    entry = next((item for item in catalog["providers"] if item["id"] == selected_provider), None)
    if entry is None:
        raise HTTPException(status_code=404, detail="Unknown provider")
    return {
        "source": entry["models"]["source"],
        "models": entry["models"]["options"],
        "default": entry["models"]["default_model"],
        "warning": entry["models"].get("warning"),
        "fallback_note": entry["models"].get("fallback_note"),
        "selected_provider": entry["id"],
        "resolved_defaults": catalog["resolved_defaults"],
        "persisted_defaults": catalog["persisted_defaults"],
        "providers": [
            {
                "id": item["id"],
                "label": item["label"],
                "configured": item["configured"],
                "local": item["local"],
                "capabilities": item["capabilities"],
            }
            for item in catalog["providers"]
        ],
    }


def _test_chat_messages(body: ProxyTestBody) -> list[dict[str, Any]]:
    if body.messages is not None:
        return list(body.messages)
    system_raw = (body.system or "").strip()
    if body.thinking:
        if system_raw:
            if not system_raw.startswith("<|think|>"):
                system_raw = "<|think|>" + system_raw
        else:
            system_raw = "<|think|>"
    messages: list[dict[str, Any]] = []
    if system_raw:
        messages.append({"role": "system", "content": system_raw})
    messages.append({"role": "user", "content": body.message})
    return messages


def _test_chat_max_tokens(body: ProxyTestBody, *, stream: bool) -> int:
    if body.max_tokens is not None:
        return body.max_tokens
    advanced = bool(
        body.messages is not None
        or body.tools
        or body.thinking
        or (body.system and body.system.strip())
    )
    if stream:
        return 512
    if advanced:
        return 512
    return 32


def _test_chat_payload(body: ProxyTestBody, *, stream: bool) -> dict[str, Any]:
    payload: dict[str, Any] = {
        "model": body.model,
        "messages": _test_chat_messages(body),
        "max_tokens": _test_chat_max_tokens(body, stream=stream),
        "stream": stream,
    }
    if body.tools:
        payload["tools"] = body.tools
    if body.tool_choice is not None:
        payload["tool_choice"] = body.tool_choice
    elif body.tools:
        payload["tool_choice"] = "auto"
    return payload


@router.post("/test-chat")
async def api_test_chat(body: ProxyTestBody) -> dict:
    env = read_env_file_parsed()
    master = master_key_from_env(env)
    payload = _test_chat_payload(body, stream=False)
    headers = {"Authorization": f"Bearer {master}", "Content-Type": "application/json"}
    r = await post_with_retry(
        f"{PROXY_INTERNAL_URL}/v1/chat/completions",
        headers=headers,
        json_body=payload,
        timeout=120.0,
        context="test_chat",
    )
    return {
        "status_code": r.status_code,
        "body": r.text[:16000],
    }


@router.post("/test-chat-stream", response_model=None)
async def api_test_chat_stream(body: ProxyTestBody) -> Response | StreamingResponse:
    env = read_env_file_parsed()
    master = master_key_from_env(env)
    payload = _test_chat_payload(body, stream=True)
    headers = {"Authorization": f"Bearer {master}", "Content-Type": "application/json"}
    url = f"{PROXY_INTERNAL_URL}/v1/chat/completions"

    response, client = await stream_chat_completion(
        url,
        headers=headers,
        json_body=payload,
        timeout=120.0,
        context="test_chat_stream",
    )
    if response.status_code >= 400:
        err_body = await response.aread()
        await aclose_stream(response, client)
        return Response(
            content=err_body,
            status_code=response.status_code,
            media_type=response.headers.get("content-type", "application/json"),
        )

    async def byte_stream():
        try:
            async for chunk in response.aiter_bytes():
                yield chunk
        except httpx.RequestError as e:
            code, msg = classify_httpx_error(e, "test_chat_stream")
            yield sse_error_chunk(code, msg)
        finally:
            await aclose_stream(response, client)

    return StreamingResponse(byte_stream(), media_type="text/event-stream")


@router.post("/test-embeddings")
async def api_test_embeddings(body: EmbeddingTestBody) -> dict:
    env = read_env_file_parsed()
    master = master_key_from_env(env)
    payload = {"model": body.model, "input": body.input}
    headers = {"Authorization": f"Bearer {master}", "Content-Type": "application/json"}
    r = await post_with_retry(
        f"{PROXY_INTERNAL_URL}/v1/embeddings",
        headers=headers,
        json_body=payload,
        timeout=60.0,
        context="test_embeddings",
    )
    return {"status_code": r.status_code, "body": r.text[:16000]}
