"""
Built-in tool implementations for Phase 3 (OpenAI-compatible tool_calls).

Register names here; `llm_client` exposes OpenAPI tool definitions filtered by allowlist.

Includes MCP-parity helpers (legacy tool names) and `http_fetch`
with a strict URL prefix allowlist (`AGENT_PLATFORM_HTTP_FETCH_ALLOWLIST`).

`mcp_call` / `mcp_list_tools` use the official MCP Streamable HTTP client (`mcp` package)
against URLs allowed by `AGENT_PLATFORM_MCP_ENDPOINT_ALLOWLIST`. Optional nested
`chat_completions` calls the embedded LLM proxy (high spend risk; allowlist only).

`workspace_list` / `workspace_read` / `workspace_write` use `ToolContext.project_id` from the
DAG executor (never model-supplied ids); omit or null project → JSON error from the handler.
"""

from __future__ import annotations

import json
import os
from typing import Any
from urllib.parse import urlparse, urlunparse

import httpx

from context_budget import fit_chat_messages_for_request, max_output_tokens_default
from dag_schema import sanitize_llm_model_alias
from mcp_streamable_client import (
    call_mcp_tool_streamable,
    list_mcp_tools_streamable,
    mcp_endpoint_allowlist_prefixes,
)

from llm_proxy_env import (
    llm_proxy_base_url_v1,
    llm_proxy_http_timeout_seconds,
    llm_proxy_master_key,
)
from tool_context import ToolContext
from document_service import read_workspace_file_for_llm
from workspace_service import (
    WorkspaceError,
    ensure_process_workspace,
    list_dir,
    normalize_relative_path,
    read_text_file,
    write_text_file,
)


def _llm_proxy_default_model_from_env() -> str | None:
    """SUBAGENT_MODEL, else PLANNER_MODEL — same resolution as llm_client (no import cycle)."""
    sub = (os.getenv("SUBAGENT_MODEL") or "").strip()
    if sub:
        m = sanitize_llm_model_alias(sub)
        if m:
            return m
    plan = (os.getenv("PLANNER_MODEL") or "").strip()
    if plan:
        return sanitize_llm_model_alias(plan)
    return None


def _llm_proxy_origin() -> str:
    """LLM proxy base URL without /v1 (same convention as llm_client)."""
    raw = llm_proxy_base_url_v1().rstrip("/")
    if raw.endswith("/v1"):
        raw = raw[: -len("/v1")].rstrip("/")
    return raw or "http://127.0.0.1:18410"


def http_fetch_allowlist_prefixes() -> list[str]:
    """Non-empty entries from AGENT_PLATFORM_HTTP_FETCH_ALLOWLIST (comma-separated URL prefixes)."""
    raw = (os.getenv("AGENT_PLATFORM_HTTP_FETCH_ALLOWLIST") or "").strip()
    if not raw:
        return []
    return [x.strip() for x in raw.split(",") if x.strip()]


def http_fetch_max_bytes() -> int:
    raw = (os.getenv("AGENT_PLATFORM_HTTP_FETCH_MAX_BYTES") or "262144").strip()
    try:
        return max(1024, min(2_097_152, int(raw)))
    except ValueError:
        return 262_144


def http_fetch_timeout_seconds() -> float:
    raw = (os.getenv("AGENT_PLATFORM_HTTP_FETCH_TIMEOUT_SECONDS") or "30").strip()
    try:
        return max(1.0, min(120.0, float(raw)))
    except ValueError:
        return 30.0


def url_allowed_for_http_fetch(url: str, prefixes: list[str] | None = None) -> bool:
    """
    True if `url` is http(s) with host and matches one of the allowlist prefixes.
    Used by tests and `http_fetch`.
    """
    if prefixes is None:
        prefixes = http_fetch_allowlist_prefixes()
    if not prefixes:
        return False
    try:
        u = urlparse(url.strip())
    except Exception:
        return False
    if u.scheme not in ("http", "https") or not u.netloc:
        return False
    normalized = urlunparse((u.scheme, u.netloc, u.path or "", "", "", ""))
    for pref in prefixes:
        p = pref.strip()
        if not p:
            continue
        if "://" not in p:
            p = "http://" + p
        try:
            ap = urlparse(p)
        except Exception:
            continue
        if ap.scheme not in ("http", "https") or not ap.netloc:
            continue
        prefix_norm = urlunparse((ap.scheme, ap.netloc, ap.path or "", "", "", "")).rstrip("/")
        if not prefix_norm:
            continue
        n = normalized.rstrip("/")
        if n == prefix_norm or n.startswith(prefix_norm + "/"):
            return True
    return False


