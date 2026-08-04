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

1. **Provider TTS via BYOK** (lowest effort, best quality, costs per char)
   - OpenAI `gpt-4o-mini-tts` / ElevenLabs / Azure Speech.
   - Path: point `SPEECH_API_BASE` at the provider. The endpoint is built; what
     is missing is per-provider key handling — the route sends no upstream
     `Authorization`, so a hosted provider needs its key wired through the way
     the chat providers do it.

2. **Self-hosted open source** (no per-use cost, build-it-yourself appeal)
   - **Piper** — fast CPU ONNX, real-time on modest hardware, decent quality.
     Easiest self-host.
   - **Kokoro-82M** — small model, near-provider quality, ONNX ports exist.
     Current best quality/size ratio.
   - **F5-TTS / XTTS-v2** — voice cloning (give E.V. a custom voice), GPU
     preferred, slower.
   - Integration path: run one of them behind an OpenAI-shaped server and set
     `SPEECH_API_BASE` — no desktop or proxy change. Serving the model in-process
     from the sidecar is the other option, and costs a heavy Python dependency
     in the packaged payload. This is the recommended route for a self-built
     voice.

3. **Latency polish (applies to any backend)**
   - ~~Split reply into sentences, synthesize per sentence, queue chunks into
     one rodio sink~~ — **done**: `take_sentence` + `speech_queue` in
     `assistant.rs`, first audio lands while the reply is still streaming.
   - Cache greeting/ack phrases ("On it.", "Systems nominal.") as local files.

## STT — options to explore

1. Current: local whisper `base.en` push-to-talk (done).
2. **Bigger/faster models** — `small.en` for accuracy, or GPU features of
   whisper-rs (vulkan/cuda) if transcription feels slow.
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
