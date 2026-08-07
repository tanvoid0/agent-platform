"""Stateless single-turn chat via the embedded OpenAI-compatible LLM proxy."""

from __future__ import annotations

import asyncio
import os
from typing import Any

import httpx
from fastapi import APIRouter, Depends, HTTPException, Response
from fastapi.responses import JSONResponse, StreamingResponse
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


class ChatCompletionRequest(BaseModel):
    """OpenAI-compatible chat completions body for the Flow UI and other clients."""

    model_config = ConfigDict(extra="ignore")

    messages: list[Any]
    model: str | None = None
    provider: str | None = None
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


async def _stream_completion(url: str, headers: dict[str, str], payload: dict[str, Any]):
    """
    Pass the proxy's SSE body through byte-for-byte (frames are already OpenAI
    `chat.completion.chunk` deltas — see llm_proxy/routes/llm.py).

    The concurrency semaphore is held for the life of the stream, not just the
    request, so a slow reader still counts against AGENT_PLATFORM_CHAT_MAX_CONCURRENT.
    Acquired before opening upstream and released in the generator's `finally`.
    """
    from llm_proxy.services.upstream_http import (
        aclose_stream,
        sse_error_chunk,
        stream_chat_completion,
    )

    await _llm_semaphore.acquire()
    response = client = None
    try:
        response, client = await stream_chat_completion(
            url,
            headers=headers,
            json_body=payload,
            timeout=llm_proxy_http_timeout_seconds(),
            context="chat_stream",
        )
    except Exception:
        _llm_semaphore.release()
        raise

    if response.status_code >= 400:
        err_body = await response.aread()
        await aclose_stream(response, client)
        _llm_semaphore.release()
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
            yield sse_error_chunk("upstream_error", f"Upstream request failed: {e}")
        finally:
            await aclose_stream(response, client)
            _llm_semaphore.release()

    return StreamingResponse(gen(), media_type="text/event-stream")


@router.post("/chat", summary="Single-turn OpenAI-compatible chat completion")
async def chat_completions(
    req: ChatCompletionRequest,
    principal: TokenPrincipal = Depends(require_valid_token),
):
    """
    Single-turn chat completion via the embedded LLM proxy (POST {base}/chat/completions).
    With ``stream: true`` the proxy's ``text/event-stream`` body is passed through
    unchanged; otherwise the full JSON completion is returned.
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

    base = llm_proxy_base_url_v1()
    fitted_messages, _ = fit_chat_messages_for_request([dict(m) for m in req.messages])
    payload: dict[str, Any] = {"messages": fitted_messages}
    if req.model is not None and req.model.strip():
        sm = sanitize_llm_model_alias(req.model.strip())
        if sm:
            payload["model"] = sm
    if req.provider is not None and req.provider.strip():
        # The proxy validates the hint and routes to that provider
        # (llm_proxy/routes/llm.py); unknown providers come back as 400.
        payload["provider"] = req.provider.strip().lower()
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

    if req.stream:
        return await _stream_completion(f"{base}/chat/completions", headers, payload)

    async with _llm_semaphore:
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
