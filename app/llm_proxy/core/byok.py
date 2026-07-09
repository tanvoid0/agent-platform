"""BYOK (bring-your-own-key): route a request through the client's own provider key.

The platform token still gates *access* (see ``require_valid_token`` on the
routes); BYOK only replaces the *upstream* provider credential so the proxy
forwards to the vendor using the caller's key and spends none of its own quota.

Transport is HTTP headers (keeps the secret out of the JSON body and logs, and
works with stock OpenAI SDK clients via ``default_headers``):

    X-BYOK-Provider           required to activate BYOK (e.g. "openai")
    X-BYOK-Api-Key            the caller's upstream key
    X-BYOK-Base-Url           optional; host must be allowlisted (SSRF guard)
    X-BYOK-Anthropic-Version  optional; overrides the anthropic-version pin

A custom base URL is accepted only when its host is on the allowlist (each
provider's canonical host plus any ``BYOK_ALLOWED_HOSTS`` the operator adds), it
is https, and it is not a raw IP -- this blocks pointing the proxy at internal
services. When no base URL is given, the provider's canonical base is used.
"""

from __future__ import annotations

import ipaddress
from dataclasses import dataclass
from typing import Literal
from urllib.parse import urlsplit

from fastapi import Request

from llm_proxy.core.errors import LlmProxyError
from llm_proxy.core.provider_config import _from_env_or_dotenv

BYOKCapability = Literal["chat", "embeddings", "image_generation"]

HEADER_PROVIDER = "x-byok-provider"
HEADER_API_KEY = "x-byok-api-key"
HEADER_BASE_URL = "x-byok-base-url"
HEADER_ANTHROPIC_VERSION = "x-byok-anthropic-version"

_DEFAULT_ANTHROPIC_VERSION = "2023-06-01"


@dataclass(frozen=True)
class BYOKProviderSpec:
    """A key-based upstream a BYOK caller may target."""

    id: str
    label: str
    canonical_base: str  # includes version path (e.g. .../v1); no trailing slash
    canonical_host: str
    auth_style: Literal["bearer", "anthropic"]
    modalities: frozenset[BYOKCapability]
    chat_path: str = "/chat/completions"
    embeddings_path: str = "/embeddings"
    images_path: str = "/images/generations"


# OpenAI-compatible, key-based vendors. Local backends (ollama/lm_studio) are
# intentionally absent: BYOK is about not spending the platform's cloud quota.
_BYOK_PROVIDERS: dict[str, BYOKProviderSpec] = {
    "openai": BYOKProviderSpec(
        id="openai",
        label="OpenAI",
        canonical_base="https://api.openai.com/v1",
        canonical_host="api.openai.com",
        auth_style="bearer",
        modalities=frozenset({"chat", "embeddings", "image_generation"}),
    ),
    "anthropic": BYOKProviderSpec(
        id="anthropic",
        label="Claude",
        canonical_base="https://api.anthropic.com/v1",
        canonical_host="api.anthropic.com",
        auth_style="anthropic",
        # Claude's OpenAI-compat surface has no embeddings endpoint.
        modalities=frozenset({"chat"}),
    ),
    "gemini": BYOKProviderSpec(
        id="gemini",
        label="Gemini",
        canonical_base="https://generativelanguage.googleapis.com/v1beta/openai",
        canonical_host="generativelanguage.googleapis.com",
        auth_style="bearer",
        modalities=frozenset({"chat", "embeddings"}),
    ),
    "aimlapi": BYOKProviderSpec(
        id="aimlapi",
        label="AIMLAPI",
        canonical_base="https://api.aimlapi.com/v1",
        canonical_host="api.aimlapi.com",
        auth_style="bearer",
        modalities=frozenset({"chat", "embeddings"}),
    ),
    "openrouter": BYOKProviderSpec(
        id="openrouter",
        label="OpenRouter",
        canonical_base="https://openrouter.ai/api/v1",
        canonical_host="openrouter.ai",
        auth_style="bearer",
        modalities=frozenset({"chat", "embeddings"}),
    ),
    "groq": BYOKProviderSpec(
        id="groq",
        label="Groq",
        canonical_base="https://api.groq.com/openai/v1",
        canonical_host="api.groq.com",
        auth_style="bearer",
        modalities=frozenset({"chat"}),
    ),
    "mistral": BYOKProviderSpec(
        id="mistral",
        label="Mistral",
        canonical_base="https://api.mistral.ai/v1",
        canonical_host="api.mistral.ai",
        auth_style="bearer",
        modalities=frozenset({"chat", "embeddings"}),
    ),
}

BYOK_PROVIDER_IDS: tuple[str, ...] = tuple(_BYOK_PROVIDERS)


def byok_discovery() -> dict[str, object]:
    """Self-describing BYOK contract for ``GET /v1/capabilities``.

    Lets a client learn up front which providers it may bring a key for, each
    one's modalities and canonical host, and the header names to send — instead
    of discovering support via a failed request.
    """
    return {
        "enabled": True,
        "transport": "headers",
        "headers": {
            "provider": "X-BYOK-Provider",
            "api_key": "X-BYOK-Api-Key",
            "base_url": "X-BYOK-Base-Url",
            "anthropic_version": "X-BYOK-Anthropic-Version",
        },
        "extra_allowed_hosts": sorted(_extra_allowed_hosts()),
        "providers": [
            {
                "id": spec.id,
                "label": spec.label,
                "modalities": sorted(spec.modalities),
                "canonical_host": spec.canonical_host,
            }
            for spec in _BYOK_PROVIDERS.values()
        ],
    }


