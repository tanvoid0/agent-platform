from __future__ import annotations

import contextvars
import json
import logging
import os
import sys
import time
import uuid
from datetime import datetime, timezone
from typing import Any

from starlette.middleware.base import BaseHTTPMiddleware
from starlette.requests import Request
from starlette.responses import Response

_request_id_var: contextvars.ContextVar[str | None] = contextvars.ContextVar(
    "request_id", default=None
)
_workspace_id_var: contextvars.ContextVar[str | None] = contextvars.ContextVar(
    "workspace_id", default=None
)
_client_id_var: contextvars.ContextVar[str | None] = contextvars.ContextVar(
    "client_id", default=None
)

_LOGGING_CONFIGURED = False


def _normalize_context_value(value: Any) -> str | None:
    if value is None:
        return None
    text = str(value).strip()
    return text or None


def bind_request_context(
    *,
    request_id: Any | None = None,
    workspace_id: Any | None = None,
    client_id: Any | None = None,
) -> dict[str, contextvars.Token]:
    tokens: dict[str, contextvars.Token] = {}
    if request_id is not None:
        tokens["request_id"] = _request_id_var.set(_normalize_context_value(request_id))
    if workspace_id is not None:
        tokens["workspace_id"] = _workspace_id_var.set(_normalize_context_value(workspace_id))
    if client_id is not None:
        tokens["client_id"] = _client_id_var.set(_normalize_context_value(client_id))
    return tokens


def update_request_context(*, workspace_id: Any | None = None, client_id: Any | None = None) -> None:
    if workspace_id is not None:
        _workspace_id_var.set(_normalize_context_value(workspace_id))
    if client_id is not None:
        _client_id_var.set(_normalize_context_value(client_id))


def reset_request_context(tokens: dict[str, contextvars.Token]) -> None:
    for name, token in reversed(list(tokens.items())):
        if name == "request_id":
            _request_id_var.reset(token)
        elif name == "workspace_id":
            _workspace_id_var.reset(token)
        elif name == "client_id":
            _client_id_var.reset(token)


class StructuredContextFilter(logging.Filter):
    def filter(self, record: logging.LogRecord) -> bool:
        record.request_id = getattr(record, "request_id", None) or _request_id_var.get()
        record.workspace_id = getattr(record, "workspace_id", None) or _workspace_id_var.get()
        record.client_id = getattr(record, "client_id", None) or _client_id_var.get()
        return True


class JsonLogFormatter(logging.Formatter):
    def format(self, record: logging.LogRecord) -> str:
        payload: dict[str, Any] = {
            "timestamp": datetime.fromtimestamp(record.created, tz=timezone.utc).isoformat(),
            "level": record.levelname,
            "logger": record.name,
            "message": record.getMessage(),
        }
        request_id = getattr(record, "request_id", None)
        workspace_id = getattr(record, "workspace_id", None)
        client_id = getattr(record, "client_id", None)
        if request_id:
            payload["request_id"] = request_id
        if workspace_id:
            payload["workspace_id"] = workspace_id
        if client_id:
            payload["client_id"] = client_id
        for key in (
            "event",
            "method",
            "path",
            "route",
            "status_code",
            "duration_ms",
            "remote_addr",
        ):
            value = getattr(record, key, None)
            if value is not None:
                payload[key] = value
        if record.exc_info:
            payload["exc_info"] = self.formatException(record.exc_info)
        return json.dumps(payload, ensure_ascii=True, default=str)


def setup_logging() -> None:
    global _LOGGING_CONFIGURED
    if _LOGGING_CONFIGURED:
        return

    level_name = (os.getenv("AGENT_PLATFORM_LOG_LEVEL") or "INFO").strip().upper()
    level = getattr(logging, level_name, logging.INFO)
    handler = logging.StreamHandler(sys.stdout)
    handler.setFormatter(JsonLogFormatter())
    handler.addFilter(StructuredContextFilter())

    root = logging.getLogger()
    root.handlers = [handler]
    root.setLevel(level)
    _LOGGING_CONFIGURED = True


class RequestLoggingMiddleware(BaseHTTPMiddleware):
    def __init__(self, app, logger_name: str = "agent_platform.request") -> None:
        super().__init__(app)
        self.logger = logging.getLogger(logger_name)

    async def dispatch(self, request: Request, call_next) -> Response:
        request_id = getattr(request.state, "request_id", None) or request.headers.get("X-Request-ID")
        request_id = _normalize_context_value(request_id) or str(uuid.uuid4())
        request.state.request_id = request_id
        client_id = request.headers.get("X-Agent-Platform-Client")
        request.state.client_id = _normalize_context_value(client_id)
        tokens = bind_request_context(request_id=request_id, client_id=client_id)
        started = time.perf_counter()
        route = request.scope.get("route")
        route_path = getattr(route, "path", request.url.path)
        remote_addr = request.client.host if request.client else None

        def _workspace_from_request() -> Any | None:
            state_workspace = getattr(request.state, "workspace_id", None)
            if state_workspace is not None:
                return state_workspace
            return request.path_params.get("workspace_id")

        try:
            response = await call_next(request)
        except Exception:
            update_request_context(
                workspace_id=_workspace_from_request(),
                client_id=getattr(request.state, "client_id", None),
            )
            duration_ms = int((time.perf_counter() - started) * 1000)
            self.logger.exception(
                "request failed",
                extra={
                    "event": "request.failed",
                    "method": request.method,
                    "path": request.url.path,
                    "route": route_path,
                    "duration_ms": duration_ms,
                    "remote_addr": remote_addr,
                    "status_code": 500,
                },
            )
            raise
        else:
            update_request_context(
                workspace_id=_workspace_from_request(),
                client_id=getattr(request.state, "client_id", None),
            )
            duration_ms = int((time.perf_counter() - started) * 1000)
            self.logger.info(
                "request completed",
                extra={
                    "event": "request.completed",
                    "method": request.method,
                    "path": request.url.path,
                    "route": route_path,
                    "duration_ms": duration_ms,
                    "remote_addr": remote_addr,
                    "status_code": response.status_code,
                },
            )
            return response
        finally:
            reset_request_context(tokens)