def openapi_tool_definitions() -> list[dict[str, Any]]:
    """All tools the executor may advertise when allowlisted."""
    return [
        {
            "type": "function",
            "function": {
                "name": "echo",
                "description": (
                    "Returns the provided text unchanged. Useful for testing tool wiring "
                    "and deterministic sub-steps."
                ),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "text": {
                            "type": "string",
                            "description": "Text to return verbatim.",
                        },
                    },
                    "required": ["text"],
                },
            },
        },
        {
            "type": "function",
            "function": {
                "name": "http_fetch",
                "description": (
                    "HTTP GET or POST to an allowlisted URL only. Requires "
                    "AGENT_PLATFORM_HTTP_FETCH_ALLOWLIST (comma-separated URL prefixes). "
                    "Redirects are not followed. Response body is truncated to "
                    "AGENT_PLATFORM_HTTP_FETCH_MAX_BYTES."
                ),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "Full http(s) URL under an allowlisted prefix.",
                        },
                        "method": {
                            "type": "string",
                            "enum": ["GET", "POST"],
                            "description": "HTTP method (default GET).",
                        },
                        "json_body": {
                            "type": "object",
                            "description": "Optional JSON object for POST body (Content-Type: application/json).",
                        },
                    },
                    "required": ["url"],
                },
            },
        },
        {
            "type": "function",
            "function": {
                "name": "llm_proxy_health",
                "description": (
                    "GET the embedded LLM proxy /v1/health (no API key). "
                    "Uses LLM_ORCHESTRATOR_BASE_URL."
                ),
                "parameters": {"type": "object", "properties": {}},
            },
        },
        {
            "type": "function",
            "function": {
                "name": "list_models",
                "description": (
                    "GET /v1/models from the embedded LLM proxy. "
                    "Uses AGENT_PLATFORM_MASTER_KEY when set. "
                    "Optional providers/provider filter the catalog (e.g. providers=all for full catalog)."
                ),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "providers": {
                            "description": (
                                "Omit for default (effective provider only). Use the string \"all\" for every "
                                "alias plus live Ollama tags and LM Studio models, or an array of "
                                "ollama, lm_studio, aimlapi, and/or gemini."
                            ),
                            "oneOf": [
                                {"type": "string", "enum": ["all"]},
                                {
                                    "type": "array",
                                    "items": {
                                        "type": "string",
                                        "enum": ["ollama", "gemini", "lm_studio", "aimlapi"],
                                    },
                                },
                            ],
                        },
                        "provider": {
                            "type": "string",
                            "enum": ["ollama", "gemini", "lm_studio", "aimlapi"],
                            "description": "Single-provider filter when providers is omitted.",
                        },
                    },
                },
            },
        },
        {
            "type": "function",
            "function": {
                "name": "llm_proxy_connection_info",
                "description": (
                    "Show LLM proxy base URL and whether the API key is set "
                    "(no secret values)."
                ),
                "parameters": {"type": "object", "properties": {}},
            },
        },
        {
            "type": "function",
            "function": {
                "name": "mcp_call",
                "description": (
                    "Call a tool on a remote MCP server using Streamable HTTP (JSON-RPC). "
                    "Requires AGENT_PLATFORM_MCP_ENDPOINT_ALLOWLIST (URL prefixes). "
                    "Optional AGENT_PLATFORM_MCP_AUTHORIZATION for the HTTP client. "
                    "Performs initialize + tools/call for each invocation."
                ),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "endpoint": {
                            "type": "string",
                            "description": "Full MCP endpoint URL (e.g. http://127.0.0.1:18409/mcp).",
                        },
                        "tool_name": {"type": "string", "description": "Server tool name."},
                        "arguments": {
                            "type": "object",
                            "description": "Arguments object for tools/call (omit or {} if none).",
                        },
                    },
                    "required": ["endpoint", "tool_name"],
                },
            },
        },
        {
            "type": "function",
            "function": {
                "name": "mcp_list_tools",
                "description": (
                    "List tools from a remote MCP server (Streamable HTTP). "
                    "Same allowlist and auth as mcp_call."
                ),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "endpoint": {
                            "type": "string",
                            "description": "Full MCP endpoint URL.",
                        },
                    },
                    "required": ["endpoint"],
                },
            },
        },
        {
            "type": "function",
            "function": {
                "name": "chat_completions",
                "description": (
                    "HIGH COST / SPEND RISK: POST /v1/chat/completions to the embedded LLM proxy "
                    "(nested LLM). Requires AGENT_PLATFORM_MASTER_KEY. "
                    "OpenAI-compatible chat_completions body."
                ),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "model": {"type": "string", "description": "Model alias from config.yaml."},
                        "messages": {
                            "type": "array",
                            "description": "OpenAI-style messages.",
                        },
                        "temperature": {"type": "number"},
                        "max_tokens": {"type": "integer"},
                        "top_p": {"type": "number"},
                        "response_format": {"type": "object"},
                    },
                    "required": ["model", "messages"],
                },
            },
        },
        {
            "type": "function",
            "function": {
                "name": "workspace_list",
                "description": (
                    "List files and subdirectories in this run's workspace. Requires the orchestration "
                    "process to be tied to a project. Paths are relative to the run folder "
                    "processes/<process_id>/ (not the whole server filesystem)."
                ),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Directory path relative to project root; omit or empty for root.",
                        },
                    },
                },
            },
        },
        {
            "type": "function",
            "function": {
                "name": "workspace_read",
                "description": (
                    "Read a UTF-8 text file from this run's folder under processes/<process_id>/. "
                    "Requires the process to be tied to a project."
                ),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "File path relative to project root.",
                        },
                    },
                    "required": ["path"],
                },
            },
        },
        {
            "type": "function",
            "function": {
                "name": "workspace_write",
                "description": (
                    "Create or overwrite a UTF-8 text file under processes/<process_id>/. "
                    "Parent directories are created as needed. Requires the process to be tied to a project."
                ),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "File path relative to project root.",
                        },
                        "content": {
                            "type": "string",
                            "description": "Full file contents to write.",
                        },
                    },
                    "required": ["path", "content"],
                },
            },
        },
    ]


