# 9. Local image and video generation: ComfyUI as the media backend

Date: 2026-08-19

## Status

Accepted, and amended by [ADR 0011](0011-stable-diffusion-cpp-media-backend.md),
which adds stable-diffusion.cpp as a second backend behind the same domain. The
`media` domain, the job shape and the ComfyUI integration described here are
unchanged and still the default.

**Two factual corrections**, both found while re-checking this decision and both
marked inline below: ComfyUI is **GPL-3.0**, not Apache-2.0, and
stable-diffusion.cpp has supported video since September 2025 — the survey below
had it as images-only. The second was the sentence this ADR's choice rested on.

## Context

The ask: the AI chat should be able to produce **images and videos from natural
language**, locally, with local tooling — "I don't expect it to be as accurate
as Veo, but an aid to help open pathways to local image generation." A dedicated
page is acceptable if it ships faster than folding it into chat.

### What this machine (and a typical install) can actually run

- **GPU here: RTX 5080, 16 GB VRAM.** That fits Flux.2 Klein / Z-Image-Turbo /
  SDXL for images, and quantised Wan 2.2 or LTX-2 FP8 for short video clips.
  Video generation on consumer hardware is minutes per clip, not seconds — the
  feature has to be shaped as a job you start and come back to, not a chat turn.

### The backends that exist, checked rather than assumed

- **Ollama** grew *experimental* image generation in January 2026
  (`x/flux2-klein`, `x/z-image-turbo`) — but **macOS only**. Verified on this
  machine (Windows, ollama 0.32.14, `x/flux2-klein` already pulled):
  `/api/generate` answers `"image generation models are not currently
  supported"`, and there is no `/v1/images/generations` route. Windows support
  is announced as coming, not shipped. No video, and none announced.
- **LM Studio** serves LLMs and embeddings. No image or video generation.
- **ComfyUI** is the de facto standard for local diffusion: images (Flux, SDXL,
  Z-Image) *and* video (Wan 2.2, LTX-2, AnimateDiff) through one HTTP API on
  `127.0.0.1:8188` — `POST /prompt` takes a workflow graph in API-JSON form,
  `GET /history/{id}` reports completion, `GET /view` hands back the output
  file. It ships native workflow templates for exactly the text-to-image and
  text-to-video cases this feature needs. Installable as a desktop
  app or a portable zip, actively maintained. **Correction (ADR 0011):
  GPL-3.0, not Apache-2.0** — which does not affect talking to it over
  loopback, but does affect ever shipping it.
- **stable-diffusion.cpp** is the sd analogue of llama.cpp: a single binary,
  GGUF weights, spawnable as a subprocess exactly like `worker/`. Images only;
  video support is experimental at best. **Correction (ADR 0011): wrong when
  written.** Wan 2.1/2.2 landed September 2025 and LTX-2.3 in May 2026, and it
  ships `sd-server` with an async job API. This is the sentence the decision
  below turned on.
- **AUTOMATIC1111 WebUI** has the simplest REST API (`/sdapi/v1/txt2img`) but is
  in maintenance mode and does images only.

Only one of these does both modalities on Windows today, and it is ComfyUI.
**Correction (ADR 0011): two do.**

### What the server already has

`llm.rs` already registers **`POST /v1/images/generations`** (OpenAI-shaped),
routed to a capability backend registry (`image_local`, `IMAGE_API_BASE`) — a
501 until something answers there. Nothing in the desktop calls it, ComfyUI is
not OpenAI-shaped, and the OpenAI images contract has no notion of video or of
a job you poll. So that route is a *second* door for OpenAI-compatible image
backends (and the obvious place Ollama-on-Windows plugs in when it lands), not
the foundation for this feature.

The desktop renders no images anywhere but its own logo, and `GET /file`
returns extracted *text* — there is no raw-binary route. Both gaps are part of
this work.

## Decision

**ComfyUI is the media generation backend, treated exactly like Ollama is for
chat: an external local app the user installs, that the server talks to over
loopback HTTP.** We do not bundle it, spawn it, or manage its models.

1. **A `media` domain on the server** (`media.rs`):
   - `POST /api/v1/media/generate` — `{kind: "image"|"video", prompt, width,
     height, ...}`. The server owns two **checked-in ComfyUI workflow templates**
     (text-to-image, text-to-video), fills in the prompt, dimensions and a seed,
     and submits to ComfyUI's `/prompt`. The model reference in each template is
     resolved against what ComfyUI reports installed (`/object_info`), so the
     template does not hard-code a checkpoint filename the user does not have.
   - Generation is a **job, not a request**: a `media_jobs` table (new
     migration), a poller against `/history/{id}`, and the finished file copied
     into the app's data dir under `media/`. Diffusion takes seconds to minutes;
     an HTTP response that holds open for that long is the wrong shape, and jobs
     survive an app restart where an in-flight response does not.
   - `GET /api/v1/media/jobs`, `GET /api/v1/media/jobs/{id}`, and
     `GET /api/v1/media/jobs/{id}/file` — the last is the server's **first
     raw-binary route**, because the desktop has to render the result and the
     existing `GET /file` is a text extractor.
