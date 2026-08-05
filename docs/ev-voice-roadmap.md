# E.V. voice roadmap

Status as of 2026-08-03. E.V. lives in `desktop/crates/app/src/assistant.rs` /
`assistant_view.rs`.

## Current state

- **TTS (speaking)**: Microsoft Edge neural TTS (`en-US-AriaNeural`) over the
  free websocket endpoint (`msedge-tts` crate), played via `rodio`. Unofficial
  endpoint — may break without notice. Automatic fallback: native platform
  engine (`tts` crate → SAPI/WinRT on Windows), which is the robotic voice.
- **STT (listening)**: local whisper.cpp (`whisper-rs` + `cpal` mic capture,
  `desktop/crates/app/src/stt.rs`). Push-to-talk toggle: 🎤 starts recording,
  ⏹ stops → transcribes → auto-sends. Quantized `base.en` model (~60 MB)
  auto-downloads to the app data dir on first use; fully offline after that.
  Cross-platform. Build needs cmake + libclang (see `desktop/.cargo/config.toml`).
  (WinRT `SpeechRecognizer` was tried first and abandoned: its online backend
  no longer transcribes on current Windows 11 builds.)

## The server route — done 2026-08-04

`POST /v1/audio/speech` exists on the FastAPI app and is what the desktop tries
first for every sentence; Edge neural, then the native engine, remain the
fallbacks (in that order, per sentence, so a backend that dies mid-reply does
not silence the rest of it).

It is a thin proxy to whatever answers at `SPEECH_API_BASE` in the OpenAI speech
shape — a hosted provider, or a local Piper/Kokoro server, the choice below is
now a deployment question rather than a code one. Registry and capability
routing mirror image generation: `llm_proxy/services/speech_backends.py`, the
`speech` modality in `core/capabilities.py`, `speech_local` in
`/v1/capabilities`. Unconfigured answers a structured 501, which the desktop
reads as "use your own engine".

Defaults are `SPEECH_DEFAULT_MODEL=tts-1` and `SPEECH_DEFAULT_VOICE=alloy`
(see `.env.example`); the desktop sends only `input` and takes them.

## TTS — options to explore (in rough order of effort)

1. **Provider TTS** (lowest effort, best quality, costs per char) — **wired**
   - Point `SPEECH_API_BASE` at the provider and set `SPEECH_API_KEY`; it is
     sent as `Authorization: Bearer`. One key for whatever the base URL is,
     not a key per named provider — the route already requires an OpenAI-shaped
     upstream, and every hosted one of those takes a bearer token.
   - Exercised against a stub upstream only — no hosted provider has been
     called with a real key yet. A backend with its own auth scheme
     (ElevenLabs' `xi-api-key`) needs a gateway in front, or a row of its own
     in `speech_backends.py`.

2. **Self-hosted open source** (no per-use cost, build-it-yourself appeal) — **built**
   - **Piper** — fast CPU ONNX, real-time on modest hardware, decent quality.
     Easiest self-host. **This is what was built:** `services/speech-service/`
     exposes the OpenAI `/v1/audio/speech` shape over Piper; point
     `SPEECH_API_BASE` at it and the capability router resolves `speech_local`.
     Setup, voice downloads and the three env vars that all matter are in
     [its README](../services/speech-service/README.md) — that file is the
     authority, not this one. Kept out of `app/` on purpose: the platform brokers
     capabilities over HTTP and never imports a model runtime.
   - **Kokoro-82M** — small model, near-provider quality, ONNX ports exist.
     Current best quality/size ratio. Swapping to it means pointing
     `SPEECH_API_BASE` at a different server; platform and desktop do not change.
   - **F5-TTS / XTTS-v2** — voice cloning (give E.V. a custom voice), GPU
     preferred, slower.
   - The rejected alternative: serving the model in-process from the sidecar,
     which costs a heavy Python dependency in the packaged payload.

3. **Latency polish (applies to any backend)**
   - ~~Split reply into sentences, synthesize per sentence, queue chunks into
     one rodio sink~~ — **done**: `take_sentence` + `speech_queue` in
     `assistant.rs`, first audio lands while the reply is still streaming.
   - Cache greeting/ack phrases ("On it.", "Systems nominal.") as local files.

## STT — options to explore

1. Current: local whisper `base.en` push-to-talk (done).
2. **Bigger/faster models** — `small.en` for accuracy, or GPU features of
   whisper-rs (vulkan/cuda) if transcription feels slow. Note whisper-rs is
   pulled on default (CPU) features today, so those flags are untested here —
   the same unknown [ADR 0006](adr/0006-in-process-rust-core.md) puts in front of
   its llama.cpp spike. Whichever gets built first answers it for both.
3. **Auto-stop (VAD)** — detect end-of-speech instead of a second click
   (whisper.cpp ships a VAD; energy-threshold is the cheap version).
4. **Streaming partials** — show words as they're recognized (whisper.cpp
   streaming mode).
5. **Wake word** ("Hey E.V.") — openWakeWord / tiny keyword-spotting model;
   only after push-to-talk feels solid.

## North star

Full-duplex conversation: wake word → streaming STT → LLM stream → sentence-
chunked TTS, interruptible mid-reply (barge-in stops playback, starts
listening). Every piece above is a step toward this.
