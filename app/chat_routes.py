"""Stateless single-turn chat via the embedded OpenAI-compatible LLM proxy."""

from __future__ import annotations

import asyncio
import os
from typing import Any

import httpx
from fastapi import APIRouter, Depends, HTTPException
from fastapi.responses import JSONResponse
from pydantic import BaseModel, ConfigDict, field_validator

from api_tokens.auth import TokenPrincipal, require_scope, require_valid_token
from context_budget import fit_chat_messages_for_request, max_output_tokens_default
from dag_schema import sanitize_llm_model_alias
from llm_proxy_env import (
    llm_proxy_base_url_v1,
    llm_proxy_http_timeout_seconds,
    llm_proxy_master_key,
)

router = APIRouter(tags=["chat"])


def _chat_max_concurrent_requests() -> int:
    """
    Caps requests in flight to the upstream LLM proxy at once. Many simulated agents
    can fire chat calls in the same tick; this throttles them to what the configured
    upstream (Ollama, AIML API, etc.) can actually sustain, queueing the rest instead
    of letting them all hit the upstream and bounce off its own rate limiting.
    """
    raw = (os.getenv("AGENT_PLATFORM_CHAT_MAX_CONCURRENT") or "8").strip()
    try:
        return max(1, int(raw))
    except ValueError:
        return 8


_llm_semaphore = asyncio.Semaphore(_chat_max_concurrent_requests())


def _chat_resolved_defaults() -> dict[str, str]:
    """Same provider/model as the embedded OpenAI proxy for unqualified requests (config + env)."""
    from llm_proxy.routes.llm import get_resolved_proxy_defaults

    return get_resolved_proxy_defaults()


class ChatCompletionRequest(BaseModel):
    """OpenAI-compatible chat completions body for the Flow UI and other clients."""

    model_config = ConfigDict(extra="ignore")

    messages: list[Any]
    model: str | None = None
    tools: list[dict[str, Any]] | None = None
    tool_choice: Any | None = None
    temperature: float | None = None
    max_tokens: int | None = None
    top_p: float | None = None
    response_format: dict[str, Any] | None = None
    stream: bool | None = None

    @field_validator("messages")
    @classmethod
    def _messages_non_empty(cls, v: list[Any]) -> list[Any]:
        if not isinstance(v, list) or len(v) == 0:
            raise ValueError("messages must be a non-empty list")
        return v


def _llm_proxy_origin() -> str:
    base = llm_proxy_base_url_v1().rstrip("/")
    return base.rsplit("/v1", 1)[0]


@router.get("/llm/ready")
async def llm_ready():
    """
    Lightweight probe: server GETs /v1/health/readiness on the embedded LLM proxy.

    Do not use GET /v1/models here: that handler may call Ollama and LM Studio (multi-second),
    while the Flow UI times out browser fetches to this route (~4.5s) and showed LLM offline
    even when chat and agents worked.
    """
    key = llm_proxy_master_key()
    if not key:
        raise HTTPException(
            status_code=503,
            detail="AGENT_PLATFORM_MASTER_KEY is not set.",
        )
    origin = _llm_proxy_origin()
    headers = {"Authorization": f"Bearer {key}"}
    try:
        async with httpx.AsyncClient(timeout=8.0) as client:
            r = await client.get(f"{origin}/v1/health/readiness", headers=headers)
    except httpx.RequestError as e:
        raise HTTPException(status_code=502, detail=f"Upstream request failed: {e}") from e
    if r.status_code != 200:
        raise HTTPException(
            status_code=502,
            detail=f"LLM proxy returned HTTP {r.status_code}",
        )
    return {"ok": True}


@router.get("/chat/resolved-defaults")
async def chat_resolved_defaults():
    """
    Effective LLM provider for this Agent Platform process (matches embedded proxy defaults).

    Used by the Flow UI so the client does not need Vite flags to mirror server routing.
    """
    d = _chat_resolved_defaults()
    return {"provider": d["provider"], "model": d["model"]}


@router.get("/llm/ui-catalog")
async def llm_ui_catalog():
    """
    Provider activation, reachability probes, chat model lists (from embedded proxy),
    and Gemini media defaults for Flow settings and pickers.
    """
    from llm_ui_catalog import build_llm_ui_catalog_response

    return await build_llm_ui_catalog_response()


@router.post("/chat", summary="One-shot OpenAI-compatible chat completion")
async def chat_completions(
    req: ChatCompletionRequest,
    principal: TokenPrincipal = Depends(require_valid_token),
):
    """
    One-shot chat completion via the embedded LLM proxy (POST {base}/chat/completions).
    Does not create a Process; for multi-agent runs use POST /api/v1/processes.
    Concurrency capped by AGENT_PLATFORM_CHAT_MAX_CONCURRENT (default 8); excess
    requests from simulated agents queue here instead of hitting the upstream at once.
    The upstream call itself also retries on rate-limit responses (see upstream_http.py).
    """
    require_scope(principal, "chat:write")
    key = llm_proxy_master_key()
    if not key:
        raise HTTPException(
            status_code=503,
            detail="AGENT_PLATFORM_MASTER_KEY is not set.",
        )

    async with _llm_semaphore:
        base = llm_proxy_base_url_v1()
        fitted_messages, _ = fit_chat_messages_for_request([dict(m) for m in req.messages])
        payload: dict[str, Any] = {"messages": fitted_messages}
        if req.model is not None and req.model.strip():
            sm = sanitize_llm_model_alias(req.model.strip())
            if sm:
                payload["model"] = sm
        if req.tools is not None:
            payload["tools"] = req.tools
        if req.tool_choice is not None:
            payload["tool_choice"] = req.tool_choice
        if req.temperature is not None:
            payload["temperature"] = req.temperature
        if req.max_tokens is not None:
            payload["max_tokens"] = req.max_tokens
        else:
            payload["max_tokens"] = max_output_tokens_default()
        if req.top_p is not None:
            payload["top_p"] = req.top_p
        if req.response_format is not None:
            payload["response_format"] = req.response_format
        if req.stream is not None:
            payload["stream"] = req.stream

        headers = {"Content-Type": "application/json", "Authorization": f"Bearer {key}"}
        try:
            async with httpx.AsyncClient(timeout=llm_proxy_http_timeout_seconds()) as client:
                r = await client.post(f"{base}/chat/completions", headers=headers, json=payload)
        except httpx.RequestError as e:
            raise HTTPException(status_code=502, detail=f"Upstream request failed: {e}") from e

        try:
            data = r.json()
        except Exception:
            return JSONResponse(content={"raw": r.text}, status_code=r.status_code)
        return JSONResponse(content=data, status_code=r.status_code)