2. **The prompt is used verbatim; a model only elaborates it on request.** An
   "enhance" flag runs the natural-language ask through the in-process `/v1`
   (same pattern as `search.rs`): expand "a cat in the rain" into a proper
   diffusion prompt. Any model failure — no master key, bad output, timeout —
   degrades to the user's own words rather than erroring. The deterministic
   path is the default; the model is an upgrade, never a dependency.
3. **A `Studio` screen** (`studio.rs`/`studio_view.rs`), not a chat extension:
   prompt box, image/video toggle, size presets, and a job gallery. Images
   render in-app via iced's image widget fed by the file route. **Video does
   not play in-app** — iced has no video decoder, so a finished video gets a
   thumbnail-less card with *Open* (default player) and *Reveal in folder*.
   Honest ceiling, noted in code; an in-app player is a rabbit hole this
   feature does not need.
4. **Unconfigured is a first-class state, never an error** — the lesson ADR
   0008 encodes. No ComfyUI on `127.0.0.1:8188` means the screen still renders:
   it says what is missing, links the install, and the probe result appears on
   the Providers screen like Ollama's does. Never a 503, never a bare spinner.
5. **E.V. gets this for free.** `POST /api/v1/media/generate` is a write, and
   `assistant_tools::api_write` already parks writes behind the one confirm
   card. "E.V., make me a picture of X" is the existing tool calling the new
   route — zero new tools, and the card shows exactly what will be generated.
   Job status reads come free through `api_get`.

## Consequences

**Good.**

- Both modalities, one integration, working on Windows today. The workflow
  templates are data, not code — supporting a new model family is editing JSON,
  not writing Rust.
- Nothing bundled: no Python environment shipped, no model weights distributed,
  no GPU contention managed. ComfyUI owns its models the way Ollama owns its —a
  boundary this repo already knows how to live with.
- The job shape means the app stays usable during a five-minute video render,
  and a finished job is on disk, not in a response nobody kept.
- The server's only new outbound surface is loopback HTTP to a port the user
  configured — same trust story as `OLLAMA_API_BASE`.

**Bad, and accepted.**

- **The user must install ComfyUI and download models themselves.** That is
  real friction — gigabytes of weights, a separate app to keep updated. It is
  also exactly the Ollama deal, which this user base has already taken.
- **Workflow templates are a contract with ComfyUI's node graph.** A ComfyUI
  update that renames a node breaks the template silently-ish (the job fails
  with ComfyUI's error, which we surface verbatim). Pinned templates against
  core nodes only — no custom node packs — keeps this small.
- Quality is "local diffusion" quality. The ask priced this in.
- Video preview is out-of-app. Ceiling noted where it is cut.

**Not chosen, and why.**

- **Bundling ComfyUI or sd.cpp as a subprocess** (the `worker/` pattern): sd.cpp
  has no video, so it cannot cover the ask alone; bundling ComfyUI means
  shipping a Python environment the installer just finished deleting (ADR
  0005/0007) and owning GPU/driver breakage for every user. The subprocess
  pattern is right for the LoRA pipeline because we *wrote* that pipeline; it
  is wrong for a third-party app with its own updater and ecosystem.
- **Waiting for Ollama's Windows image support**: no date, no video, and the
  user asked now. When it lands it slots in as an *additional* image backend
  behind the existing `/v1/images/generations` registry — this decision does
  not block that one.
- **Cloud generation APIs** (OpenAI Images, Replicate, fal): the ask was local.
  BYOK cloud image generation already has a door (`/v1/images/generations` with
  BYOK headers) and stays where it is.
- **Extending the OpenAI images route instead of a media domain**: that
  contract is synchronous, image-only, and base64-in-JSON. Forcing video jobs
  through it means inventing non-standard fields until it is OpenAI-shaped in
  name only. A small honest domain beats a large dishonest compatibility layer.
- **Images inline in the E.V. transcript, this round**: chat has no attachment
  plumbing at all (checked — the only image in the app is the logo). Studio is
  the surface; E.V. triggers jobs and can open the screen (`open_screen` tool
  already exists). Inline results are the obvious second slice once the file
  route exists, and are deferred, not declined.

## Tenancy

Media jobs are master-key resources like the rest of the desktop's own surface
— no workspace column. The desktop is the only UI; a workspace-scoped gallery
is a decision for the day a second client wants one, and adding a column then
is a forward-only migration like any other.