def tools_filtered_by_allowlist(allowed: frozenset[str]) -> list[dict[str, Any]]:
    return [t for t in openapi_tool_definitions() if t["function"]["name"] in allowed]


def _run_http_fetch(args: dict[str, Any]) -> str:
    prefixes = http_fetch_allowlist_prefixes()
    if not prefixes:
        return json.dumps(
            {
                "error": "http_fetch_disabled",
                "detail": "Set AGENT_PLATFORM_HTTP_FETCH_ALLOWLIST to non-empty comma-separated URL prefixes.",
            }
        )

    url = args.get("url", "")
    if not isinstance(url, str) or not url.strip():
        return json.dumps({"error": "invalid_url", "detail": "url must be a non-empty string"})

    url = url.strip()
    if not url_allowed_for_http_fetch(url, prefixes):
        return json.dumps({"error": "url_not_allowlisted", "url": url})

    method = (args.get("method") or "GET").upper()
    if method not in ("GET", "POST"):
        return json.dumps({"error": "invalid_method", "detail": "Use GET or POST"})

    timeout = http_fetch_timeout_seconds()
    max_bytes = http_fetch_max_bytes()
    body_json = args.get("json_body")

    try:
        with httpx.Client(timeout=timeout, follow_redirects=False) as client:
            if method == "GET":
                r = client.get(url)
            else:
                if body_json is not None and not isinstance(body_json, dict):
                    return json.dumps({"error": "json_body_must_be_object"})
                r = client.post(url, json=body_json if isinstance(body_json, dict) else None)
    except httpx.RequestError as e:
        return json.dumps({"error": "request_failed", "detail": str(e)})

    raw = r.content
    truncated = len(raw) > max_bytes
    if truncated:
        raw = raw[:max_bytes]
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError:
        text = raw.decode("utf-8", errors="replace")

    ct = r.headers.get("content-type", "")
    return json.dumps(
        {
            "status_code": r.status_code,
            "content_type": ct,
            "body": text,
            "truncated": truncated,
        }
    )