def _extra_allowed_hosts() -> frozenset[str]:
    """Operator-added hosts (``BYOK_ALLOWED_HOSTS``, comma-separated)."""
    raw = _from_env_or_dotenv("BYOK_ALLOWED_HOSTS")
    if not raw:
        return frozenset()
    return frozenset(h.strip().lower() for h in raw.split(",") if h.strip())


def _allowed_hosts(spec: BYOKProviderSpec) -> frozenset[str]:
    return frozenset({spec.canonical_host}) | _extra_allowed_hosts()


@dataclass(frozen=True)
class BYOKRoute:
    """Resolved BYOK target: where to send and how to authenticate."""

    spec: BYOKProviderSpec
    api_key: str
    base: str  # resolved base (canonical or validated custom), no trailing slash
    anthropic_version: str

    def supports(self, capability: BYOKCapability) -> bool:
        return capability in self.spec.modalities

    def require(self, capability: BYOKCapability) -> None:
        """Raise a structured 501 when this BYOK provider can't serve ``capability``."""
        if capability not in self.spec.modalities:
            raise LlmProxyError(
                501,
                "capability_unavailable",
                f"BYOK provider {self.spec.id} does not support {capability}.",
                extra={
                    "capability": capability,
                    "byok_provider": self.spec.id,
                    "byok_provider_modalities": sorted(self.spec.modalities),
                },
            )

    def url(self, capability: BYOKCapability) -> str:
        if capability == "chat":
            return f"{self.base}{self.spec.chat_path}"
        if capability == "embeddings":
            return f"{self.base}{self.spec.embeddings_path}"
        return f"{self.base}{self.spec.images_path}"

    def outbound_headers(self) -> dict[str, str]:
        headers: dict[str, str] = {
            "Content-Type": "application/json",
            "Authorization": f"Bearer {self.api_key}",
        }
        if self.spec.auth_style == "anthropic":
            # Native header form some Anthropic surfaces expect, alongside Bearer.
            headers["x-api-key"] = self.api_key
            headers["anthropic-version"] = self.anthropic_version
        return headers


def _validate_custom_base(spec: BYOKProviderSpec, raw: str) -> str:
    parts = urlsplit(raw)
    if parts.scheme != "https":
        raise LlmProxyError(
            400,
            "byok_invalid_base_url",
            "X-BYOK-Base-Url must be an https URL.",
        )
    if parts.username or parts.password:
        raise LlmProxyError(
            400,
            "byok_invalid_base_url",
            "X-BYOK-Base-Url must not contain credentials.",
        )
    host = (parts.hostname or "").lower()
    if not host:
        raise LlmProxyError(
            400,
            "byok_invalid_base_url",
            "X-BYOK-Base-Url is missing a host.",
        )
    try:
        ipaddress.ip_address(host)
    except ValueError:
        pass  # hostname, as required
    else:
        raise LlmProxyError(
            403,
            "byok_host_not_allowed",
            "X-BYOK-Base-Url must be a hostname, not an IP address.",
        )
    if host not in _allowed_hosts(spec):
        raise LlmProxyError(
            403,
            "byok_host_not_allowed",
            f"Host {host} is not allowlisted for BYOK. "
            "Set BYOK_ALLOWED_HOSTS to permit additional upstreams.",
            extra={"host": host, "allowed_hosts": sorted(_allowed_hosts(spec))},
        )
    # Preserve any path prefix the caller supplied (regional/proxied endpoints).
    return raw.rstrip("/")


def parse_byok(request: Request) -> BYOKRoute | None:
    """Build a ``BYOKRoute`` from request headers, or ``None`` when BYOK is not requested.

    BYOK activates only when ``X-BYOK-Provider`` is present. A provider without a
    key, an unknown provider, or a disallowed base URL raises a structured error
    rather than silently falling back to the platform's own credentials.
    """
    provider = (request.headers.get(HEADER_PROVIDER) or "").strip().lower()
    if not provider:
        return None

    spec = _BYOK_PROVIDERS.get(provider)
    if spec is None:
        raise LlmProxyError(
            400,
            "byok_unknown_provider",
            f"Unknown BYOK provider '{provider}'.",
            extra={"byok_providers": list(BYOK_PROVIDER_IDS)},
        )

    api_key = (request.headers.get(HEADER_API_KEY) or "").strip()
    if not api_key:
        raise LlmProxyError(
            400,
            "byok_missing_key",
            "X-BYOK-Api-Key is required when X-BYOK-Provider is set.",
        )

    raw_base = (request.headers.get(HEADER_BASE_URL) or "").strip()
    base = _validate_custom_base(spec, raw_base) if raw_base else spec.canonical_base

    version = (
        request.headers.get(HEADER_ANTHROPIC_VERSION) or ""
    ).strip() or _DEFAULT_ANTHROPIC_VERSION

    return BYOKRoute(spec=spec, api_key=api_key, base=base, anthropic_version=version)
