"""ASGI middleware: request correlation id."""

from __future__ import annotations

import uuid

from observability import bind_request_context, reset_request_context
from starlette.middleware.base import BaseHTTPMiddleware
from starlette.requests import Request
from starlette.responses import Response

REQUEST_ID_HEADER = "X-Request-ID"


class RequestIdMiddleware(BaseHTTPMiddleware):
    async def dispatch(self, request: Request, call_next) -> Response:
        existing = getattr(request.state, "request_id", None)
        incoming = request.headers.get(REQUEST_ID_HEADER)
        rid = (
            existing.strip()
            if isinstance(existing, str) and existing.strip()
            else incoming.strip()
            if incoming and incoming.strip()
            else str(uuid.uuid4())
        )
        request.state.request_id = rid
        tokens = bind_request_context(request_id=rid)
        try:
            response = await call_next(request)
        finally:
            reset_request_context(tokens)
        response.headers[REQUEST_ID_HEADER] = rid
        return response
