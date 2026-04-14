"""Centralized JSON error shape and FastAPI exception handlers."""

from __future__ import annotations

import logging
from typing import Any

from fastapi import HTTPException, Request
from fastapi.encoders import jsonable_encoder
from fastapi.exceptions import RequestValidationError
from fastapi.responses import JSONResponse

logger = logging.getLogger("llm_proxy")

ERROR_TYPE = "llm_proxy_error"


def wants_wrapped_json_errors(request: Request) -> bool:
    path = request.url.path
    # Only the embedded OpenAI-compatible surface (/v1/*); do not wrap all Agent Platform /api errors.
    if path.startswith("/v1"):
        return True
    accept = (request.headers.get("accept") or "").lower()
    return "application/json" in accept


def get_request_id(request: Request) -> str | None:
    rid = getattr(request.state, "request_id", None)
    return str(rid) if rid else None


def json_error_payload(
    message: str,
    code: str,
    *,
    request_id: str | None = None,
    extra: dict[str, Any] | None = None,
) -> dict[str, Any]:
    err: dict[str, Any] = {
        "message": message,
        "type": ERROR_TYPE,
        "code": code,
    }
    if request_id:
        err["request_id"] = request_id
    if extra:
        err["extra"] = extra
    return {"error": err}


def json_error_response(
    status_code: int,
    message: str,
    code: str,
    *,
    request_id: str | None = None,
    extra: dict[str, Any] | None = None,
) -> JSONResponse:
    return JSONResponse(
        status_code=status_code,
        content=json_error_payload(message, code, request_id=request_id, extra=extra),
    )


class LlmProxyError(Exception):
    """Raised for embedded LLM proxy failures; handled globally into json_error_payload."""

    def __init__(
        self,
        status_code: int,
        code: str,
        message: str,
        *,
        extra: dict[str, Any] | None = None,
    ) -> None:
        self.status_code = status_code
        self.code = code
        self.message = message
        self.extra = extra
        super().__init__(message)


def _detail_to_message(detail: Any) -> str:
    if detail is None:
        return "Error"
    if isinstance(detail, str):
        return detail
    try:
        return str(jsonable_encoder(detail))
    except Exception:
        return "Error"


def register_exception_handlers(app: Any) -> None:
    @app.exception_handler(LlmProxyError)
    async def llm_proxy_error_handler(request: Request, exc: LlmProxyError) -> JSONResponse:
        rid = get_request_id(request)
        logger.warning(
            "llm_proxy_error code=%s status=%s request_id=%s",
            exc.code,
            exc.status_code,
            rid,
        )
        return json_error_response(
            exc.status_code,
            exc.message,
            exc.code,
            request_id=rid,
            extra=exc.extra,
        )

    @app.exception_handler(HTTPException)
    async def http_exception_handler(request: Request, exc: HTTPException) -> JSONResponse | Any:
        if not wants_wrapped_json_errors(request):
            from fastapi.exception_handlers import http_exception_handler as default_http

            return await default_http(request, exc)

        rid = get_request_id(request)
        code = {
            401: "unauthorized",
            403: "forbidden",
            404: "not_found",
            400: "bad_request",
            502: "bad_gateway",
            503: "service_unavailable",
            500: "internal_error",
        }.get(exc.status_code, "http_error")
        msg = _detail_to_message(exc.detail)
        logger.info(
            "http_exception status=%s code=%s request_id=%s detail=%s",
            exc.status_code,
            code,
            rid,
            msg[:200],
        )
        return json_error_response(exc.status_code, msg, code, request_id=rid)

    @app.exception_handler(RequestValidationError)
    async def validation_exception_handler(request: Request, exc: RequestValidationError) -> JSONResponse | Any:
        if not wants_wrapped_json_errors(request):
            from fastapi.exception_handlers import request_validation_exception_handler

            return await request_validation_exception_handler(request, exc)

        rid = get_request_id(request)
        body = jsonable_encoder(exc.errors())
        logger.info("validation_error request_id=%s errors=%s", rid, str(body)[:500])
        return json_error_response(
            422,
            "Request validation failed",
            "validation_error",
            request_id=rid,
            extra={"errors": body},
        )

    @app.exception_handler(Exception)
    async def unhandled_exception_handler(request: Request, exc: Exception) -> JSONResponse | Any:
        if not wants_wrapped_json_errors(request):
            logger.exception("unhandled_exception path=%s", request.url.path)
            raise exc

        rid = get_request_id(request)
        logger.exception("unhandled_exception request_id=%s path=%s", rid, request.url.path)
        return json_error_response(
            500,
            "An unexpected error occurred.",
            "internal_error",
            request_id=rid,
        )
