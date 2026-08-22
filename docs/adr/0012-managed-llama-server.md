# 12. `llama-server` as a managed local backend

Date: 2026-08-22

## Status

Accepted. Amends [ADR 0006](0006-in-process-rust-core.md), which stands for the
desktop app's own in-process engine, and generalises the lifecycle half of
[ADR 0011](0011-stable-diffusion-cpp-media-backend.md).

## Context

The question that started this: *"shouldn't the LLM server run internally like
the dedicated server, with its own logs? Same for the image/video server."*

For images, it already did. `media_sdcpp_process.rs` fetches a pinned 39 MB
`sd-server`, launches it on loopback, waits for it, drains its stderr into the
ring `GET /system/logs` serves, stops it when it goes idle and reaps it with a
job object. The user installs nothing and can read what it said.

For chat, none of that was true. Provider `local` meant llama.cpp **linked into**
whichever binary was built with `--features local-llm`:

- **Off by default**, so a shipped `agent-platformd` had no local model at all.
  In practice "run a model locally" meant Ollama or LM Studio — which the user
  installs, and which the Providers screen launches with `spawn_detached`: no
  output captured, no health wait, no reaping, and "running" inferred from an
  empty model list rather than a probe.
- **No log surface.** The in-process engine wrote nothing into the ring. Not a
  load line, not an unload line, not the reason a model failed to load.
- **Backwards ownership.** The one shipped way to reach a local GGUF from the
  server was `local_server_port`: the *daemon* calling back into the *desktop
  app's* loopback HTTP server. Close the app and every server-run agent on that
  provider dies.
- **Build cost.** `llama-cpp-sys-2` compiles C++ in our tree; a CUDA build needs
  the toolkit on the build machine and does not carry the runtime. This is the
  reason the feature is off by default, and the reason it stayed off.

ADR 0011 had already rejected *linking* stable-diffusion.cpp for a related
reason — two ggml-based sys crates in one binary cost hundreds of `LNK2005` — and
took a subprocess over HTTP instead. The same argument applies to llama.cpp, and
upstream ships the same shape of artefact: `llama-server`, OpenAI-compatible,
35 MB in the Vulkan build.

## Decision

**Provider `local` is a `llama-server` this daemon fetches, runs, logs and
stops — the same lifecycle `sd-server` already had.**

1. **The mechanism is shared, the policy is not.** `managed_server.rs` holds what
   the two have in common: a pinned GitHub release, the download to a `.part`,
   the unpack, the walk to find the executable, the spawn with stderr drained
   into the ring, the health wait that watches the child, the bounded stderr tail
   that becomes the error message, `loopback_port`, and the Windows job object.
   `media_sdcpp_process.rs` shrank by 440 lines onto it. Policy stays split
   because it genuinely differs: sd-server restarts when the *modality* changes,
   llama-server when the *model* does, and neither health check is the other's.

2. **No new configuration.** `LOCAL_MODEL_PATH` and `LOCAL_N_CTX` are the keys
   the in-process engine already read and the desktop already writes, so a user
   who had a GGUF configured keeps it: same file, same context, different
   process. They become `-m`, `-c`, `-ngl 999`, `--jinja` and `-a <file stem>`.
   `--jinja` is not optional — without it a `tools` array is ignored and an agent
   turn comes back as prose. `LOCAL_LLM_ARGS` overrides the lot for a flag this
   does not know about.

3. **`local` is an ordinary upstream now, so `ChatDest` is gone.** It resolves
   to `http://127.0.0.1:18412/v1/chat/completions` and goes down the same path
   as Ollama: streaming, retries, usage normalisation, the capability guard.
   That deleted `llm_local.rs` (236 lines), both `local-llm` branches in the
   chat handlers, and the server's `local-llm`/`cuda` cargo features. The daemon
   compiles no C++ at all.

4. **The desktop keeps its in-process engine.** ADR 0006 is not reversed:
   `local-llm` still links llama.cpp into the *app*, and a build with it still
   answers the app's own chat without a hop. What changed is that this is no
   longer the only way to run a local model, so the Settings card offers the
   GGUF picker, the Hugging Face downloader and the context box in **every**
   build — they configure the daemon — and only the VRAM and last-turn rows stay
   behind the feature.

5. **Nothing is managed unless `LOCAL_API_BASE` is loopback**, the same rule
   ADR 0011 draws for media. A remote base is someone else's llama-server: probe
   it, never spawn, download or kill it. The default is 18412, because 18410 is
   the daemon and 18411 is the app's own OpenAI surface.

