"""Upstream LLM vendor HTTP: retries, error classification, SSE error chunks."""

from __future__ import annotations

import asyncio
import json
import logging
import os
import random
import re
from typing import Any
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit

import httpx

from llm_proxy.core.errors import LlmProxyError

logger = logging.getLogger("llm_proxy")


def _sanitize_url_for_log(url: str) -> str:
    try:
        parts = urlsplit(url)
        if not parts.query:
            return url
        pairs = [
            (k, "***" if k.lower() in ("key", "api_key", "token", "access_token") else v)
            for k, v in parse_qsl(parts.query, keep_blank_values=True)
        ]
        new_query = urlencode(pairs)
        return urlunsplit((parts.scheme, parts.netloc, parts.path, new_query, parts.fragment))
    except Exception:
        return re.sub(r"([?&])(key|api_key|token)=[^&]*", r"\1\2=***", url, flags=re.I)


def classify_httpx_error(exc: httpx.RequestError, context: str = "") -> tuple[str, str]:
    """Return (machine_code, human_message)."""
    url = ""
    if exc.request is not None:
        url = str(exc.request.url)
    safe_url = _sanitize_url_for_log(url) if url else "(unknown url)"
    host = ""
    try:
        if exc.request is not None:
            host = str(exc.request.url.host or "")
    except Exception:
        pass

    if isinstance(exc, httpx.ConnectError):
        msg = (
            f"Cannot reach upstream at {safe_url} (connection failed). "
            f"{'Is Ollama running?' if 'ollama' in safe_url.lower() or ':11434' in safe_url else 'Check the URL and network.'}"
        )
        return ("connect_failed", msg)
    if isinstance(exc, httpx.ConnectTimeout):
        return ("connect_timeout", f"Connection to upstream timed out: {safe_url}")
    if isinstance(exc, httpx.ReadTimeout):
        return ("read_timeout", f"Upstream read timed out ({context or 'request'}): {safe_url}")
    if isinstance(exc, httpx.WriteTimeout):
        return ("write_timeout", f"Upstream write timed out: {safe_url}")
    if isinstance(exc, httpx.PoolTimeout):
        return ("pool_timeout", "HTTP client pool timed out; retry shortly.")
    if isinstance(exc, httpx.RemoteProtocolError):
        return ("protocol_error", f"Upstream closed the connection unexpectedly ({host or safe_url}).")
    if isinstance(exc, httpx.ProxyError):
        return ("proxy_error", f"HTTP proxy error while contacting {safe_url}")
    if isinstance(exc, httpx.UnsupportedProtocol):
        return ("bad_url", f"Invalid or unsupported URL: {safe_url}")

    return ("transport_error", f"Upstream request failed ({context}): {exc.__class__.__name__}: {safe_url}")


def should_retry_transport(exc: httpx.RequestError, attempt: int, max_attempts: int) -> bool:
    if attempt >= max_attempts - 1:
        return False
    return isinstance(
        exc,
        (
            httpx.ConnectError,
            httpx.ConnectTimeout,
            httpx.ReadTimeout,
            httpx.WriteTimeout,
            httpx.RemoteProtocolError,
            httpx.PoolTimeout,
        ),
    )


# Upstream providers (Ollama, AIML API, etc.) reject request bursts from many
# concurrent agents with these status codes, or with a 4xx whose body names the
# limit explicitly (e.g. AIML API returns "too many concurrent requests").
_RATE_LIMIT_STATUSES = frozenset({429, 503})
_RATE_LIMIT_PHRASES = ("too many concurrent", "rate limit", "rate_limit_exceeded", "too many requests")


def is_rate_limited_response(status_code: int, body_text: str | None) -> bool:
    if status_code in _RATE_LIMIT_STATUSES:
        return True
    if status_code >= 400 and body_text:
        lowered = body_text.lower()
        return any(p in lowered for p in _RATE_LIMIT_PHRASES)
    return False


def llm_proxy_error_from_httpx(exc: httpx.RequestError, context: str = "") -> LlmProxyError:
    code, msg = classify_httpx_error(exc, context)
    return LlmProxyError(502, code, msg)


