"""Turning raw model output into something a parser or a person can read.

Small enough to have lived inside `workflows/assist.py`, until a second caller
(`action_orchestrator.engine`) needed the same two rules and the alternative was
a second copy that would drift.
"""

from __future__ import annotations

import re

_FENCE = re.compile(r"^```(?:[a-zA-Z0-9_-]+)?\s*(.*?)\s*```$", re.DOTALL)
_THINK = re.compile(r"^\s*<think>.*?</think>", re.DOTALL)


def strip_code_fences(text: str) -> str:
    """Unwrap a ```-fenced block and drop a leading `<think>` section.

    Reasoning models (deepseek-r1, qwen3, …) prefix their answer with inline
    deliberation; it is not the answer. The fence is what a model adds when it
    has been asked for JSON and decided to be helpful about it.
    """
    text = _THINK.sub("", text or "").strip()
    match = _FENCE.match(text)
    return match.group(1) if match else text


def looks_like_machine_output(text: str) -> bool:
    """Is this JSON or a code fence rather than prose?

    Used to decide whether a string may be shown to the user as the assistant's
    own words. A truncated `​```json {"reasoning": …` is worse than saying
    nothing: it reads as the app being broken, and it is what the user sees in a
    review banner or a chat turn.
    """
    return (text or "").strip().startswith(("{", "[", "```", "<think>"))
