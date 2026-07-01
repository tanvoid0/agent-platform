"""Structured exceptions for external API token auth failures.

Each carries an HTTP status + machine-readable `code` so external callers can
branch on `code` (e.g. "TOKEN_REVOKED") instead of string-matching messages.
"""

from __future__ import annotations


class ApiTokenError(Exception):
    status_code: int = 401
    code: str = "TOKEN_ERROR"

    def __init__(self, message: str, token_prefix: str | None = None) -> None:
        self.message = message
        self.token_prefix = token_prefix
        super().__init__(message)


class TokenNotFoundError(ApiTokenError):
    status_code = 401
    code = "TOKEN_INVALID"


class TokenRevokedError(ApiTokenError):
    status_code = 401
    code = "TOKEN_REVOKED"


class TokenHeldError(ApiTokenError):
    status_code = 403
    code = "TOKEN_HELD"


class TokenExpiredError(ApiTokenError):
    status_code = 401
    code = "TOKEN_EXPIRED"


class InsufficientScopeError(ApiTokenError):
    status_code = 403
    code = "INSUFFICIENT_SCOPE"


class RateLimitExceededError(ApiTokenError):
    status_code = 429
    code = "RATE_LIMIT_EXCEEDED"
