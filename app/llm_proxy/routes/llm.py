"""
OpenAI-compatible LLM routes (Ollama, LM Studio, Gemini).

Mounted on the main FastAPI app alongside the HTML config UI.
"""

from __future__ import annotations

import json
import os
import time
from dataclasses import dataclass
from typing import Annotated, Any, Awaitable, Callable

import httpx
import yaml
from fastapi import APIRouter, HTTPException, Query, Request, Response
from fastapi.responses import JSONResponse, StreamingResponse

from llm_proxy.core.config_cache import load_config_yaml_dict, read_env_file_parsed
from llm_proxy.core.errors import LlmProxyError
from llm_proxy.core.provider_config import (
    aimlapi_api_key,
    aimlapi_configured,
    aimlapi_openai_base,
    default_model_for_provider,
    first_configured_provider,
    gemini_api_key,
    lm_studio_api_base,
    lm_studio_api_key,
    lm_studio_configured,
    ollama_api_base,
    ollama_configured,
    provider_configured,
    is_supported_provider,
)
from llm_proxy.services.local_backends import coerce_local_model_if_needed
from llm_proxy.services.model_catalog_cache import get_catalog_cache
from llm_proxy.services.upstream_http import (
    aclose_stream,
    classify_httpx_error,
    get_with_retry,
    post_with_retry,
    sse_error_chunk,
    stream_chat_completion,
)

router = APIRouter(tags=["llm"])


GEMINI_OPENAI_BASE = os.environ.get(
    "GEMINI_OPENAI_BASE", "https://generativelanguage.googleapis.com/v1beta/openai"
).rstrip("/")

def _master_key() -> str:
    return os.environ.get("AGENT_PLATFORM_MASTER_KEY", "").strip()


def _default_provider_env_or_dotenv() -> str:
    raw = os.environ.get("DEFAULT_PROVIDER", "").strip().lower()
    if raw:
        return raw
    return (read_env_file_parsed().get("DEFAULT_PROVIDER") or "").strip().lower()


def _default_model_env_or_dotenv() -> str:
    raw = os.environ.get("DEFAULT_MODEL", "").strip()
    if raw:
        return raw
    return (read_env_file_parsed().get("DEFAULT_MODEL") or "").strip()


def _load_yaml() -> dict[str, Any]:
    return load_config_yaml_dict()


def _defaults_from_config(data: dict[str, Any]) -> tuple[str, str]:
    d = data.get("defaults") if isinstance(data.get("defaults"), dict) else {}
    p = str(d.get("provider") or "").strip().lower()
    m = str(d.get("model") or "").strip()
    if not is_supported_provider(p):
        p = ""
    if not m:
        m = ""
    return p, m


def _alias_map_raw(data: dict[str, Any]) -> dict[str, tuple[str, str]]:
    out: dict[str, tuple[str, str]] = {}
    for block in data.get("providers") or []:
        if not isinstance(block, dict):
            continue
        prov = str(block.get("name") or "").strip().lower()
        if not is_supported_provider(prov):
            continue
        for item in block.get("models") or []:
            if isinstance(item, str):
                s = item.strip()
                if s:
                    out[s] = (prov, s)
            elif isinstance(item, dict):
                name = item.get("model_name")
                mod = item.get("model")
                if (
                    isinstance(name, str)
                    and name.strip()
                    and isinstance(mod, str)
                    and mod.strip()
                ):
                    out[name.strip()] = (prov, mod.strip())
    for entry in data.get("model_list") or []:
        if not isinstance(entry, dict):
            continue
        name = entry.get("model_name")
        if not isinstance(name, str) or not name.strip():
            continue
        prov = str(entry.get("provider") or "").strip().lower()
        mod = str(entry.get("model") or "").strip()
        if not is_supported_provider(prov) or not mod:
            continue
        out[name.strip()] = (prov, mod)
    return out


def _alias_map(data: dict[str, Any]) -> dict[str, tuple[str, str]]:
    raw = _alias_map_raw(data)
    return {k: v for k, v in raw.items() if provider_configured(v[0])}


