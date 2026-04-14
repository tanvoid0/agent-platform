"""Cross-cutting concerns: error model and HTTP middleware."""

from llm_proxy.core.errors import (
    LlmProxyError,
    get_request_id,
    json_error_payload,
    json_error_response,
    register_exception_handlers,
    wants_wrapped_json_errors,
)

__all__ = [
    "LlmProxyError",
    "get_request_id",
    "json_error_payload",
    "json_error_response",
    "register_exception_handlers",
    "wants_wrapped_json_errors",
]
