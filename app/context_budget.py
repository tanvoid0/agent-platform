"""
Server-side prompt sizing: token estimates, truncation, and message shrinking.

The UI never decides context limits; it only displays whatever the API returns.
"""

from __future__ import annotations

import copy
import os
from functools import lru_cache
from typing import Any

# Approximate OpenAI-style overhead per message (role + delimiters), in tokens.
_MESSAGE_OVERHEAD_TOKENS = 4


@lru_cache(maxsize=4)
def _tiktoken_encoding(name: str):
    import tiktoken

    return tiktoken.get_encoding(name)


def _encoding_name() -> str:
    return (os.getenv("AGENT_PLATFORM_TOKEN_ENCODING") or "cl100k_base").strip() or "cl100k_base"


def estimate_tokens(text: str) -> int:
    """Best-effort token count; uses tiktoken when available."""
    if not text:
        return 0
    try:
        enc = _tiktoken_encoding(_encoding_name())
        return len(enc.encode(text))
    except Exception:
        return max(1, (len(text) + 3) // 4)


def context_window_tokens() -> int:
    raw = (os.getenv("AGENT_PLATFORM_CONTEXT_WINDOW_TOKENS") or "32768").strip()
    try:
        return max(1024, int(raw))
    except ValueError:
        return 32768


def max_output_tokens_default() -> int:
    raw = (os.getenv("AGENT_PLATFORM_MAX_OUTPUT_TOKENS") or "4096").strip()
    try:
        return max(256, int(raw))
    except ValueError:
        return 4096


def safety_margin_tokens() -> int:
    raw = (os.getenv("AGENT_PLATFORM_CONTEXT_SAFETY_MARGIN") or "512").strip()
    try:
        return max(0, int(raw))
    except ValueError:
        return 512


def tool_result_soft_cap_tokens() -> int:
    """Per tool result: truncate before appending to the conversation (then shrink may trim further)."""
    raw = (os.getenv("AGENT_PLATFORM_TOOL_RESULT_MAX_TOKENS") or "12000").strip()
    try:
        return max(256, int(raw))
    except ValueError:
        return 12000


def prompt_token_budget() -> int:
    """
    Maximum tokens allowed for the prompt side of a chat/completions request
    (messages only, excluding the completion).
    """
    w = context_window_tokens()
    out = max_output_tokens_default()
    margin = safety_margin_tokens()
    return max(512, w - out - margin)


def estimate_messages_tokens(messages: list[dict[str, Any]]) -> int:
    total = 0
    for m in messages:
        c = m.get("content")
        if isinstance(c, str):
            total += estimate_tokens(c)
        elif c is not None:
            total += estimate_tokens(str(c))
        # tool_calls JSON can be large; count roughly
        tc = m.get("tool_calls")
        if isinstance(tc, list):
            total += estimate_tokens(str(tc))
    total += _MESSAGE_OVERHEAD_TOKENS * len(messages)
    return total


def truncate_text_to_tokens(
    text: str,
    max_tokens: int,
    *,
    suffix: str = "...[truncated]",
) -> str:
    if max_tokens <= 0:
        return ""
    if not text:
        return ""
    s_len = estimate_tokens(suffix)
    if max_tokens <= s_len:
        return suffix[:max_tokens] if max_tokens < len(suffix) else suffix
    try:
        enc = _tiktoken_encoding(_encoding_name())
        ids = enc.encode(text)
        if len(ids) <= max_tokens - s_len:
            return text
        keep = max_tokens - s_len
        return enc.decode(ids[:keep]) + suffix
    except Exception:
        # Heuristic fallback: ~4 chars per token
        budget_chars = max(0, (max_tokens - s_len) * 4 - 3)
        if len(text) <= budget_chars:
            return text
        return text[:budget_chars] + suffix


def _role_priority(role: str | None) -> int:
    if role == "tool":
        return 0
    if role == "assistant":
        return 1
    if role == "user":
        return 2
    return 3  # system


def shrink_messages_to_budget(
    messages: list[dict[str, Any]],
    budget_tokens: int,
    *,
    min_message_tokens: int = 48,
) -> list[dict[str, Any]]:
    """
    Return a deep copy of messages with contents truncated until the estimated
    prompt size is <= budget_tokens. Shrinks tool outputs first, then assistant,
    then user, then system (oldest indices first within each role tier).
    """
    if budget_tokens <= 0:
        return []
    msgs: list[dict[str, Any]] = copy.deepcopy(messages)
    if not msgs:
        return msgs

    def total_est() -> int:
        return estimate_messages_tokens(msgs)

    if total_est() <= budget_tokens:
        return msgs

    order = sorted(
        range(len(msgs)),
        key=lambda i: (_role_priority(str(msgs[i].get("role"))), i),
    )

    max_rounds = max(len(msgs) * 32, 64)
    for _ in range(max_rounds):
        if total_est() <= budget_tokens:
            break
        over = total_est() - budget_tokens
        if over <= 0:
            break
        progressed = False
        for i in order:
            m = msgs[i]
            c = m.get("content")
            if not isinstance(c, str) or not c:
                continue
            te = estimate_tokens(c)
            if te <= min_message_tokens:
                continue
            target = max(min_message_tokens, te - over)
            new_c = truncate_text_to_tokens(c, target)
            if new_c != c:
                m["content"] = new_c
                progressed = True
                break
        if not progressed:
            break

    for _ in range(len(msgs) * 8):
        if total_est() <= budget_tokens:
            break
        over = total_est() - budget_tokens
        if over <= 0:
            break
        best_i = -1
        best_te = 0
        for i, m in enumerate(msgs):
            c = m.get("content")
            if not isinstance(c, str):
                continue
            te = estimate_tokens(c)
            if te > best_te:
                best_te = te
                best_i = i
        if best_i < 0 or best_te <= 1:
            break
        m = msgs[best_i]
        c = m.get("content")
        assert isinstance(c, str)
        m["content"] = truncate_text_to_tokens(c, max(1, best_te - over))

    return msgs


def fit_chat_messages_for_request(
    messages: list[dict[str, Any]],
) -> tuple[list[dict[str, Any]], int]:
    """
    Ensure messages fit the configured prompt budget. Returns (messages, prompt_token_budget).
    """
    budget = prompt_token_budget()
    shrunk = shrink_messages_to_budget(messages, budget)
    return shrunk, budget


def fit_dependency_outputs_to_budget(chunks: list[str], max_tokens: int) -> list[str]:
    """
    Truncate dependency outputs so the combined estimate (with separators) fits max_tokens.
    Uses proportional scaling when over budget.
    """
    if not chunks:
        return []
    if max_tokens <= 0:
        return [truncate_text_to_tokens(c, 0) for c in chunks]
    sep = "\n---\n"
    sep_t = estimate_tokens(sep) * max(0, len(chunks) - 1)
    body_budget = max(64, max_tokens - sep_t)
    estimates = [max(1, estimate_tokens(c)) for c in chunks]
    total_est = sum(estimates)
    if total_est <= body_budget:
        return list(chunks)
    scale = body_budget / total_est
    out: list[str] = []
    for c, te in zip(chunks, estimates):
        allow = max(48, int(te * scale))
        out.append(truncate_text_to_tokens(c, allow))
    return out


def subdag_parent_output_max_tokens() -> int:
    """Max tokens for parent task output embedded in sub-DAG expansion prompts."""
    raw = (os.getenv("AGENT_PLATFORM_SUBDAG_PARENT_MAX_TOKENS") or "4000").strip()
    try:
        return max(256, int(raw))
    except ValueError:
        return 4000


def dependency_context_token_budget(
    *,
    system_message: str,
    instructions_and_preamble: str,
) -> int:
    """
    Tokens available for the dependency block given system + user base text.
    """
    budget = prompt_token_budget()
    fixed = estimate_tokens(system_message) + estimate_tokens(instructions_and_preamble)
    fixed += _MESSAGE_OVERHEAD_TOKENS * 2
    header = "\n\nContext from previous steps:\n"
    fixed += estimate_tokens(header)
    return max(256, budget - fixed)