def _effective_defaults() -> tuple[str, str]:
    data = _load_yaml()
    config_provider, config_model = _defaults_from_config(data)
    default_provider = _default_provider_env_or_dotenv()
    default_model = _default_model_env_or_dotenv()
    p = (
        default_provider
        if is_supported_provider(default_provider)
        else config_provider
    )
    m = config_model if p == config_provider else ""
    if not is_supported_provider(p):
        p = ""
    if not is_supported_provider(p):
        p = first_configured_provider()
        m = ""
    elif not provider_configured(p):
        p = first_configured_provider()
        m = ""
    dm = default_model.strip()
    if dm:
        m = dm
    elif not m:
        m = default_model_for_provider(p)
    return p, m


def get_resolved_proxy_defaults() -> dict[str, str]:
    """Provider + model the embedded proxy would use for an unqualified request (for Settings UI)."""
    p, m = _effective_defaults()
    return {"provider": p, "model": m}


@dataclass(frozen=True)
class _LiveModelSource:
    provider_id: str
    configured: Callable[[], bool]
    fetch: Callable[[], Awaitable[list[str]]]


async def _fetch_ollama_live_models() -> list[str]:
    base = ollama_api_base().rstrip("/")
    tag_resp = await get_with_retry(
        f"{base}/api/tags",
        timeout=8.0,
        context="v1_models_tags",
    )
    if tag_resp.status_code != 200:
        return []
    payload = tag_resp.json()
    out: list[str] = []
    for item in payload.get("models") or []:
        if not isinstance(item, dict):
            continue
        name = item.get("name")
        if isinstance(name, str) and name.strip():
            out.append(name.strip())
    return out


async def _fetch_lm_studio_live_models() -> list[str]:
    base = lm_studio_api_base().rstrip("/")
    ls_headers: dict[str, str] = {}
    ls_key = lm_studio_api_key()
    if ls_key:
        ls_headers["Authorization"] = f"Bearer {ls_key}"
    ms_resp = await get_with_retry(
        f"{base}/v1/models",
        headers=ls_headers,
        timeout=8.0,
        context="v1_models_lm_studio",
    )
    if ms_resp.status_code != 200:
        return []
    try:
        payload = ms_resp.json()
    except ValueError:
        return []
    out: list[str] = []
    for item in payload.get("data") or []:
        if not isinstance(item, dict):
            continue
        mid = item.get("id")
        if isinstance(mid, str) and mid.strip():
            out.append(mid.strip())
    return out


async def _fetch_aimlapi_live_models() -> list[str]:
    a_headers = {"Authorization": f"Bearer {aimlapi_api_key()}"}
    ms_resp = await get_with_retry(
        f"{aimlapi_openai_base()}/models",
        headers=a_headers,
        timeout=8.0,
        context="v1_models_aimlapi",
    )
    if ms_resp.status_code != 200:
        return []
    try:
        payload = ms_resp.json()
    except ValueError:
        return []
    out: list[str] = []
    for item in payload.get("data") or []:
        if not isinstance(item, dict):
            continue
        mid = item.get("id")
        if isinstance(mid, str) and mid.strip():
            out.append(mid.strip())
    return out


_LIVE_MODEL_SOURCES: tuple[_LiveModelSource, ...] = (
    _LiveModelSource("ollama", ollama_configured, _fetch_ollama_live_models),
    _LiveModelSource("lm_studio", lm_studio_configured, _fetch_lm_studio_live_models),
    _LiveModelSource("aimlapi", aimlapi_configured, _fetch_aimlapi_live_models),
)


