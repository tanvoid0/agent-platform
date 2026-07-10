"""Capability (modality) contract and capability-based provider routing.

This is the single source of truth for *what each provider can do* — not just
whether it is configured (see ``provider_config``). The proxy consults this
before forwarding a request so it can refuse an unsupported capability with a
structured ``501`` instead of blindly proxying into a backend that will 500.

Add a new modality: extend ``Modality`` and ``_PROVIDER_MODALITIES``.
Add a new provider: give it a row in ``_PROVIDER_MODALITIES`` (missing rows are
treated as chat-only, forward-compatible with unregistered YAML providers).
"""

from __future__ import annotations

from typing import Literal

from llm_proxy.core.errors import LlmProxyError
from llm_proxy.core.provider_config import (
    PROVIDER_LOCAL_SORT_ORDER,
    SUPPORTED_PROVIDER_IDS,
    provider_configured,
)
from llm_proxy.services.image_backends import (
    IMAGE_PROVIDER_IDS,
    image_provider_configured,
    is_image_provider,
)

Modality = Literal["chat", "vision_input", "embeddings", "image_generation"]

MODALITIES: tuple[Modality, ...] = (
    "chat",
    "vision_input",
    "embeddings",
    "image_generation",
)

# Per-provider declared modalities. Keep in sync with each backend's real
# surface — this is what the router trusts.
_PROVIDER_MODALITIES: dict[str, frozenset[Modality]] = {
    "ollama": frozenset({"chat", "vision_input"}),
    "lm_studio": frozenset({"chat", "vision_input", "embeddings"}),
    "aimlapi": frozenset({"chat", "embeddings"}),
    # Claude's OpenAI-compatible surface has no embeddings endpoint (see
    # embeddings route guard); it does accept image inputs.
    "anthropic": frozenset({"chat", "vision_input"}),
    "gemini": frozenset({"chat", "vision_input", "embeddings"}),
    # Image backend (separate registry; see services.image_backends).
    "image_local": frozenset({"image_generation"}),
}

# Chat-only default for providers registered later without a modality row.
_DEFAULT_MODALITIES: frozenset[Modality] = frozenset({"chat"})


def provider_modalities(provider: str) -> frozenset[Modality]:
    """Declared modalities for a provider (chat-only fallback if unregistered)."""
    return _PROVIDER_MODALITIES.get((provider or "").strip().lower(), _DEFAULT_MODALITIES)


def provider_supports(provider: str, capability: Modality) -> bool:
    return capability in provider_modalities(provider)


def modality_map(provider: str) -> dict[str, bool]:
    """Flat ``{modality: bool}`` map for the capability catalog surface."""
    declared = provider_modalities(provider)
    return {modality: modality in declared for modality in MODALITIES}


def _is_configured(provider: str) -> bool:
    """Configured check across both the chat and image registries."""
    if is_image_provider(provider):
        return image_provider_configured(provider)
    return provider_configured(provider)


def _providers_by_local_preference() -> list[str]:
    """All capability providers (chat + image), local backends first."""
    providers = list(SUPPORTED_PROVIDER_IDS) + list(IMAGE_PROVIDER_IDS)
    return sorted(
        providers,
        key=lambda pid: PROVIDER_LOCAL_SORT_ORDER.get(pid, 99),
    )


def resolve_provider_for_capability(capability: Modality) -> str | None:
    """First *configured* provider that declares ``capability``, or ``None``.

    Local backends are preferred so requests stay on-box when possible.
    """
    for provider in _providers_by_local_preference():
        if provider_supports(provider, capability) and _is_configured(provider):
            return provider
    return None


def providers_for_capability(capability: Modality) -> list[str]:
    """All configured providers that declare ``capability`` (preference order)."""
    return [
        provider
        for provider in _providers_by_local_preference()
        if provider_supports(provider, capability) and _is_configured(provider)
    ]


def require_provider_for_capability(
    capability: Modality,
    *,
    preferred: str | None = None,
) -> str:
    """Resolve a provider for ``capability`` or raise a structured ``501``.

    ``preferred`` pins a specific provider when the caller already chose one; it
    is honored only if that provider both declares the capability and is
    configured, otherwise the error names what *is* available.
    """
    pref = (preferred or "").strip().lower()
    if pref:
        if not provider_supports(pref, capability):
            raise LlmProxyError(
                501,
                "capability_unavailable",
                f"Provider {pref} does not support {capability}.",
                extra=_capability_extra(capability),
            )
        if not _is_configured(pref):
            raise LlmProxyError(
                503,
                "provider_not_configured",
                f"Provider {pref} is not configured (check environment for this provider).",
            )
        return pref

    resolved = resolve_provider_for_capability(capability)
    if resolved is None:
        raise LlmProxyError(
            501,
            "capability_unavailable",
            f"No configured provider supports {capability}.",
            extra=_capability_extra(capability),
        )
    return resolved


def _capability_extra(capability: Modality) -> dict[str, object]:
    return {
        "capability": capability,
        "providers_with_capability": [
            provider
            for provider in _providers_by_local_preference()
            if provider_supports(provider, capability)
        ],
        "configured_providers_with_capability": providers_for_capability(capability),
    }