def _v1_models_query_params(args: dict[str, Any]) -> list[tuple[str, str]]:
    """Query string for GET /v1/models."""
    raw_providers = args.get("providers")
    if raw_providers is not None:
        if isinstance(raw_providers, str):
            s = raw_providers.strip().lower()
            if s == "all":
                return [("providers", "all")]
            if s in ("ollama", "gemini", "lm_studio", "aimlapi"):
                return [("providers", s)]
        if isinstance(raw_providers, list):
            out: list[tuple[str, str]] = []
            for p in raw_providers:
                if isinstance(p, str) and p.strip():
                    pl = p.strip().lower()
                    if pl in ("ollama", "gemini", "lm_studio", "aimlapi"):
                        out.append(("providers", pl))
            if out:
                return out
    prov = args.get("provider")
    if isinstance(prov, str) and prov.strip():
        p = prov.strip().lower()
        if p in ("ollama", "gemini", "lm_studio", "aimlapi"):
            return [("provider", p)]
    return []


def _run_llm_proxy_health() -> str:
    base = _llm_proxy_origin()
    try:
        with httpx.Client(timeout=20.0) as client:
            r = client.get(f"{base}/v1/health")
        body = r.text
        try:
            parsed = r.json()
            body_out = json.dumps(parsed, indent=2)[:24000]
        except Exception:
            body_out = body[:24000]
        return json.dumps(
            {"status_code": r.status_code, "url": str(r.request.url), "body": body_out},
            indent=2,
        )
    except httpx.RequestError as e:
        return json.dumps({"error": str(e), "base_url": base}, indent=2)


def _run_list_models(args: dict[str, Any] | None = None) -> str:
    args = args or {}
    base = _llm_proxy_origin()
    headers: dict[str, str] = {"Content-Type": "application/json"}
    key = llm_proxy_master_key()
    if key:
        headers["Authorization"] = f"Bearer {key}"
    qparams = _v1_models_query_params(args)
    try:
        with httpx.Client(timeout=30.0) as client:
            r = client.get(f"{base}/v1/models", headers=headers, params=qparams or None)
        body = r.text
        try:
            parsed = r.json()
            body_out = json.dumps(parsed, indent=2)[:24000]
        except Exception:
            body_out = body[:24000]
        return json.dumps(
            {"status_code": r.status_code, "url": str(r.request.url), "body": body_out},
            indent=2,
        )
    except httpx.RequestError as e:
        return json.dumps({"error": str(e), "base_url": base}, indent=2)


def _run_llm_proxy_connection_info() -> str:
    origin = _llm_proxy_origin()
    base_v1 = llm_proxy_base_url_v1()
    return json.dumps(
        {
            "llm_proxy_origin": origin,
            "llm_proxy_base_url_v1": base_v1,
            "llm_proxy_api_key_set": bool(llm_proxy_master_key()),
            "openai_base_for_clients": base_v1,
        },
        indent=2,
    )