async def collect_full_catalog_model_rows() -> list[dict[str, str]]:
    """
    Same rows as GET /v1/models?providers=all: YAML aliases for configured providers
    plus live Ollama tags and LM Studio model ids.
    """
    data = _load_yaml()
    aliases = _alias_map(data)
    rows: list[dict[str, str]] = []
    for alias, (prov, _mid) in sorted(aliases.items(), key=lambda x: x[0].lower()):
        rows.append({"id": alias, "object": "model", "owned_by": prov})
    seen_ids = {row["id"] for row in rows}

    for source in _LIVE_MODEL_SOURCES:
        if not source.configured():
            continue
        try:
            for mid in await source.fetch():
                if mid in seen_ids:
                    continue
                rows.append({"id": mid, "object": "model", "owned_by": source.provider_id})
                seen_ids.add(mid)
        except LlmProxyError:
            continue

    return rows


def _resolve_model(requested: str | None) -> tuple[str, str]:
    data = _load_yaml()
    raw = _alias_map_raw(data)
    aliases = _alias_map(data)
    dp, dm = _effective_defaults()

    if requested is None or not str(requested).strip():
        return dp, dm

    r = str(requested).strip()
    if r in raw and not provider_configured(raw[r][0]):
        prov = raw[r][0]
        raise HTTPException(
            status_code=503,
            detail=f"Provider {prov} is not configured (check environment for this provider).",
        )
    if r in aliases:
        return aliases[r]
    return dp, r


def _require_auth(request: Request) -> None:
    key = _master_key()
    if not key:
        return
    auth = request.headers.get("authorization") or ""
    if not auth.startswith("Bearer "):
        raise HTTPException(status_code=401, detail="Missing or invalid Authorization header")
    token = auth.removeprefix("Bearer ").strip()
    if token != key:
        raise HTTPException(status_code=401, detail="Invalid API key")


