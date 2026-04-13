"""Stateless single-turn chat proxy to llm-orchestrator (OpenAI-compatible)."""

from __future__ import annotations

from typing import Any

import httpx
from fastapi import APIRouter, HTTPException
from fastapi.responses import JSONResponse
from pydantic import BaseModel, ConfigDict, field_validator

from context_budget import fit_chat_messages_for_request, max_output_tokens_default
from dag_schema import sanitize_llm_model_alias
from orchestrator_env import (
    orchestrator_base_url_v1,
    orchestrator_http_timeout_seconds,
    orchestrator_master_key,
)

router = APIRouter(tags=["chat"])


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


def _orchestrator_origin() -> str:
    base = orchestrator_base_url_v1().rstrip("/")
    return base.rsplit("/v1", 1)[0]


@router.get("/orchestrator/ready")
async def orchestrator_ready():
    """
    Lightweight probe: server calls llm-orchestrator with the configured bearer key.
    Used by the browser UI to show local stack health without exposing ORCHESTRATOR_MASTER_KEY.
    """
    key = orchestrator_master_key()
    if not key:
        raise HTTPException(
            status_code=503,
            detail="ORCHESTRATOR_MASTER_KEY is not set.",
        )
    origin = _orchestrator_origin()
    headers = {"Authorization": f"Bearer {key}"}
    try:
        async with httpx.AsyncClient(timeout=8.0) as client:
            r = await client.get(f"{origin}/v1/models", headers=headers)
    except httpx.RequestError as e:
        raise HTTPException(status_code=502, detail=f"Upstream request failed: {e}") from e
    if r.status_code != 200:
        raise HTTPException(
            status_code=502,
            detail=f"Orchestrator returned HTTP {r.status_code}",
        )
    return {"ok": True}


@router.post("/chat")
async def chat_completions(req: ChatCompletionRequest):
    """
    One-shot chat completion via the configured orchestrator (POST {base}/chat/completions).
    Does not create a Process; for multi-agent orchestration use POST /api/v1/processes.
    """
    key = orchestrator_master_key()
    if not key:
        raise HTTPException(
            status_code=503,
            detail="ORCHESTRATOR_MASTER_KEY is not set.",
        )

    base = orchestrator_base_url_v1()
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
        async with httpx.AsyncClient(timeout=orchestrator_http_timeout_seconds()) as client:
            r = await client.post(f"{base}/chat/completions", headers=headers, json=payload)
    except httpx.RequestError as e:
        raise HTTPException(status_code=502, detail=f"Upstream request failed: {e}") from e

    try:
        data = r.json()
    except Exception:
        return JSONResponse(content={"raw": r.text}, status_code=r.status_code)
    return JSONResponse(content=data, status_code=r.status_code)