async def _run_chat_completions_async(args: dict[str, Any]) -> str:
    """POST /v1/chat/completions on the embedded LLM proxy."""
    base = _llm_proxy_origin()
    key = llm_proxy_master_key()
    if not key:
        return json.dumps(
            {
                "error": "missing_key",
                "detail": "AGENT_PLATFORM_MASTER_KEY is required for chat_completions.",
            }
        )

    model = args.get("model")
    messages = args.get("messages")
    if not isinstance(model, str) or not model.strip():
        return json.dumps({"error": "invalid_model", "detail": "model must be a non-empty string"})
    if not isinstance(messages, list):
        return json.dumps({"error": "invalid_messages", "detail": "messages must be a JSON array"})

    resolved_model = sanitize_llm_model_alias(model.strip()) or _llm_proxy_default_model_from_env()
    if not resolved_model:
        return json.dumps(
            {
                "error": "invalid_model",
                "detail": (
                    "Model was a role-style slug (e.g. *-expert, *-scaffolder) or invalid; set SUBAGENT_MODEL / PLANNER_MODEL "
                    "in .env or pass a real alias from GET /v1/models."
                ),
            }
        )

    fitted_messages, _ = fit_chat_messages_for_request([dict(m) for m in messages])
    payload: dict[str, Any] = {"model": resolved_model, "messages": fitted_messages}
    if args.get("temperature") is not None:
        payload["temperature"] = args["temperature"]
    if args.get("max_tokens") is not None:
        payload["max_tokens"] = args["max_tokens"]
    else:
        payload["max_tokens"] = max_output_tokens_default()
    if args.get("top_p") is not None:
        payload["top_p"] = args["top_p"]
    if args.get("response_format") is not None:
        payload["response_format"] = args["response_format"]

    headers = {"Content-Type": "application/json", "Authorization": f"Bearer {key}"}
    try:
        async with httpx.AsyncClient(timeout=llm_proxy_http_timeout_seconds()) as client:
            r = await client.post(f"{base}/v1/chat/completions", headers=headers, json=payload)
    except httpx.RequestError as e:
        return json.dumps({"error": "request_failed", "detail": str(e), "base_url": base}, indent=2)

    body = r.text
    try:
        parsed = r.json()
        body_out = json.dumps(parsed, indent=2)[:24000]
    except Exception:
        body_out = body[:24000]
    return json.dumps(
        {"status_code": r.status_code, "url": str(r.request.url), "body": body_out},
        indent=2,
    )


def _dag_workspace_relative(user_path: str, context: ToolContext) -> str:
    """
    Paths from the model are relative to the run folder processes/<process_id>/ when the DAG
    has both project_id and process_id; otherwise relative to the project root.
    """
    raw = user_path if isinstance(user_path, str) else ""
    inner = normalize_relative_path(raw)
    if context.project_id is None:
        return inner
    if context.process_id is not None:
        prefix = f"processes/{context.process_id}"
        if not inner:
            return prefix
        return f"{prefix}/{inner}"
    return inner


def _run_workspace_tools(name: str, args: dict[str, Any], context: ToolContext | None) -> str:
    if context is None or context.project_id is None:
        return json.dumps(
            {
                "error": "no_project_context",
                "detail": "Associate this process with a project to use workspace tools.",
            }
        )
    pid = context.project_id
    try:
        if context.process_id is not None:
            ensure_process_workspace(pid, context.process_id)
        if name == "workspace_list":
            rel = args.get("path", "")
            if not isinstance(rel, str):
                rel = ""
            rel = _dag_workspace_relative(rel, context)
            entries = list_dir(pid, rel)
            return json.dumps(
                {"entries": [{"name": e.name, "path": e.path, "type": e.kind} for e in entries]}
            )
        if name == "workspace_read":
            path = args.get("path", "")
            if not isinstance(path, str) or not path.strip():
                return json.dumps({"error": "invalid_arguments", "detail": "path is required"})
            full = _dag_workspace_relative(path.strip(), context)
            payload = read_workspace_file_for_llm(pid, full)
            return json.dumps(payload)
        if name == "workspace_write":
            path = args.get("path", "")
            content = args.get("content", "")
            if not isinstance(path, str) or not path.strip():
                return json.dumps({"error": "invalid_arguments", "detail": "path is required"})
            if not isinstance(content, str):
                content = str(content)
            full = _dag_workspace_relative(path.strip(), context)
            write_text_file(pid, full, content)
            return json.dumps({"ok": True, "path": full})
    except WorkspaceError as e:
        return json.dumps({"error": e.code, "message": e.message})
    return json.dumps({"error": "unknown_workspace_op", "name": name})


def _parse_tool_args(arguments_json: str) -> tuple[dict[str, Any] | None, str | None]:
    """Returns (args, error_json) where error_json is set on parse failure."""
    try:
        args = json.loads(arguments_json) if arguments_json.strip() else {}
    except json.JSONDecodeError as e:
        return None, json.dumps({"error": "invalid_arguments_json", "detail": str(e)})
    if not isinstance(args, dict):
        return None, json.dumps({"error": "arguments_must_be_object"})
    return args, None


