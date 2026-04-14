"""
ADR-style tool policy for future MCP / function-calling (Phase 3).

v1 execution path is prompt-only (no tools). When tools are enabled:
- AGENT_PLATFORM_TOOLS_ENABLED=1
- AGENT_PLATFORM_TOOLS_ALLOWLIST=comma-separated tool ids (empty = deny all)
- AGENT_PLATFORM_TOOL_BUDGET_PER_RUN=max tool invocations per run (default 0 = disabled)

Wire these checks into the executor before any tool call. When tools are enabled, `DAGExecutor`
passes an allowlisted OpenAPI tool list to the LLM and counts invocations against
`AGENT_PLATFORM_TOOL_BUDGET_PER_RUN` minus `Process.tool_invocations_used`.

The `http_fetch` tool additionally requires `AGENT_PLATFORM_HTTP_FETCH_ALLOWLIST` (URL prefixes).
`mcp_call` and `mcp_list_tools` require `AGENT_PLATFORM_MCP_ENDPOINT_ALLOWLIST` and use the
official MCP Streamable HTTP client (`mcp` package). Optional `chat_completions` calls the
embedded LLM proxy (nested LLM; allowlist explicitly). LLM helper tools
(`llm_proxy_health`, `list_models`, …) use the same env as `llm_client`
(`LLM_ORCHESTRATOR_BASE_URL`, `AGENT_PLATFORM_MASTER_KEY`).

`workspace_list`, `workspace_read`, and `workspace_write` operate on the process's linked
project sandbox (`Process.project_id`); add them to the allowlist when tools are enabled.
They require no extra env beyond `AGENT_PLATFORM_WORKSPACE_ROOT` (optional; defaults next to the DB).
"""

from __future__ import annotations

import os
from dataclasses import dataclass


def tools_enabled() -> bool:
    return os.getenv("AGENT_PLATFORM_TOOLS_ENABLED", "").strip() in ("1", "true", "yes")


def tool_allowlist() -> frozenset[str]:
    raw = (os.getenv("AGENT_PLATFORM_TOOLS_ALLOWLIST") or "").strip()
    if not raw:
        return frozenset()
    return frozenset(x.strip() for x in raw.split(",") if x.strip())


def tool_budget_per_run() -> int:
    raw = (os.getenv("AGENT_PLATFORM_TOOL_BUDGET_PER_RUN") or "0").strip()
    try:
        return max(0, int(raw))
    except ValueError:
        return 0


@dataclass(frozen=True)
class ToolPolicy:
    enabled: bool
    allowlist: frozenset[str]
    budget_per_run: int

    def is_allowed(self, tool_id: str) -> bool:
        if not self.enabled:
            return False
        if not self.allowlist:
            return False
        return tool_id.strip() in self.allowlist


def load_policy() -> ToolPolicy:
    return ToolPolicy(
        enabled=tools_enabled(),
        allowlist=tool_allowlist(),
        budget_per_run=tool_budget_per_run(),
    )