6. **Pinned by tag, Vulkan by default.** `b10549`, because llama.cpp has no
   semver and cuts releases most days; the Vulkan asset is 35 MB and runs on
   AMD and Intel, where CUDA is 147–251 MB plus a separate 391 MB cudart.
   `LOCAL_LLM_VARIANT` overrides for someone who wants CUDA and will fetch its
   runtime themselves. Idle servers stop after `LOCAL_LLM_IDLE_SECS` (600),
   because a resident 9 GB model is 9 GB the image backend cannot have.

## Consequences

**Good.**

- Provider `local` works in a **default build**, on a machine with no Ollama, no
  LM Studio and no C++ toolchain.
- It says what it is doing. `[llama-server]` lines land in `GET /system/logs`
  beside `[sd-server]`, which is how a model that fails to load reports why
  rather than dying silently.
- Server-run agents on `local` no longer depend on the desktop app being open.
- The two local backends are now one pattern with two configurations, so a third
  (an embeddings server, a speech server) is a `Release` and a health check.
- The daemon's build got cheaper: no `llama-cpp-sys-2`, no CUDA toolkit, no DLLs
  to ship beside it.

**Bad, and accepted.**

- **A hop per token.** Loopback HTTP and SSE re-encoding sit between the model
  and the caller where the linked engine had neither. It is the same trade
  ADR 0011 took for images, on a link that is not the bottleneck a GPU is.
- **`llama-server` has no semver**, so the pin is a standing maintenance item —
  the same one ADR 0011 accepted for `sd-server`.
- **First use downloads.** 35 MB before the first local turn on a fresh install,
  reported as a stage rather than a stall. The gigabytes are the weights either
  way.
- **The two managed servers do not arbitrate VRAM with each other.** Each frees
  its own on its own idle timer, so generating an image while a chat model is
  resident can still spill on a 16 GB card. Marked `ponytail:` in the source;
  wire a mutual stop if it bites in practice rather than in theory.
- **Two engines can answer `local` in a `local-llm` desktop build** — the app's
  in-process one for its own chat, the managed one for everything routed at the
  server. That is the ADR 0006 split, kept deliberately, and the Settings card
  says which answered the last turn.

## The driven run (2026-08-22)

Not only unit-tested. On a Windows box with an RTX 5080, a scratch
`AGENT_PLATFORM_APP_DIR`, and no pre-existing llama.cpp install:

- **The daemon fetched and ran it.** First `local` completion downloaded
  `llama-b10549-bin-win-vulkan-x64.zip`, unpacked it, spawned, waited, and
  answered with `"model":"Qwen3-14B-Q4_K_M"` and
  `"system_fingerprint":"b10549-b2e5e9b28"` — the pinned tag, on the wire.
- **The log ring carried it.** `GET /system/logs` showed 19 `[llama-server]`
  lines: `load_model: loading model …`, `model loaded`, `listening on
  http://127.0.0.1:18412`, and the per-request `print_timing` rows. `[sd-server]`
  lines sat beside them in the same ring from the media run, through the same
  shared drain.
- **Driven from the UI, not only over curl.** E.V. with provider `local` and no
  model id: the query launched the server (1.5 s load for a 1B, 8.4 s for the
  14B), and the reply rendered in the app — including a `tool_calls` reply that
  came back through `--jinja` and surfaced as the Coder approval gate.
- **Reaping holds.** `taskkill /F` on `agent-platformd` took `llama-server` and
  `sd-server` with it, both times.
- **The media half still works after the refactor.** Same Studio screen, one
  image through ComfyUI and one through a `sd-server` that this daemon fetched,
  spawned and drove to `stage: ready` with the catalogue's z-image-turbo weights.

**And the thing the run taught.** The first 14B turn ran at **0.54 tok/s**
because another process held 15.4 of the card's 16.3 GB; b10549 fits its own
parameters to free VRAM and quietly keeps what will not fit on the CPU. Nothing
in the ring said so — the lines that would (`device_info`,
`common_params_fit_impl`) need `-lv 4`, which also emits ~200 lines of GGUF
metadata per load. Recorded as a `ponytail:` note rather than turned on. The
same run hit the proxy's 300 s upstream read timeout, which is not new: a slow
Ollama has always had that ceiling.
