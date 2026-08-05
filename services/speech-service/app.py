"""Standalone text-to-speech service (Piper) in the OpenAI `/v1/audio/speech` shape.

The agent-platform proxy routes the `speech` capability here when
`SPEECH_API_BASE` points at it; the desktop app's E.V. voice is the caller.

Kept out of the main app for the same reason as the image service: the platform
brokers capabilities over HTTP and never imports a model runtime.

Piper runs in-process when `piper-tts` is importable, and through its CLI
otherwise, so a pip install and a downloaded release binary both work. The
in-process path exists for latency: loading a voice costs ~1.4 s against ~50 ms
to synthesize a sentence, so a process per request meant every sentence of a
streamed reply paid the load again and the voice never caught up with the text.
"""

from __future__ import annotations

import io
import os
import shutil
import subprocess
import tempfile
import threading
import wave
from contextlib import asynccontextmanager, suppress
from functools import lru_cache
from pathlib import Path

from fastapi import FastAPI, HTTPException
from fastapi.responses import Response
from pydantic import BaseModel


@asynccontextmanager
async def lifespan(app: FastAPI):
    # Load the default voice before the first request rather than during it —
    # that request is the one a listener notices as a gap.
    with suppress(Exception):
        load_voice(str(voice_path(DEFAULT_VOICE)))
    yield


app = FastAPI(title="Speech service", version="0.1.0", lifespan=lifespan)

PIPER_BIN = os.environ.get("PIPER_BIN", "piper")
VOICES_DIR = Path(os.environ.get("PIPER_VOICES_DIR", "voices"))
DEFAULT_VOICE = os.environ.get("PIPER_VOICE", "en_US-amy-medium")
# Piper writes WAV. Transcoding would mean an ffmpeg dependency for no gain —
# the desktop decodes whatever the content type says.
MEDIA_TYPE = "audio/wav"


class SpeechRequest(BaseModel):
    input: str
    # Accepted and mostly ignored: the OpenAI shape carries them, Piper picks a
    # voice by file. `model` exists so a caller's default does not 422.
    model: str | None = None
    voice: str | None = None
    response_format: str | None = None
    speed: float | None = None


def voice_path(name: str) -> Path:
    """Resolve a voice name to its .onnx, rejecting anything outside the dir."""
    candidate = (VOICES_DIR / f"{name}.onnx").resolve()
    if not str(candidate).startswith(str(VOICES_DIR.resolve())):
        raise HTTPException(status_code=400, detail="invalid voice")
    if not candidate.exists():
        available = sorted(p.stem for p in VOICES_DIR.glob("*.onnx"))
        raise HTTPException(
            status_code=404,
            detail=f"voice {name!r} not installed; available: {available or 'none'}",
        )
    return candidate


@lru_cache(maxsize=4)
def load_voice(path: str):
    """The loaded voice for a `.onnx`, kept for the life of the process.

    Raises `ImportError` when only the release binary is installed; the caller
    falls back to the CLI.
    """
    from piper import PiperVoice

    return PiperVoice.load(path)


# ponytail: one global lock. A loaded voice is not documented as thread-safe and
# FastAPI runs sync handlers on a threadpool, so concurrent callers serialize.
# Synthesis is ~50 ms; go per-voice, or to multiple workers, if that queues.
_synth_lock = threading.Lock()


def synthesize(text: str, model: Path) -> bytes | None:
    """WAV bytes from the in-process voice, or `None` when `piper-tts` is not
    importable — a release-binary install has the CLI and nothing else."""
    try:
        voice = load_voice(str(model))
    except ImportError:
        return None
    buf = io.BytesIO()
    with _synth_lock, wave.open(buf, "wb") as wav:
        voice.synthesize_wav(text, wav)
    return buf.getvalue()


@app.get("/health")
def health() -> dict[str, object]:
    """Readiness: something can synthesize, and at least one voice is installed.

    `engine` is worth reading when the voice sounds slow: `cli` reloads the
    model on every request, `in-process` does not.
    """
    voices = sorted(p.stem for p in VOICES_DIR.glob("*.onnx")) if VOICES_DIR.exists() else []
    binary = shutil.which(PIPER_BIN) or (PIPER_BIN if Path(PIPER_BIN).exists() else None)
    engine = "cli" if binary else None
    with suppress(ImportError):
        import piper  # noqa: F401

        engine = "in-process"
    return {
        "status": "ok" if (engine and voices) else "unconfigured",
        "engine": engine,
        "piper": binary,
        "voices": voices,
        "default_voice": DEFAULT_VOICE,
    }


@app.post("/v1/audio/speech")
def speech(req: SpeechRequest) -> Response:
    text = (req.input or "").strip()
    if not text:
        raise HTTPException(status_code=400, detail="input is required")
    model = voice_path((req.voice or DEFAULT_VOICE).strip() or DEFAULT_VOICE)

    if (audio := synthesize(text, model)) is not None:
        return Response(content=audio, media_type=MEDIA_TYPE)

    # Release-binary install: a process per request, model load included. Piper
    # streams raw PCM to stdout but only writes a real WAV header to a file, so
    # it gets a temp file and we hand back the bytes.
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp) / "speech.wav"
        try:
            proc = subprocess.run(
                [PIPER_BIN, "--model", str(model), "--output_file", str(out)],
                input=text.encode("utf-8"),
                capture_output=True,
                timeout=120,
            )
        except FileNotFoundError as e:
            raise HTTPException(status_code=503, detail=f"piper not found: {PIPER_BIN}") from e
        except subprocess.TimeoutExpired as e:
            raise HTTPException(status_code=504, detail="piper timed out") from e
        if proc.returncode != 0 or not out.exists():
            detail = proc.stderr.decode("utf-8", "replace").strip()[-500:]
            raise HTTPException(status_code=500, detail=f"piper failed: {detail}")
        audio = out.read_bytes()

    return Response(content=audio, media_type=MEDIA_TYPE)