def run_tool(name: str, arguments_json: str) -> str:
    """
    Execute a tool by name. `arguments_json` is the JSON string from the model.
    Returns a string (often JSON) for the assistant message `tool` role.
    """
    args, err = _parse_tool_args(arguments_json)
    if err is not None:
        return err
    assert args is not None

    if name == "echo":
        text = args.get("text", "")
        if not isinstance(text, str):
            text = str(text)
        return json.dumps({"echo": text})

    if name == "http_fetch":
        return _run_http_fetch(args)

    if name == "llm_proxy_health":
        return _run_llm_proxy_health()
    if name == "orchestrator_health":
        return _run_llm_proxy_health()

    if name == "list_models":
        return _run_list_models(args)

    if name == "llm_proxy_connection_info":
        return _run_llm_proxy_connection_info()
    if name == "orchestrator_connection_info":
        return _run_llm_proxy_connection_info()

    if name in ("mcp_call", "mcp_list_tools", "chat_completions"):
        return json.dumps(
            {
                "error": "use_async_tool_path",
                "detail": f"{name} must be invoked via run_tool_async in the LLM loop.",
            }
        )

    if name in ("workspace_list", "workspace_read", "workspace_write"):
        return json.dumps(
            {
                "error": "workspace_tools_require_context",
                "detail": "Workspace tools must run inside the DAG executor with a project-scoped process.",
            }
        )

    return json.dumps({"error": "unknown_tool", "name": name})


async def run_tool_async(
    name: str,
    arguments_json: str,
    context: ToolContext | None = None,
) -> str:
    """
    Async tool execution for the LLM loop (MCP Streamable HTTP + chat_completions).
    Other tools delegate to synchronous `run_tool`.
    """
    args, err = _parse_tool_args(arguments_json)
    if err is not None:
        return err
    assert args is not None

    if name in ("workspace_list", "workspace_read", "workspace_write"):
        return _run_workspace_tools(name, args, context)

    if name == "mcp_call":
        prefixes = mcp_endpoint_allowlist_prefixes()
        if not prefixes:
            return json.dumps(
                {
                    "error": "mcp_disabled",
                    "detail": "Set AGENT_PLATFORM_MCP_ENDPOINT_ALLOWLIST to comma-separated URL prefixes.",
                }
            )
        endpoint = args.get("endpoint", "")
        tool_name = args.get("tool_name", "")
        if not isinstance(endpoint, str) or not endpoint.strip():
            return json.dumps({"error": "invalid_endpoint"})
        if not isinstance(tool_name, str) or not tool_name.strip():
            return json.dumps({"error": "invalid_tool_name"})
        endpoint = endpoint.strip()
        tool_name = tool_name.strip()
        if not url_allowed_for_http_fetch(endpoint, prefixes):
            return json.dumps({"error": "endpoint_not_allowlisted", "endpoint": endpoint})
        raw_args = args.get("arguments")
        if raw_args is None:
            call_args: dict[str, Any] = {}
        elif isinstance(raw_args, dict):
            call_args = raw_args
        else:
            return json.dumps({"error": "arguments_must_be_object"})
        return await call_mcp_tool_streamable(endpoint, tool_name, call_args)

    if name == "mcp_list_tools":
        prefixes = mcp_endpoint_allowlist_prefixes()
        if not prefixes:
            return json.dumps(
                {
                    "error": "mcp_disabled",
                    "detail": "Set AGENT_PLATFORM_MCP_ENDPOINT_ALLOWLIST to comma-separated URL prefixes.",
                }
            )
        endpoint = args.get("endpoint", "")
        if not isinstance(endpoint, str) or not endpoint.strip():
            return json.dumps({"error": "invalid_endpoint"})
        endpoint = endpoint.strip()
        if not url_allowed_for_http_fetch(endpoint, prefixes):
            return json.dumps({"error": "endpoint_not_allowlisted", "endpoint": endpoint})
        return await list_mcp_tools_streamable(endpoint)

    if name == "chat_completions":
        return await _run_chat_completions_async(args)

    return run_tool(name, arguments_json)