@router.get("/v1/health")
async def health(provider: str | None = None, model: str | None = None) -> JSONResponse:
    """
    LLM liveness: quick check that provider is reachable.
    Uses defaults from config/env when provider/model omitted.
    Optional query: ?provider=ollama|lm_studio|gemini|aimlapi&model=<id> to check a specific route.
    Does NOT block on model catalog checks; uses cached model list.
    """
    dp, dm = _effective_defaults()

    p = (provider or "").strip().lower() or dp
    m = (model or "").strip() or dm

    if not is_supported_provider(p):
        return JSONResponse(
            status_code=400,
            content={
                "status": "error",
                "detail": "provider must be ollama, lm_studio, gemini, or aimlapi",
            },
        )

    started = time.perf_counter()
    detail: dict[str, Any] = {"provider": p, "model": m}
    cache = get_catalog_cache()

    if p == "ollama":
        if not ollama_configured():
            return JSONResponse(
                status_code=503,
                content={
                    "status": "unhealthy",
                    "detail": "OLLAMA_API_BASE is not set",
                    **detail,
                    "elapsed_ms": int((time.perf_counter() - started) * 1000),
                },
            )
        base = ollama_api_base().rstrip("/")
        url = f"{base}/api/version"
        try:
            r = await get_with_retry(url, timeout=4.0, context="health_ollama_version")
        except LlmProxyError as e:
            return JSONResponse(
                status_code=503,
                content={
                    "status": "unhealthy",
                    "detail": e.message,
                    **detail,
                    "elapsed_ms": int((time.perf_counter() - started) * 1000),
                },
            )
        detail["upstream_status"] = r.status_code
        if r.status_code != 200:
            return JSONResponse(
                status_code=503,
                content={
                    "status": "unhealthy",
                    "detail": r.text[:500],
                    **detail,
                    "elapsed_ms": int((time.perf_counter() - started) * 1000),
                },
            )
        names = cache.get_ollama_tags()
        if names:
            detail["model_present"] = any(
                n == m or n.split(":")[0] == m.split(":")[0] for n in names
            )
            detail["model_list_age_sec"] = int(cache.ollama_tag_age_sec())
        else:
            detail["model_present"] = None
            detail["model_list_age_sec"] = None

        return JSONResponse(
            content={
                "status": "ok",
                **detail,
                "elapsed_ms": int((time.perf_counter() - started) * 1000),
            }
        )

    if p == "lm_studio":
        if not lm_studio_configured():
            return JSONResponse(
                status_code=503,
                content={
                    "status": "unhealthy",
                    "detail": "LM_STUDIO_API_BASE is not set",
                    **detail,
                    "elapsed_ms": int((time.perf_counter() - started) * 1000),
                },
            )
        base = lm_studio_api_base().rstrip("/")
        ls_headers: dict[str, str] = {}
        ls_key = lm_studio_api_key()
        if ls_key:
            ls_headers["Authorization"] = f"Bearer {ls_key}"
        try:
            r = await get_with_retry(
                f"{base}/v1/models",
                headers=ls_headers,
                timeout=4.0,
                context="health_lm_studio_version",
            )
        except LlmProxyError as e:
            return JSONResponse(
                status_code=503,
                content={
                    "status": "unhealthy",
                    "detail": e.message,
                    **detail,
                    "elapsed_ms": int((time.perf_counter() - started) * 1000),
                },
            )
        detail["upstream_status"] = r.status_code
        if r.status_code != 200:
            return JSONResponse(
                status_code=503,
                content={
                    "status": "unhealthy",
                    "detail": r.text[:500],
                    **detail,
                    "elapsed_ms": int((time.perf_counter() - started) * 1000),
                },
            )
        ids = cache.get_lm_studio_models()
        if ids:
            detail["model_present"] = m in ids
            detail["model_list_age_sec"] = int(cache.lm_studio_models_age_sec())
        else:
            detail["model_present"] = None
            detail["model_list_age_sec"] = None

        return JSONResponse(
            content={
                "status": "ok",
                **detail,
                "elapsed_ms": int((time.perf_counter() - started) * 1000),
            }
        )

    if p == "aimlapi":
        if not provider_configured("aimlapi"):
            return JSONResponse(
                status_code=503,
                content={
                    "status": "unhealthy",
                    "detail": "AIMLAPI_API_KEY is not set",
                    **detail,
                    "elapsed_ms": int((time.perf_counter() - started) * 1000),
                },
            )
        a_headers = {"Authorization": f"Bearer {aimlapi_api_key()}"}
        try:
            r = await get_with_retry(
                f"{aimlapi_openai_base()}/models",
                headers=a_headers,
                timeout=4.0,
                context="health_aimlapi_models",
            )
        except LlmProxyError as e:
            return JSONResponse(
                status_code=503,
                content={
                    "status": "unhealthy",
                    "detail": e.message,
                    **detail,
                    "elapsed_ms": int((time.perf_counter() - started) * 1000),
                },
            )
        detail["upstream_status"] = r.status_code
        ok = r.status_code == 200
        return JSONResponse(
            status_code=200 if ok else 503,
            content={
                "status": "ok" if ok else "unhealthy",
                **detail,
                "elapsed_ms": int((time.perf_counter() - started) * 1000),
            },
        )

    if not provider_configured("gemini"):
        return JSONResponse(
            status_code=503,
            content={
                "status": "unhealthy",
                "detail": "GEMINI_API_KEY is not set",
                **detail,
                "elapsed_ms": int((time.perf_counter() - started) * 1000),
            },
        )
    gkey = gemini_api_key()
    meta_url = f"https://generativelanguage.googleapis.com/v1beta/models/{m}"
    try:
        r = await get_with_retry(
            meta_url,
            params={"key": gkey},
            timeout=4.0,
            context="health_gemini_meta",
        )
    except LlmProxyError as e:
        return JSONResponse(
            status_code=503,
            content={
                "status": "unhealthy",
                "detail": e.message,
                **detail,
                "elapsed_ms": int((time.perf_counter() - started) * 1000),
            },
        )
    detail["upstream_status"] = r.status_code
    ok = r.status_code == 200
    return JSONResponse(
        status_code=200 if ok else 503,
        content={
            "status": "ok" if ok else "unhealthy",
            **detail,
            "elapsed_ms": int((time.perf_counter() - started) * 1000),
        },
    )


@router.get("/v1/health/readiness")
async def health_readiness() -> dict[str, str]:
    """Always returns ok. Use this for Kubernetes readiness probes to avoid timeout."""
    return {"status": "ok"}


