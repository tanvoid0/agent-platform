"""Unit tests for MCP Streamable HTTP helpers (no live server)."""

import asyncio
import json
import os
from unittest.mock import AsyncMock, patch

from mcp.types import CallToolResult, TextContent

from mcp_streamable_client import serialize_call_tool_result
from tool_handlers import run_tool_async, tools_filtered_by_allowlist


def test_serialize_call_tool_result_text():
    r = CallToolResult(content=[TextContent(type="text", text="hello")], isError=False)
    out = json.loads(serialize_call_tool_result(r))
    assert out["text"] == "hello"
    assert out["isError"] is False


def test_run_tool_async_echo():
    async def _():
        out = await run_tool_async("echo", json.dumps({"text": "x"}))
        assert json.loads(out)["echo"] == "x"

    asyncio.run(_())


def test_mcp_call_disabled_without_allowlist():
    async def _():
        out = await run_tool_async(
            "mcp_call",
            json.dumps({"endpoint": "http://127.0.0.1:1/mcp", "tool_name": "t"}),
        )
        assert json.loads(out)["error"] == "mcp_disabled"

    asyncio.run(_())


@patch.dict(os.environ, {"AGENT_PLATFORM_MCP_ENDPOINT_ALLOWLIST": "http://127.0.0.1:9/mcp"}, clear=False)
def test_mcp_call_endpoint_not_allowlisted():
    async def _():
        out = await run_tool_async(
            "mcp_call",
            json.dumps({"endpoint": "http://127.0.0.1:8/mcp", "tool_name": "t"}),
        )
        assert json.loads(out)["error"] == "endpoint_not_allowlisted"

    asyncio.run(_())


@patch.dict(os.environ, {"AGENT_PLATFORM_MCP_ENDPOINT_ALLOWLIST": "http://127.0.0.1:9/mcp"}, clear=False)
@patch("tool_handlers.call_mcp_tool_streamable", new_callable=AsyncMock)
def test_mcp_call_invokes_streamable_client(mock_call):
    mock_call.return_value = '{"ok": true}'

    async def _():
        out = await run_tool_async(
            "mcp_call",
            json.dumps(
                {
                    "endpoint": "http://127.0.0.1:9/mcp",
                    "tool_name": "echo",
                    "arguments": {"a": 1},
                }
            ),
        )
        assert json.loads(out) == {"ok": True}
        mock_call.assert_awaited_once_with(
            "http://127.0.0.1:9/mcp",
            "echo",
            {"a": 1},
        )

    asyncio.run(_())


def test_tools_filtered_includes_mcp_tools():
    tools = tools_filtered_by_allowlist(
        frozenset({"mcp_call", "mcp_list_tools", "chat_completions"})
    )
    names = {t["function"]["name"] for t in tools}
    assert names == {"mcp_call", "mcp_list_tools", "chat_completions"}


def test_chat_completions_missing_key():
    async def _():
        with patch.dict(
            os.environ,
            {"ORCHESTRATOR_MASTER_KEY": ""},
            clear=False,
        ):
            out = await run_tool_async(
                "chat_completions",
                json.dumps({"model": "local", "messages": [{"role": "user", "content": "hi"}]}),
            )
        assert json.loads(out)["error"] == "missing_key"

    asyncio.run(_())
