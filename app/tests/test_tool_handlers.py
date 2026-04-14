import json
import os
from unittest.mock import MagicMock, patch

import pytest

from tool_handlers import (
    run_tool,
    tools_filtered_by_allowlist,
    url_allowed_for_http_fetch,
)


def test_echo_tool():
    out = run_tool("echo", json.dumps({"text": "hello"}))
    data = json.loads(out)
    assert data["echo"] == "hello"


def test_unknown_tool():
    out = run_tool("missing", "{}")
    data = json.loads(out)
    assert data["error"] == "unknown_tool"


def test_tools_filtered_by_allowlist():
    tools = tools_filtered_by_allowlist(frozenset({"echo", "http_fetch"}))
    names = {t["function"]["name"] for t in tools}
    assert names == {"echo", "http_fetch"}


@pytest.mark.parametrize(
    ("url", "prefixes", "expected"),
    [
        ("http://127.0.0.1:18410/v1/health", ["http://127.0.0.1:18410"], True),
        ("http://127.0.0.1:18410/v1/models", ["http://127.0.0.1:18410"], True),
        ("https://example.com/api/x", ["https://example.com"], True),
        ("http://evil.com", ["http://127.0.0.1:18410"], False),
        ("ftp://127.0.0.1:18410/x", ["http://127.0.0.1:18410"], False),
    ],
)
def test_url_allowed_for_http_fetch(url, prefixes, expected):
    assert url_allowed_for_http_fetch(url, prefixes) is expected


def test_http_fetch_without_allowlist_env():
    out = run_tool("http_fetch", json.dumps({"url": "http://127.0.0.1:1/"}))
    data = json.loads(out)
    assert data["error"] == "http_fetch_disabled"


@patch.dict(os.environ, {"AGENT_PLATFORM_HTTP_FETCH_ALLOWLIST": "http://127.0.0.1:9"}, clear=False)
def test_http_fetch_not_allowlisted():
    out = run_tool("http_fetch", json.dumps({"url": "http://127.0.0.1:8/x"}))
    data = json.loads(out)
    assert data["error"] == "url_not_allowlisted"


@patch.dict(os.environ, {"AGENT_PLATFORM_HTTP_FETCH_ALLOWLIST": "http://127.0.0.1:9"}, clear=False)
@patch("tool_handlers.httpx.Client")
def test_http_fetch_get(mock_client_cls):
    mock_response = MagicMock()
    mock_response.status_code = 200
    mock_response.content = b'{"ok":true}'
    mock_response.headers = {"content-type": "application/json"}
    mock_response.request.url = "http://127.0.0.1:9/y"

    mock_instance = MagicMock()
    mock_instance.__enter__.return_value = mock_instance
    mock_instance.__exit__.return_value = False
    mock_instance.get.return_value = mock_response
    mock_client_cls.return_value = mock_instance

    out = run_tool("http_fetch", json.dumps({"url": "http://127.0.0.1:9/y"}))
    data = json.loads(out)
    assert data["status_code"] == 200
    assert data["body"] == '{"ok":true}'
    mock_instance.get.assert_called_once()


def test_llm_proxy_connection_info_tool():
    out = run_tool("llm_proxy_connection_info", "{}")
    data = json.loads(out)
    assert "llm_proxy_origin" in data
    assert "llm_proxy_base_url_v1" in data
    assert data["llm_proxy_base_url_v1"].endswith("/v1")
    assert "llm_proxy_api_key_set" in data