def sse_error_chunk(code: str, message: str) -> bytes:
    payload = {
        "error": {
            "message": message,
            "type": "llm_proxy_error",
            "param": None,
            "code": code,
        }
    }
    return f"data: {json.dumps(payload)}\n\n".encode("utf-8")


async def aclose_stream(response: httpx.Response | None, client: httpx.AsyncClient | None) -> None:
    if response is not None:
        try:
            await response.aclose()
        except Exception:
            pass
    if client is not None:
        try:
            await client.aclose()
        except Exception:
            pass


class UpstreamHttpClient:
    """httpx AsyncClient calls with transport retries and structured failures."""

    def __init__(
        self,
        *,
        max_retries: int | None = None,
        backoff_ms: int | None = None,
        rate_limit_max_retries: int | None = None,
        rate_limit_backoff_ms: int | None = None,
    ) -> None:
        self._max_retries = max(
            1,
            max_retries if max_retries is not None else int(os.environ.get("ORCHESTRATOR_HTTP_MAX_RETRIES", "3")),
        )
        self._backoff_ms = max(
            10,
            backoff_ms if backoff_ms is not None else int(os.environ.get("ORCHESTRATOR_HTTP_RETRY_BACKOFF_MS", "120")),
        )
        # Separate, more generous budget for "many agents at once" rate-limit responses
        # (HTTP 429/503, or a 4xx body naming the limit) vs. raw connection failures.
        self._rate_limit_max_retries = max(
            1,
            rate_limit_max_retries
            if rate_limit_max_retries is not None
            else int(os.environ.get("ORCHESTRATOR_HTTP_RATE_LIMIT_MAX_RETRIES", "6")),
        )
        self._rate_limit_backoff_ms = max(
            10,
            rate_limit_backoff_ms
            if rate_limit_backoff_ms is not None
            else int(os.environ.get("ORCHESTRATOR_HTTP_RATE_LIMIT_BACKOFF_MS", "400")),
        )

    def _backoff_seconds(self, attempt: int, backoff_ms: int | None = None) -> float:
        base = ((backoff_ms if backoff_ms is not None else self._backoff_ms) / 1000.0) * (2**attempt)
        jitter = random.uniform(0, base * 0.25)
        return min(base + jitter, 30.0)

    def _rate_limit_delay_seconds(self, response: httpx.Response, attempt: int) -> float:
        """Prefer the upstream's own Retry-After hint when present."""
        retry_after = response.headers.get("retry-after")
        if retry_after:
            try:
                return min(max(float(retry_after), 0.0), 30.0)
            except ValueError:
                pass
        return self._backoff_seconds(attempt, self._rate_limit_backoff_ms)

    async def get(
        self,
        url: str,
        *,
        headers: dict[str, str] | None = None,
        params: dict[str, Any] | None = None,
        timeout: float = 30.0,
        context: str = "GET",
    ) -> httpx.Response:
        last: httpx.RequestError | None = None
        for attempt in range(self._max_retries):
            try:
                async with httpx.AsyncClient(timeout=timeout) as client:
                    return await client.get(url, headers=headers, params=params)
            except httpx.RequestError as e:
                last = e
                safe = _sanitize_url_for_log(url)
                if should_retry_transport(e, attempt, self._max_retries):
                    delay = self._backoff_seconds(attempt)
                    logger.warning(
                        "retry %s attempt=%s/%s delay=%.2fs url=%s err=%s",
                        context,
                        attempt + 1,
                        self._max_retries,
                        delay,
                        safe,
                        e.__class__.__name__,
                    )
                    await asyncio.sleep(delay)
                    continue
                logger.warning("get_failed %s url=%s err=%s", context, safe, e.__class__.__name__)
                raise llm_proxy_error_from_httpx(e, context) from e
        assert last is not None
        raise llm_proxy_error_from_httpx(last, context) from last

    async def post(
        self,
        url: str,
        *,
        headers: dict[str, str] | None = None,
        json_body: Any = None,
        timeout: float = 300.0,
        context: str = "POST",
        retry_rate_limits: bool = True,
    ) -> httpx.Response:
        transport_attempt = 0
        rate_limit_attempt = 0
        safe = _sanitize_url_for_log(url)
        while True:
            try:
                async with httpx.AsyncClient(timeout=timeout) as client:
                    response = await client.post(url, headers=headers, json=json_body)
            except httpx.RequestError as e:
                if should_retry_transport(e, transport_attempt, self._max_retries):
                    delay = self._backoff_seconds(transport_attempt)
                    logger.warning(
                        "retry %s attempt=%s/%s delay=%.2fs url=%s err=%s",
                        context,
                        transport_attempt + 1,
                        self._max_retries,
                        delay,
                        safe,
                        e.__class__.__name__,
                    )
                    await asyncio.sleep(delay)
                    transport_attempt += 1
                    continue
                logger.warning("post_failed %s url=%s err=%s", context, safe, e.__class__.__name__)
                raise llm_proxy_error_from_httpx(e, context) from e

            if (
                retry_rate_limits
                and rate_limit_attempt < self._rate_limit_max_retries - 1
                and is_rate_limited_response(response.status_code, response.text)
            ):
                delay = self._rate_limit_delay_seconds(response, rate_limit_attempt)
                logger.warning(
                    "rate_limited %s attempt=%s/%s delay=%.2fs url=%s status=%s",
                    context,
                    rate_limit_attempt + 1,
                    self._rate_limit_max_retries,
                    delay,
                    safe,
                    response.status_code,
                )
                await asyncio.sleep(delay)
                rate_limit_attempt += 1
                continue

            return response

    async def open_stream(
        self,
        url: str,
        *,
        headers: dict[str, str] | None = None,
        json_body: Any = None,
        timeout: float = 300.0,
        context: str = "chat_stream",
    ) -> tuple[httpx.Response, httpx.AsyncClient]:
        """
        Open a streaming POST. Caller must drain the body and call ``aclose_stream``.

        Retries transport failures and upstream rate-limit responses (status only —
        the body is unread at this point, so a 429/503 is closed and retried without
        sniffing the message text).
        """
        transport_attempt = 0
        rate_limit_attempt = 0
        safe = _sanitize_url_for_log(url)
        while True:
            client = httpx.AsyncClient(timeout=timeout)
            try:
                req = client.build_request("POST", url, headers=headers, json=json_body)
                response = await client.send(req, stream=True)
            except httpx.RequestError as e:
                await client.aclose()
                if should_retry_transport(e, transport_attempt, self._max_retries):
                    delay = self._backoff_seconds(transport_attempt)
                    logger.warning(
                        "retry %s attempt=%s/%s delay=%.2fs url=%s err=%s",
                        context,
                        transport_attempt + 1,
                        self._max_retries,
                        delay,
                        safe,
                        e.__class__.__name__,
                    )
                    await asyncio.sleep(delay)
                    transport_attempt += 1
                    continue
                logger.warning("stream_open_failed %s url=%s err=%s", context, safe, e.__class__.__name__)
                raise llm_proxy_error_from_httpx(e, context) from e

            if (
                rate_limit_attempt < self._rate_limit_max_retries - 1
                and response.status_code in _RATE_LIMIT_STATUSES
            ):
                delay = self._rate_limit_delay_seconds(response, rate_limit_attempt)
                logger.warning(
                    "rate_limited %s attempt=%s/%s delay=%.2fs url=%s status=%s",
                    context,
                    rate_limit_attempt + 1,
                    self._rate_limit_max_retries,
                    delay,
                    safe,
                    response.status_code,
                )
                await aclose_stream(response, client)
                await asyncio.sleep(delay)
                rate_limit_attempt += 1
                continue

            return response, client


default_upstream_client = UpstreamHttpClient()

get_with_retry = default_upstream_client.get
post_with_retry = default_upstream_client.post
stream_chat_completion = default_upstream_client.open_stream
