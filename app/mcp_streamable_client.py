"""
Streamable HTTP MCP client using the official `mcp` SDK (JSON-RPC over HTTP).

Used by `mcp_call` / `mcp_list_tools` tools when endpoints match
`AGENT_PLATFORM_MCP_ENDPOINT_ALLOWLIST`.
"""

from __future__ import annotations

import json
import os
from typing import Any

import httpx
from mcp import types as mcp_types
from mcp.types import TextContent
from mcp.client.session import ClientSession
from mcp.client.streamable_http import streamable_http_client
from mcp.shared.exceptions import McpError
from mcp.shared._httpx_utils import create_mcp_http_client


def mcp_endpoint_allowlist_prefixes() -> list[str]:
    raw = (os.getenv("AGENT_PLATFORM_MCP_ENDPOINT_ALLOWLIST") or "").strip()
    if not raw:
        return []
    return [x.strip() for x in raw.split(",") if x.strip()]


def mcp_timeout_seconds() -> float:
    raw = (os.getenv("AGENT_PLATFORM_MCP_TIMEOUT_SECONDS") or "120").strip()
    try:
        return max(5.0, min(600.0, float(raw)))
    except ValueError:
        return 120.0


def mcp_authorization_header() -> str | None:
    """Optional Authorization header value for all MCP HTTP requests (e.g. Bearer …)."""
    v = (os.getenv("AGENT_PLATFORM_MCP_AUTHORIZATION") or "").strip()
    return v or None


def _truncate(s: str, max_len: int = 240_000) -> str:
    if len(s) <= max_len:
        return s
    return s[: max_len - 20] + "\n…[truncated]"


def serialize_call_tool_result(result: mcp_types.CallToolResult) -> str:
    """Serialize MCP tools/call result for the assistant/tool message."""
    blocks_out: list[Any] = []
    text_parts: list[str] = []
    for block in result.content:
        dumped = block.model_dump(mode="json")
        blocks_out.append(dumped)
        if isinstance(block, TextContent):
            text_parts.append(block.text)

    payload: dict[str, Any] = {
        "isError": result.isError,
        "content": blocks_out,
        "text": "\n".join(text_parts) if text_parts else "",
    }
    if result.structuredContent is not None:
        payload["structuredContent"] = result.structuredContent
    return _truncate(json.dumps(payload, indent=2))


def _build_http_client(timeout_seconds: float) -> httpx.AsyncClient:
    headers: dict[str, str] = {}
    auth = mcp_authorization_header()
    if auth:
        headers["Authorization"] = auth
    return create_mcp_http_client(
        headers=headers or None,
        timeout=httpx.Timeout(timeout_seconds, read=max(timeout_seconds, 300.0)),
    )


async def call_mcp_tool_streamable(
    endpoint: str,
    tool_name: str,
    arguments: dict[str, Any] | None,
) -> str:
    """
    Connect to `endpoint`, initialize, invoke tools/call, return JSON text for the model.
    """
    timeout_seconds = mcp_timeout_seconds()
    client = _build_http_client(timeout_seconds)
    try:
        async with client:
            async with streamable_http_client(
                endpoint,
                http_client=client,
                terminate_on_close=True,
            ) as (read, write, _get_sid):
                async with ClientSession(
                    read,
                    write,
                    client_info=mcp_types.Implementation(name="agent-platform", version="0.1.0"),
                ) as session:
                    await session.initialize()
                    result = await session.call_tool(tool_name, arguments or {})
                    return serialize_call_tool_result(result)
    except McpError as e:
        return json.dumps(
            {"error": "mcp_error", "code": e.error.code, "message": e.error.message, "data": e.error.data},
            indent=2,
        )
    except Exception as e:
        return json.dumps({"error": "mcp_client_failed", "detail": str(e)}, indent=2)


async def list_mcp_tools_streamable(endpoint: str) -> str:
    """tools/list on the given MCP endpoint; returns JSON list of {name, description, ...}."""
    timeout_seconds = mcp_timeout_seconds()
    client = _build_http_client(timeout_seconds)
    try:
        async with client:
            async with streamable_http_client(
                endpoint,
                http_client=client,
                terminate_on_close=True,
            ) as (read, write, _get_sid):
                async with ClientSession(
                    read,
                    write,
                    client_info=mcp_types.Implementation(name="agent-platform", version="0.1.0"),
                ) as session:
                    await session.initialize()
                    listed = await session.list_tools()
                    tools = [
                        {
                            "name": t.name,
                            "description": t.description,
                            "inputSchema": t.inputSchema,
                        }
                        for t in listed.tools
                    ]
                    return _truncate(json.dumps({"tools": tools}, indent=2))
    except McpError as e:
        return json.dumps(
            {"error": "mcp_error", "code": e.error.code, "message": e.error.message, "data": e.error.data},
            indent=2,
        )
    except Exception as e:
        return json.dumps({"error": "mcp_client_failed", "detail": str(e)}, indent=2)
