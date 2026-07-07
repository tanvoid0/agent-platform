"""Unit tests for chat_usage helpers."""

import pytest

from chat_usage import (
    estimate_context_usage,
    merge_llm_usages,
    parse_llm_usage,
    parse_llm_usage_dict,
)
from llm_proxy.usage_normalize import normalize_completion_body, normalize_usage_dict, synthesize_usage


def test_parse_llm_usage_openai_shape():
    data = {
        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15, "cost": 0.01}
    }
    step = parse_llm_usage(data, label="test")
    assert step.prompt_tokens == 10
    assert step.completion_tokens == 5
    assert step.total_tokens == 15
    assert step.cost_usd == 0.01
    assert step.label == "test"


def test_merge_llm_usages_sums_steps():
    from chat_usage import LlmStepUsageOut

    merged = merge_llm_usages(
        [
            LlmStepUsageOut(prompt_tokens=3, completion_tokens=2, total_tokens=5, cost_usd=0.1),
            LlmStepUsageOut(prompt_tokens=7, completion_tokens=1, total_tokens=8, cost_usd=0.2),
        ]
    )
    assert merged.prompt_tokens == 10
    assert merged.completion_tokens == 3
    assert merged.total_tokens == 13
    assert merged.cost_usd == pytest.approx(0.3)
    assert len(merged.steps) == 2


def test_estimate_context_usage_categories():
    ctx = estimate_context_usage(
        system_prompt="You are helpful.",
        tools=[{"type": "function", "function": {"name": "read_file"}}],
        conversation_messages=[{"role": "user", "content": "hi"}],
    )
    assert ctx.context_window > 0
    assert ctx.categories["system_prompt"] > 0
    assert ctx.categories["tools"] > 0
    assert ctx.categories["conversation"] > 0
    assert ctx.total_estimated == sum(ctx.categories.values())
    assert ctx.percent_used >= 0


def test_normalize_usage_ollama_shape():
    out = normalize_usage_dict({"prompt_eval_count": 100, "eval_count": 25})
    assert out is not None
    assert out["prompt_tokens"] == 100
    assert out["completion_tokens"] == 25
    assert out["total_tokens"] == 125


def test_synthesize_usage_flagged():
    usage = synthesize_usage(
        [{"role": "user", "content": "hello"}],
        "world",
    )
    assert usage["estimated"] is True
    assert usage["total_tokens"] == usage["prompt_tokens"] + usage["completion_tokens"]


def test_normalize_completion_body_injects_usage():
    body = normalize_completion_body(
        b'{"choices":[{"message":{"content":"hi"}}]}',
        request_messages=[{"role": "user", "content": "hello"}],
    )
    import json

    data = json.loads(body)
    assert "usage" in data
    assert data["usage"]["total_tokens"] > 0
    assert data["usage"].get("estimated") is True
