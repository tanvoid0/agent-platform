"""Shared token usage models and helpers for chat APIs."""

from __future__ import annotations

import json
from typing import Any

from pydantic import BaseModel, Field

from context_budget import (
    context_window_tokens,
    estimate_messages_tokens,
    estimate_tokens,
    max_output_tokens_default,
    prompt_token_budget,
)
from llm_client import usage_cost_from_completion_response

# Fixed category keys (match Cursor-style UI; unused = 0).
CONTEXT_CATEGORY_KEYS = (
    "system_prompt",
    "tools",
    "rules",
    "skills",
    "mcp",
    "subagents",
    "conversation",
    "injected_context",
)


class ContextUsageOut(BaseModel):
    context_window: int
    total_estimated: int
    percent_used: float
    prompt_budget: int
    reserved_output: int
    categories: dict[str, int] = Field(default_factory=dict)


class LlmStepUsageOut(BaseModel):
    prompt_tokens: int = 0
    completion_tokens: int = 0
    total_tokens: int = 0
    cost_usd: float = 0.0
    label: str | None = None


class LlmUsageOut(BaseModel):
    prompt_tokens: int = 0
    completion_tokens: int = 0
    total_tokens: int = 0
    cost_usd: float = 0.0
    steps: list[LlmStepUsageOut] = Field(default_factory=list)


def _empty_categories() -> dict[str, int]:
    return {k: 0 for k in CONTEXT_CATEGORY_KEYS}


def _coerce_int(v: Any) -> int:
    if v is None or isinstance(v, bool):
        return 0
    try:
        return max(0, int(v))
    except (TypeError, ValueError):
        return 0


def parse_llm_usage(
    data: dict[str, Any],
    *,
    label: str | None = None,
) -> LlmStepUsageOut:
    """Parse OpenAI-compatible usage from a chat completion response body."""
    usage = data.get("usage") if isinstance(data.get("usage"), dict) else {}
    prompt = _coerce_int(usage.get("prompt_tokens"))
    completion = _coerce_int(usage.get("completion_tokens"))
    total = _coerce_int(usage.get("total_tokens"))
    if total == 0 and (prompt or completion):
        total = prompt + completion
    cost = usage_cost_from_completion_response(data)
    return LlmStepUsageOut(
        prompt_tokens=prompt,
        completion_tokens=completion,
        total_tokens=total,
        cost_usd=cost,
        label=label,
    )


def parse_llm_usage_dict(
    usage: dict[str, Any] | None,
    *,
    label: str | None = None,
) -> LlmStepUsageOut:
    """Parse a raw usage dict (e.g. from streaming final chunk)."""
    if not isinstance(usage, dict):
        return LlmStepUsageOut(label=label)
    return parse_llm_usage({"usage": usage}, label=label)


def merge_llm_usages(steps: list[LlmStepUsageOut]) -> LlmUsageOut:
    if not steps:
        return LlmUsageOut()
    return LlmUsageOut(
        prompt_tokens=sum(s.prompt_tokens for s in steps),
        completion_tokens=sum(s.completion_tokens for s in steps),
        total_tokens=sum(s.total_tokens for s in steps),
        cost_usd=sum(s.cost_usd for s in steps),
        steps=list(steps),
    )


def estimate_context_usage(
    *,
    system_prompt: str | None = None,
    tools: list[dict[str, Any]] | None = None,
    rules: list[str] | None = None,
    skills: list[str] | None = None,
    mcp_tools: list[dict[str, Any]] | None = None,
    subagent_defs: list[dict[str, Any]] | None = None,
    conversation_messages: list[dict[str, Any]] | None = None,
    injected_context: str | None = None,
) -> ContextUsageOut:
    """Estimate input context breakdown using tiktoken (may diverge from provider)."""
    categories = _empty_categories()

    if system_prompt:
        categories["system_prompt"] = estimate_tokens(system_prompt)
    if tools:
        categories["tools"] = estimate_tokens(json.dumps(tools, ensure_ascii=False))
    if rules:
        categories["rules"] = estimate_tokens("\n".join(rules))
    if skills:
        categories["skills"] = estimate_tokens("\n".join(skills))
    if mcp_tools:
        categories["mcp"] = estimate_tokens(json.dumps(mcp_tools, ensure_ascii=False))
    if subagent_defs:
        categories["subagents"] = estimate_tokens(json.dumps(subagent_defs, ensure_ascii=False))
    if conversation_messages:
        categories["conversation"] = estimate_messages_tokens(conversation_messages)
    if injected_context:
        categories["injected_context"] = estimate_tokens(injected_context)

    total = sum(categories.values())
    window = context_window_tokens()
    percent = round((total / window) * 100, 1) if window > 0 else 0.0

    return ContextUsageOut(
        context_window=window,
        total_estimated=total,
        percent_used=percent,
        prompt_budget=prompt_token_budget(),
        reserved_output=max_output_tokens_default(),
        categories=categories,
    )
