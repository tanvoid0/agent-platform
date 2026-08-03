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

## TTS — options to explore (in rough order of effort)

1. **Provider TTS via BYOK** (lowest effort, best quality, costs per char)
   - OpenAI `gpt-4o-mini-tts` / ElevenLabs / Azure Speech.
   - Path: add `/v1/audio/speech` proxy endpoint to the FastAPI server (mirrors
     the existing chat proxy + provider-key model), desktop client hits it the
     same way it hits `/chat`. Keys already managed by the Providers screen.

2. **Self-hosted open source** (no per-use cost, build-it-yourself appeal)
   - **Piper** — fast CPU ONNX, real-time on modest hardware, decent quality.
     Easiest self-host.
   - **Kokoro-82M** — small model, near-provider quality, ONNX ports exist.
     Current best quality/size ratio.
   - **F5-TTS / XTTS-v2** — voice cloning (give E.V. a custom voice), GPU
     preferred, slower.
   - Integration path: the Python sidecar already exists — serve the model from
     a `/v1/audio/speech` endpoint in the same FastAPI app. Desktop code needs
     zero changes beyond pointing `synthesize()` at it. This is the recommended
     route for a self-built voice.

3. **Latency polish (applies to any backend)**
   - Split reply into sentences, synthesize per sentence, queue chunks into one
     rodio sink → first audio in ~0.5s instead of waiting for the full reply.
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
