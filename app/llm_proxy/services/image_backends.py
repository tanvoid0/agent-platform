"""Image-generation backend registry (kept separate from the chat provider set).

Chat providers (``provider_config.SUPPORTED_PROVIDER_IDS``) assume an
OpenAI-compatible chat/embeddings surface. Image generation is a different
surface (diffusion service, OpenAI ``/v1/images/generations`` shape), so it gets
its own small registry here. The capability router in ``core.capabilities``
consults both, so ``image_generation`` resolves without contaminating chat
discovery, health, or ``/v1/models``.

Add another image backend: give it an id, a ``*_configured()`` check, a base
URL resolver, and rows below.
"""

from __future__ import annotations

from llm_proxy.core.provider_config import _from_env_or_dotenv
from llm_proxy_env import rewrite_upstream_localhost_for_docker

ImageProviderId = str

IMAGE_PROVIDER_IDS: tuple[ImageProviderId, ...] = ("image_local",)
IMAGE_PROVIDER_LABELS: dict[ImageProviderId, str] = {
    "image_local": "Image (local)",
}

# Local diffusion service default model when IMAGE_DEFAULT_MODEL is unset.
DEFAULT_IMAGE_MODEL = "flux.1-schnell"


def image_local_api_base() -> str:
    """Base URL of the local image service. Empty when unset (=> not configured).

    Unlike the chat local backends, there is no localhost default: image
    generation only lights up when the operator explicitly points at a service.
    """
    base = _from_env_or_dotenv("IMAGE_API_BASE")
    if not base:
        return ""
    return rewrite_upstream_localhost_for_docker(base.rstrip("/"))


def image_local_configured() -> bool:
    return bool(image_local_api_base())


def image_default_model() -> str:
    return _from_env_or_dotenv("IMAGE_DEFAULT_MODEL") or DEFAULT_IMAGE_MODEL


def is_image_provider(provider: str) -> bool:
    return (provider or "").strip().lower() in IMAGE_PROVIDER_IDS


_IMAGE_CONFIGURED_CHECKS = {
    "image_local": image_local_configured,
}


def image_provider_configured(provider: str) -> bool:
    check = _IMAGE_CONFIGURED_CHECKS.get((provider or "").strip().lower())
    return bool(check()) if check is not None else False


def image_upstream_url(provider: str) -> str:
    """OpenAI-style images endpoint for the given image provider."""
    pid = (provider or "").strip().lower()
    if pid == "image_local":
        base = image_local_api_base()
        if not base:
            from llm_proxy.core.errors import LlmProxyError

            raise LlmProxyError(503, "image_base_missing", "IMAGE_API_BASE is not set.")
        return f"{base}/v1/images/generations"
    from llm_proxy.core.errors import LlmProxyError

    raise LlmProxyError(500, "invalid_image_provider", "Invalid image provider routing (internal).")
