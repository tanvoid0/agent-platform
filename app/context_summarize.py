"""
Optional LLM-based condensation of oversized context chunks (server-side only).
"""

from __future__ import annotations

import os

from context_budget import estimate_tokens, truncate_text_to_tokens

_SUMMARY_PROMPT = """You compress prior-step outputs for a downstream agent. Preserve:
- concrete facts, numbers, names, paths, URLs, errors
- decisions and conclusions
- anything needed to continue the task

Be concise. Use bullet lists when helpful. Omit filler and repetition."""


def summarize_enabled() -> bool:
    raw = (os.getenv("AGENT_PLATFORM_CONTEXT_SUMMARIZE") or "").strip().lower()
    return raw in ("1", "true", "yes", "on")


def summarize_min_input_tokens() -> int:
    raw = (os.getenv("AGENT_PLATFORM_CONTEXT_SUMMARIZE_MIN_TOKENS") or "6000").strip()
    try:
        return max(512, int(raw))
    except ValueError:
        return 6000


def summarize_max_output_tokens() -> int:
    raw = (os.getenv("AGENT_PLATFORM_CONTEXT_SUMMARIZE_MAX_OUTPUT_TOKENS") or "900").strip()
    try:
        return max(128, int(raw))
    except ValueError:
        return 900


def summarize_max_input_tokens() -> int:
    raw = (os.getenv("AGENT_PLATFORM_CONTEXT_SUMMARIZE_MAX_INPUT_TOKENS") or "16000").strip()
    try:
        return max(1024, int(raw))
    except ValueError:
        return 16000


async def maybe_condense_text_for_context(text: str, *, model: str | None) -> str:
    """
    If summarization is enabled and the chunk is large, replace it with an LLM summary.
    Otherwise return text unchanged (caller still applies token budgets).
    """
    if not summarize_enabled():
        return text
    if estimate_tokens(text) < summarize_min_input_tokens():
        return text

    body = truncate_text_to_tokens(text, summarize_max_input_tokens())

    from llm_client import call_llm

    messages = [
        {"role": "system", "content": _SUMMARY_PROMPT},
        {"role": "user", "content": body},
    ]
    out, _tokens, _cost = await call_llm(
        messages,
        model=model,
        temperature=0.2,
        max_output_tokens=summarize_max_output_tokens(),
    )
    return (out or "").strip() or truncate_text_to_tokens(text, summarize_max_output_tokens())