@router.get("/v1/models")
async def list_models(
    request: Request,
    providers: Annotated[
        list[str],
        Query(
            description=(
                "Repeat for multiple backends, e.g. `providers=ollama&providers=lm_studio`. "
                "Use `providers=all` for every configured alias plus live Ollama tags and LM Studio models. "
                "Omit to list only models for the effective default provider."
            ),
        ),
    ] = [],
    provider: str | None = Query(
        None,
        description="Single-provider filter (ollama|lm_studio|gemini|aimlapi). Ignored when `providers` is non-empty.",
    ),
) -> JSONResponse:
    _require_auth(request)
    data = _load_yaml()
    aliases = _alias_map(data)
    eff_p, _ = _effective_defaults()

    raw_p = (provider or "").strip().lower()
    if raw_p and not is_supported_provider(raw_p):
        raise HTTPException(
            status_code=400,
            detail="query provider must be ollama, lm_studio, gemini, aimlapi, or omitted",
        )
    if raw_p and is_supported_provider(raw_p) and not provider_configured(raw_p):
        raise HTTPException(
            status_code=503,
            detail=f"Provider {raw_p} is not configured (check environment for this provider).",
        )

    prov_tokens = [p.strip().lower() for p in providers if isinstance(p, str) and p.strip()]
    full_catalog = False
    allowed: set[str] | None = None

    if prov_tokens:
        if "all" in prov_tokens:
            if len(prov_tokens) != 1:
                raise HTTPException(
                    status_code=400,
                    detail="providers=all must not be combined with other provider values",
                )
            full_catalog = True
        else:
            allowed_set: set[str] = set()
            for p in prov_tokens:
                if not is_supported_provider(p):
                    raise HTTPException(
                        status_code=400,
                        detail=f"unknown provider in providers: {p}",
                    )
                if not provider_configured(p):
                    raise HTTPException(
                        status_code=503,
                        detail=f"Provider {p} is not configured (check environment for this provider).",
                    )
                allowed_set.add(p)
            allowed = allowed_set
    elif raw_p and is_supported_provider(raw_p):
        allowed = {raw_p}
    else:
        ep = eff_p if is_supported_provider(eff_p) else "lm_studio"
        allowed = {ep}

    if full_catalog:
        rows = await collect_full_catalog_model_rows()
        return JSONResponse(content={"object": "list", "data": rows})

    rows: list[dict[str, str]] = []
    for alias, (prov, _mid) in sorted(aliases.items(), key=lambda x: x[0].lower()):
        if allowed is None or prov not in allowed:
            continue
        rows.append({"id": alias, "object": "model", "owned_by": prov})

    seen_ids = {row["id"] for row in rows}

    for source in _LIVE_MODEL_SOURCES:
        if allowed is not None and source.provider_id not in allowed:
            continue
        if not source.configured():
            continue
        try:
            for mid in await source.fetch():
                if mid in seen_ids:
                    continue
                rows.append({"id": mid, "object": "model", "owned_by": source.provider_id})
                seen_ids.add(mid)
        except LlmProxyError:
            continue

    return JSONResponse(content={"object": "list", "data": rows})


def _upstream_urls(provider: str) -> tuple[str, str]:
    if provider == "ollama":
        base = ollama_api_base().rstrip("/")
        if not base:
            raise LlmProxyError(
                503,
                "ollama_base_missing",
                "OLLAMA_API_BASE is not set.",
            )
        return (
            f"{base}/v1/chat/completions",
            f"{base}/v1/embeddings",
        )
    if provider == "lm_studio":
        base = lm_studio_api_base().rstrip("/")
        if not base:
            raise LlmProxyError(
                503,
                "lm_studio_base_missing",
                "LM_STUDIO_API_BASE is not set.",
            )
        return (
            f"{base}/v1/chat/completions",
            f"{base}/v1/embeddings",
        )
    if provider == "gemini":
        return (
            f"{GEMINI_OPENAI_BASE}/chat/completions",
            f"{GEMINI_OPENAI_BASE}/embeddings",
        )
    if provider == "aimlapi":
        base = aimlapi_openai_base().rstrip("/")
        return (
            f"{base}/chat/completions",
            f"{base}/embeddings",
        )
    raise LlmProxyError(500, "invalid_provider", "Invalid provider routing (internal).")


