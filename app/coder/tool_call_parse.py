"""Recover tool calls that models leak as plain text instead of structured tool_calls."""

from __future__ import annotations

import json
import re
import uuid
from typing import Any

# `<function=read_file>`, `<function=read_file>{...}`, `<function=read_file>...</function>`
LEAKED_TOOL_BLOCK_RE = re.compile(
    r"(?is)<function=(\w+)(?:[^>]*)>(.*?)(?:</function>|(?=<function=)|$)"
)
LEAKED_TOOL_TAG_RE = re.compile(r"(?is)<function=\w+(?:[^>]*)>.*?(?:</function>|(?=<function=)|$)")

KNOWN_TOOLS = frozenset({"read_file", "write_file", "list_dir", "run_command"})


def strip_leaked_tool_syntax(text: str) -> str:
    """Remove pseudo tool-call markup from assistant text."""
    if not text:
        return ""
    return LEAKED_TOOL_TAG_RE.sub("", text).strip()


def _parse_args_blob(blob: str) -> dict[str, Any]:
    blob = blob.strip()
    if not blob:
        return {}
    try:
        parsed = json.loads(blob)
        if isinstance(parsed, dict):
            return parsed
    except json.JSONDecodeError:
        pass
    return {}


def parse_leaked_tool_calls(content: str) -> list[dict[str, Any]]:
    """Parse `<function=name>` markup into the same shape as ``_parse_tool_calls``."""
    if not content or "<function=" not in content.lower():
        return []

    calls: list[dict[str, Any]] = []
    seen: set[str] = set()

    for match in LEAKED_TOOL_BLOCK_RE.finditer(content):
        name = match.group(1)
        if name not in KNOWN_TOOLS or name in seen:
            continue
        seen.add(name)
        args = _parse_args_blob(match.group(2))
        call_id = f"leaked_{uuid.uuid4().hex[:12]}"
        raw = {
            "id": call_id,
            "type": "function",
            "function": {"name": name, "arguments": json.dumps(args)},
        }
        calls.append({"id": call_id, "name": name, "arguments": args, "raw": raw})

    return calls
