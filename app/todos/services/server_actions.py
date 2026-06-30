"""Server-side execution for todo-board actions (webhooks, etc.)."""

from __future__ import annotations

from typing import Any
from urllib.parse import urlparse

import httpx

WEBHOOK_TIMEOUT_SECONDS = 15.0


def _validate_webhook_url(url: str) -> str:
    parsed = urlparse(url.strip())
    if parsed.scheme not in ("http", "https") or not parsed.netloc:
        raise ValueError("webhook_url must be an http(s) URL")
    return url.strip()


def execute_trigger_webhook(
    webhook_url: str,
    payload: dict[str, Any] | None = None,
    *,
    timeout: float = WEBHOOK_TIMEOUT_SECONDS,
) -> dict[str, Any]:
    """POST JSON payload to an external webhook (n8n, Zapier, etc.)."""
    url = _validate_webhook_url(webhook_url)
    body = payload if isinstance(payload, dict) else {}
    with httpx.Client(timeout=timeout) as client:
        response = client.post(url, json=body)
    return {
        "status_code": response.status_code,
        "ok": response.is_success,
        "body_preview": (response.text or "")[:500],
    }
