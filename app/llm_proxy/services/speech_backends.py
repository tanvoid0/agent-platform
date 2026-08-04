"""Text-to-speech backend registry (kept separate from the chat provider set).

Same shape as ``image_backends``: speech is its own upstream surface (OpenAI's
``/v1/audio/speech``), so it gets its own small registry rather than a modality
bolted onto the chat providers. The capability router in ``core.capabilities``
consults it, so ``speech`` resolves without touching chat discovery, health or
``/v1/models``.

The one backend is whatever answers at ``SPEECH_API_BASE`` with the OpenAI
speech shape — a hosted provider, or a local Piper/Kokoro server. The desktop
app's E.V. voice calls this endpoint and falls back to its own engines when
nothing is configured.

Add another speech backend: give it an id, a ``*_configured()`` check, a base
URL resolver, and rows below.
"""

from __future__ import annotations

from llm_proxy.core.provider_config import _from_env_or_dotenv
from llm_proxy_env import rewrite_upstream_localhost_for_docker

SpeechProviderId = str

SPEECH_PROVIDER_IDS: tuple[SpeechProviderId, ...] = ("speech_local",)
SPEECH_PROVIDER_LABELS: dict[SpeechProviderId, str] = {
    "speech_local": "Speech (local)",
}

# Piper's OpenAI-compatible servers answer to this model id; overridable.
DEFAULT_SPEECH_MODEL = "tts-1"
DEFAULT_SPEECH_VOICE = "alloy"
DEFAULT_SPEECH_FORMAT = "mp3"


def speech_local_api_base() -> str:
    """Base URL of the speech service. Empty when unset (=> not configured).

    As with images there is no localhost default: the voice only routes through
    the server once the operator points at something.
    """
    base = _from_env_or_dotenv("SPEECH_API_BASE")
    if not base:
        return ""
    return rewrite_upstream_localhost_for_docker(base.rstrip("/"))


def speech_local_configured() -> bool:
    return bool(speech_local_api_base())


def speech_api_key() -> str:
    """Bearer token for the upstream, empty for a local server that wants none.

    One key for whatever ``SPEECH_API_BASE`` points at, rather than a key per
    named provider: the route already requires an OpenAI-shaped upstream, and
    every hosted one of those authenticates with a bearer token. A backend with
    its own scheme (ElevenLabs' ``xi-api-key``) needs a gateway in front, or its
    own row here.
    """
    return _from_env_or_dotenv("SPEECH_API_KEY") or ""


def speech_default_model() -> str:
    return _from_env_or_dotenv("SPEECH_DEFAULT_MODEL") or DEFAULT_SPEECH_MODEL


def speech_default_voice() -> str:
    return _from_env_or_dotenv("SPEECH_DEFAULT_VOICE") or DEFAULT_SPEECH_VOICE


def speech_default_format() -> str:
    """Audio format asked of the upstream. A Piper server writes WAV and takes
    no transcoder, so it wants ``wav`` here; hosted providers default to mp3."""
    return _from_env_or_dotenv("SPEECH_DEFAULT_FORMAT") or DEFAULT_SPEECH_FORMAT


def is_speech_provider(provider: str) -> bool:
    return (provider or "").strip().lower() in SPEECH_PROVIDER_IDS


_SPEECH_CONFIGURED_CHECKS = {
    "speech_local": speech_local_configured,
}


def speech_provider_configured(provider: str) -> bool:
    check = _SPEECH_CONFIGURED_CHECKS.get((provider or "").strip().lower())
    return bool(check()) if check is not None else False


def speech_upstream_url(provider: str) -> str:
    """OpenAI-style speech endpoint for the given speech provider."""
    from llm_proxy.core.errors import LlmProxyError

    pid = (provider or "").strip().lower()
    if pid == "speech_local":
        base = speech_local_api_base()
        if not base:
            raise LlmProxyError(503, "speech_base_missing", "SPEECH_API_BASE is not set.")
        return f"{base}/v1/audio/speech"
    raise LlmProxyError(
        500, "invalid_speech_provider", "Invalid speech provider routing (internal)."
    )
