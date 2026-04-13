"""Tests for server-side context budgeting."""

from context_budget import (
    estimate_messages_tokens,
    estimate_tokens,
    fit_dependency_outputs_to_budget,
    prompt_token_budget,
    shrink_messages_to_budget,
    truncate_text_to_tokens,
)


def test_truncate_text_to_tokens_short_unchanged():
    s = "hello world"
    assert truncate_text_to_tokens(s, estimate_tokens(s) + 10) == s


def test_truncate_text_to_tokens_adds_suffix_when_long():
    long = "x" * 10_000
    out = truncate_text_to_tokens(long, 50)
    assert len(out) < len(long)
    assert "truncated" in out


def test_fit_dependency_outputs_scales_down():
    chunks = ["a" * 2000, "b" * 2000, "c" * 2000]
    out = fit_dependency_outputs_to_budget(chunks, max_tokens=120)
    assert all(isinstance(x, str) for x in out)
    assert sum(estimate_tokens(x) for x in out) <= 800  # proportional shrink + separators


def test_shrink_messages_prefers_tool_role():
    messages = [
        {"role": "system", "content": "sys"},
        {"role": "user", "content": "question"},
        {"role": "assistant", "content": "ok"},
        {"role": "tool", "tool_call_id": "1", "content": "t" * 50_000},
    ]
    budget = prompt_token_budget()
    shrunk = shrink_messages_to_budget(messages, min(budget, 800))
    assert estimate_messages_tokens(shrunk) <= 800 + 50


def test_prompt_token_budget_positive():
    assert prompt_token_budget() >= 512
