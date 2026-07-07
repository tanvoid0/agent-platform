"""Normalize and synthesize token usage on LLM proxy responses."""

from __future__ import annotations

import json
from typing import Any

from context_budget import estimate_messages_tokens, estimate_tokens


def _coerce_int(v: Any) -> int:
    if v is None or isinstance(v, bool):
        return 0
    try:
        return max(0, int(v))
    except (TypeError, ValueError):
        return 0


def normalize_usage_dict(raw: dict[str, Any] | None) -> dict[str, Any] | None:
    """Map provider-specific usage shapes to OpenAI-compatible fields."""
    if not isinstance(raw, dict):
        return None

    prompt = _coerce_int(raw.get("prompt_tokens"))
    completion = _coerce_int(raw.get("completion_tokens"))
    total = _coerce_int(raw.get("total_tokens"))

    if not prompt and not completion and not total:
        prompt = _coerce_int(raw.get("prompt_eval_count"))
        completion = _coerce_int(raw.get("eval_count"))
        if not total and (prompt or completion):
            total = prompt + completion

    if not prompt and not completion and not total:
        return None

    if not total:
        total = prompt + completion

    out: dict[str, Any] = {
        "prompt_tokens": prompt,
        "completion_tokens": completion,
        "total_tokens": total,
    }
    for key in ("cost", "total_cost", "response_cost"):
        if key in raw:
            out[key] = raw[key]
    return out


def synthesize_usage(
    request_messages: list[dict[str, Any]] | None,
    response_content: str,
) -> dict[str, Any]:
    """Estimate usage when upstream omits it (tiktoken-based)."""
    prompt_tokens = estimate_messages_tokens(request_messages or [])
    completion_tokens = estimate_tokens(response_content or "")
    return {
        "prompt_tokens": prompt_tokens,
        "completion_tokens": completion_tokens,
        "total_tokens": prompt_tokens + completion_tokens,
        "estimated": True,
    }


def normalize_completion_body(
    body: bytes,
    *,
    request_messages: list[dict[str, Any]] | None = None,
) -> bytes:
    """Ensure chat completion JSON includes a normalized usage block when possible."""
    if not body:
        return body
    try:
        data = json.loads(body)
    except json.JSONDecodeError:
        return body
    if not isinstance(data, dict):
        return body

    usage = normalize_usage_dict(
        data.get("usage") if isinstance(data.get("usage"), dict) else None
    )
    if usage is None:
        content = ""
        choices = data.get("choices") or []
        if choices:
            msg = choices[0].get("message") or {}
            content = str(msg.get("content") or "")
        usage = synthesize_usage(request_messages, content)

    data["usage"] = usage
    return json.dumps(data, ensure_ascii=False).encode("utf-8")
