# Speech service

Standalone text-to-speech service (Piper) exposing the OpenAI
`/v1/audio/speech` shape. The agent-platform proxy routes the `speech`
capability here when `SPEECH_API_BASE` points at it, and the desktop app's E.V.
voice is the caller.

Kept out of the main app on purpose, like the image service: the platform
brokers capabilities over HTTP and never imports a model runtime. Nothing here
is a dependency of `app/`.

## Why Piper

CPU-only ONNX, real-time on modest hardware, ~60 MB per voice, permissive
licence, no per-use cost. Swapping it for Kokoro-82M (better quality, heavier
setup) later means pointing `SPEECH_API_BASE` at that server instead — the
platform and the desktop do not change.

## Setup

```bash
cd services/speech-service
python -m venv .venv && . .venv/Scripts/activate   # Linux: . .venv/bin/activate
pip install -r requirements.txt
```

Then install Piper itself, either way:

```bash
pip install piper-tts          # importable module, plus a `piper` console script
```

or download a release binary from `github.com/rhasspy/piper/releases` (there are
no wheels for every Python/OS combination — the binary is the fallback) and
point `PIPER_BIN` at it.

Prefer the pip install. When `piper` is importable the voice is loaded once and
stays loaded (`engine: in-process` in `/health`); the binary path shells out per
request, and loading the model costs ~1.4 s against ~50 ms to synthesize a
sentence. Over a streamed reply that is the difference between the voice
tracking the text and falling seconds behind it.

Voices are separate downloads: `huggingface.co/rhasspy/piper-voices`. Each is a
`<name>.onnx` plus a `<name>.onnx.json` alongside it. Drop both in `voices/`:

```
services/speech-service/voices/en_US-amy-medium.onnx
services/speech-service/voices/en_US-amy-medium.onnx.json
```

## Run

```bash
uvicorn app:app --host 127.0.0.1 --port 8123
```

```bash
curl http://127.0.0.1:8123/health
# {"status":"ok","engine":"in-process","voices":["en_US-amy-medium"], ...}
```

`status` is `unconfigured` until both an engine and one voice are found.

## Environment

| var | default | notes |
|-----|---------|-------|
| `PIPER_BIN` | `piper` | binary name on PATH, or a full path |
| `PIPER_VOICES_DIR` | `voices` | where the `.onnx` files live |
| `PIPER_VOICE` | `en_US-amy-medium` | used when the request names none |

## Wire into agent-platform

In the platform's `.env`:

```
SPEECH_API_BASE=http://127.0.0.1:8123
SPEECH_DEFAULT_FORMAT=wav
SPEECH_DEFAULT_VOICE=en_US-amy-medium
```

All three matter. No `SPEECH_API_KEY` — this service takes no auth, so bind it
to loopback. `SPEECH_DEFAULT_FORMAT=wav` because Piper writes WAV and
transcoding to MP3 would mean an ffmpeg dependency for nothing; the desktop
decodes either. `SPEECH_DEFAULT_VOICE` because the proxy's own default is
OpenAI's `alloy`, which Piper does not have — leave it unset and every request
404s with the voices you do have listed.

Then the capability router lights up:

```bash
curl -s localhost:18410/v1/capabilities -H "Authorization: Bearer $KEY" | jq .resolved.speech
# "speech_local"

curl -s localhost:18410/v1/audio/speech -H "Authorization: Bearer $KEY" \
  -H 'content-type: application/json' \
  -d '{"input":"Systems nominal."}' --output speech.wav
```

E.V. picks it up with no restart: the desktop tries the server for every
sentence and falls back to Edge neural TTS, then the native engine.

## Request body

| field | default | notes |
|-------|---------|-------|
| `input` | required | the text to speak |
| `voice` | `PIPER_VOICE` | a `.onnx` stem in the voices dir |
| `model` | ignored | accepted so an OpenAI-shaped caller does not 422 |
| `response_format` | ignored | always WAV; see above |