def _outbound_headers(provider: str) -> dict[str, str]:
    h: dict[str, str] = {"Content-Type": "application/json"}
    if provider == "gemini":
        key = gemini_api_key()
        if not key:
            raise LlmProxyError(
                503,
                "gemini_key_missing",
                "GEMINI_API_KEY is not configured for Gemini routes.",
            )
        h["Authorization"] = f"Bearer {key}"
    elif provider == "aimlapi":
        key = aimlapi_api_key()
        if not key:
            raise LlmProxyError(
                503,
                "aimlapi_key_missing",
                "AIMLAPI_API_KEY is not configured for AIMLAPI routes.",
            )
        h["Authorization"] = f"Bearer {key}"
    elif provider == "lm_studio":
        key = lm_studio_api_key()
        if key:
            h["Authorization"] = f"Bearer {key}"
    return h


@router.post("/v1/chat/completions")
async def chat_completions(request: Request) -> Response:
    _require_auth(request)
    try:
        body: dict[str, Any] = await request.json()
    except json.JSONDecodeError as e:
        raise HTTPException(status_code=400, detail=f"Invalid JSON: {e}") from e

    raw_model = body.get("model")
    if raw_model is not None and not isinstance(raw_model, str):
        raise HTTPException(status_code=400, detail="model must be a string")
    prov, resolved = _resolve_model(raw_model if isinstance(raw_model, str) else None)
    resolved = await coerce_local_model_if_needed(prov, resolved)
    body = dict(body)
    body["model"] = resolved

    chat_url, _ = _upstream_urls(prov)
    headers = _outbound_headers(prov)
    stream = bool(body.get("stream"))

    if not stream:
        r = await post_with_retry(
            chat_url,
            headers=headers,
            json_body=body,
            timeout=300.0,
            context="chat_completions",
        )
        return Response(
            content=r.content,
            status_code=r.status_code,
            media_type=r.headers.get("content-type", "application/json"),
        )

    response, client = await stream_chat_completion(
        chat_url,
        headers=headers,
        json_body=body,
        timeout=300.0,
        context="chat_completions_stream",
    )
    if response.status_code >= 400:
        err_body = await response.aread()
        await aclose_stream(response, client)
        return Response(
            content=err_body,
            status_code=response.status_code,
            media_type=response.headers.get("content-type", "application/json"),
        )

    async def gen():
        try:
            async for chunk in response.aiter_bytes():
                yield chunk
        except httpx.RequestError as e:
            code, msg = classify_httpx_error(e, "chat_completions_stream")
            yield sse_error_chunk(code, msg)
        finally:
            await aclose_stream(response, client)

    return StreamingResponse(gen(), media_type="text/event-stream")


@router.post("/v1/embeddings")
async def embeddings(request: Request) -> Response:
    _require_auth(request)
    try:
        body: dict[str, Any] = await request.json()
    except json.JSONDecodeError as e:
        raise HTTPException(status_code=400, detail=f"Invalid JSON: {e}") from e

    raw_model = body.get("model")
    if not isinstance(raw_model, str) or not raw_model.strip():
        raise HTTPException(status_code=400, detail="model is required")
    prov, resolved = _resolve_model(raw_model)
    resolved = await coerce_local_model_if_needed(prov, resolved)
    body = dict(body)
    body["model"] = resolved

    _, emb_url = _upstream_urls(prov)
    headers = _outbound_headers(prov)
    r = await post_with_retry(
        emb_url,
        headers=headers,
        json_body=body,
        timeout=120.0,
        context="embeddings",
    )
    return Response(
        content=r.content,
        status_code=r.status_code,
        media_type=r.headers.get("content-type", "application/json"),
    )
