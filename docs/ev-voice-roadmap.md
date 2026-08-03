# E.V. voice roadmap

Status as of 2026-08-03. E.V. lives in `desktop/crates/app/src/assistant.rs` /
`assistant_view.rs`.

## Current state

- **TTS (speaking)**: Microsoft Edge neural TTS (`en-US-AriaNeural`) over the
  free websocket endpoint (`msedge-tts` crate), played via `rodio`. Unofficial
  endpoint — may break without notice. Automatic fallback: native platform
  engine (`tts` crate → SAPI/WinRT on Windows), which is the robotic voice.
- **STT (listening)**: Windows built-in speech recognition (WinRT
  `SpeechRecognizer`) — push-to-talk button, listens until silence,
  auto-sends. Windows-only; requires "Online speech recognition" enabled in
  Windows privacy settings (or an offline language pack). No streaming
  partials.

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

1. Current: WinRT one-shot dictation (done).
2. **whisper-rs** (whisper.cpp bindings) — local, cross-platform, `base.en`
   model ~142 MB, good accuracy. Needs cmake toolchain + mic capture via
   `cpal`. Removes the Windows-only + online-speech-setting constraints.
3. **Streaming partials** — show words as they're recognized (WinRT
   `ContinuousRecognitionSession` or whisper.cpp streaming mode).
4. **Wake word** ("Hey E.V.") — openWakeWord / tiny keyword-spotting model;
   only after push-to-talk feels solid.

## North star

Full-duplex conversation: wake word → streaming STT → LLM stream → sentence-
chunked TTS, interruptible mid-reply (barge-in stops playback, starts
listening). Every piece above is a step toward this.
