# 11. stable-diffusion.cpp as a second media backend

Date: 2026-08-21

## Status

Accepted. Amends [ADR 0009](0009-local-media-generation.md), which stands as
the description of the ComfyUI backend and of the job-shaped `media` domain.

## Context

The ask: stop requiring a separate ComfyUI installation, and serve our own
open-source image and video generation API.

ADR 0009 chose ComfyUI two days before this, and explicitly rejected bundling
it. Re-checking that decision against the current state of the alternatives
turned up **two facts in ADR 0009 that are wrong**, and both of them were
load-bearing.

### ADR 0009 says ComfyUI is Apache-2.0. It is GPL-3.0.

Verified at [the repository](https://github.com/comfyanonymous/ComfyUI). This
does not affect the existing integration — talking to a separate process over
loopback HTTP is aggregation, not derivation, and the Rust server is unaffected
— but it does affect the thing that was being considered: *shipping* ComfyUI
inside our installer would be conveying GPL-3.0 code, with the source-offer
obligations that carries.

### ADR 0009 says stable-diffusion.cpp is "images only; video support is
### experimental at best". It has done video for a year.

Verified at [the repository](https://github.com/leejet/stable-diffusion.cpp):
Wan 2.1 / Wan 2.2 landed September 2025, LTX-2.3 in May 2026, and MiniMax-H3
this month, alongside HunyuanVideo 1.5 and LingBot-Video. Images cover SD1/2/XL,
SD3/3.5, FLUX.1 and FLUX.2, Qwen Image, Z-Image and more. It is MIT-licensed,
ships GGUF quantisation, and — the part that matters most here — ships
**`sd-server`**, an HTTP server with a native async job API, an
OpenAI-compatible `/v1/images/generations`, and an AUTOMATIC1111 compatibility
layer.

"Only ComfyUI does both modalities on Windows today" was the sentence the whole
of ADR 0009 turned on, and it stopped being true before it was written.

### What the two backends actually cost

| | ComfyUI | `sd-server` |
|---|---|---|
| License | GPL-3.0 | MIT |
| Runtime | Python 3.13 + torch, ≈3.5 GB | one native binary: **39 MB** Vulkan, 336 MB CUDA (+563 MB cudart) |
| Modalities | image + video | image + video |
| Our contract | node-graph JSON, per-node | flat parameters |
| Model switching | per request | per process (`--diffusion-model` at startup) |
| Ecosystem | large, custom nodes | core model families only |
| Release discipline | weekly, versioned | rolling, `master-NNN-<sha>` |

The size difference survives the obvious objection. A first run downloads
gigabytes of weights either way; what changes is whether a **3.5 GB Python and
torch runtime** rides along with them, and whether we are the ones distributing
it.

## Decision

**Two backends behind one `media` domain, selected by `MEDIA_BACKEND`.**

1. **A thin seam, not an abstraction layer.** `media.rs` keeps the job row, the
   waiter, the deadline, prompt enhancement, the file route and every HTTP
   route. A backend supplies exactly three things: a probe, a submit, and a
   poll that answers `Poll::{Pending, Done{bytes, file_name}, Failed(String)}`.
   `Done` carries **bytes** rather than a URL specifically because that is
   where the two differ — ComfyUI names a file to fetch from `/view`,
   `sd-server` returns base64 in the poll body — and carrying bytes keeps the
   difference inside the adapter. There is exactly one function that writes a
   finished file.

2. **`MEDIA_BACKEND=comfy` stays the default.** sd.cpp's *video* output has not
   been compared against ComfyUI's on real hardware, and there are open
   complaints upstream about Wan quality. Flipping the default is a decision to
   make on rendered frames, not on a table. An unrecognised value logs and
   falls back rather than refusing to boot.

3. **ComfyUI is kept, not replaced.** It carries an ecosystem sd.cpp does not,
   new architectures reach it first, and a user who already runs it should not
   have to give it up. Deleting a working integration to make a point is not a
   simplification.

4. **Sampling parameters are deliberately not sent.** Sampler, scheduler, step
   count and CFG are omitted from every `sd-server` submission so it applies
   the defaults for the model it loaded. A distilled model wants `txt_cfg` 1.0
   and ~8 steps; a full one wants 3.5 and ~28; a client that pins either gets
   the other class badly wrong. We send what the user chose — prompt, size,
   seed, frame count — and nothing else.

5. **`POST /v1/images/generations` stops answering 501 on an sd.cpp install,
   with no adapter.** `sd-server` serves that exact route, OpenAI-shaped, at
   the same base. `image_api_base()` falls back to the media base when the
   backend is `sdcpp`, and the existing capability-backend registry does the
   rest. ComfyUI gets no such fallback — its API is a node graph and an OpenAI
   client pointed at it would 404. This is the "serve our own open-source image
   API" half of the ask, and it is a five-line fallback rather than a feature.

6. **`GET /status` reports `backend` and `modes`.** `sd-server` binds one model
   at startup and says which modes that model supports, so "this install cannot
   do video right now" is answerable *before* a job is submitted rather than
   minutes into one. ComfyUI reports both modes, because it loads models per
   graph. `MediaStatus::supports` treats an empty list as "yes" so an older
   server is not read as a broken one.

## Consequences

**Good.**

- Both modalities with no Python, no interpreter, no torch, and no GPL
  distribution question — on a 39 MB binary.
- No workflow templates on this path. ADR 0009 accepted "a ComfyUI update that
  renames a node breaks the template silently-ish" as a standing risk; flat
  parameters do not have that failure mode.
- The OpenAI images route becomes real for free.
- The seam is small enough that a third backend (Ollama's Windows image
  support, when it lands) is another `match` arm rather than a redesign.

**Bad, and accepted.**

- **Model coverage lags.** A new architecture reaches ComfyUI in days and
  sd.cpp in weeks-to-months. Keeping ComfyUI is the mitigation, and is why
  point 3 above is a decision and not an accident.
- **Video quality is unverified against ComfyUI.** Named here rather than
  discovered later. `--rng cpu` exists specifically to match ComfyUI's RNG,
  which makes an A/B meaningful; until it is run, the default does not move.
- **`sd-server` has no semver.** Releases are `master-827-97d2990`. Any
  lifecycle work must pin an exact tag and never track master.
- **One model per process.** No runtime model switching. On the 16 GB card this
  targets that is not a real loss — image and video weights cannot both be
  resident anyway — but it does mean the lifecycle work below is
  process-per-model, not one long-lived server.

## Lifecycle (added 2026-08-21)

`media_sdcpp_process.rs` fetches, launches and reaps `sd-server`, so the user
installs nothing. Three decisions inside it are worth naming.

7. **Model flags are configuration, not a table in this crate.** Families need
   different flags — `-m` for a full checkpoint, `--diffusion-model` plus
   `--vae` plus `--llm`/`--clip_l`/`--t5xxl` for split ones — and upstream adds
   families weekly. A lookup table here would be *our* treadmill, the exact risk
   this ADR names as sd.cpp's. `MEDIA_SDCPP_ARGS` carries them verbatim; a
   curated per-family table can fill that variable later without this module
   changing. Unset is [`Stage::Unconfigured`], a named state with an actionable
   error — never a silent download that arrives at the same error 39 MB later.

8. **A dead child ends the health wait, and its own stderr becomes the error.**
   Measured against the real binary: a wrong model path makes `sd-server` exit
   in **under a second**. A health check that only polled the port would have
   sat there for the full five-minute start timeout before reporting a typo.
   `[ERROR]` lines are preferred over the tail, because the tail is six lines of
   Vulkan device banner and quoting it puts a graphics card in front of a file
   error.

9. **Nothing is managed unless `MEDIA_API_BASE` is loopback.** A remote base
   belongs to someone else: probe it, never spawn, download or kill it.

Also: Vulkan is the default asset (39 MB, and it runs on AMD and Intel) over
CUDA (336 MB plus a separate 563 MB cudart); the release is **pinned by tag**
because sd.cpp has no semver; idle servers are stopped after
`MEDIA_SDCPP_IDLE_SECS` (default 600) because a loaded model holds the same VRAM
local chat inference wants; and a Windows job object reaps the child when
`agent-platformd` is terminated rather than shut down, the same mechanism the
desktop's `shell.rs` uses.

**One measured gotcha, recorded because it is invisible from the code.**
Unpacking names `%SystemRoot%\System32\tar.exe` by absolute path on Windows
rather than bare `tar`. Windows ships bsdtar there and bsdtar reads zip; GNU
tar, which git-bash and MSYS put on `PATH`, does not — verified on this exact
release zip, where GNU tar 1.35 answers *"This does not look like a tar
archive"*. Which one a bare `tar` resolves to is a property of the user's
`PATH`, and that is not a thing to gamble an install on. Non-Windows uses
`unzip`, the tool that is actually about this format.

**Not done in this decision, deliberately.**

- **Fetching model weights.** Still the user's job, on both backends, and still
  the gigabytes that dominate first-run either way.
- ~~A curated per-family model table.~~ **Landed 2026-08-21**: two entries, one
  per modality, behind `GET /api/v1/media/models` and
  `POST /api/v1/media/models/{id}/install`. It fills the launch arguments and
  nothing more — `MEDIA_SDCPP_ARGS` still overrides it, which is what keeps a
  stale entry costing a catalogue row rather than the feature. Every URL is
  ungated (sd.cpp's own doc links a **gated** FLUX.1-schnell VAE; the Comfy-Org
  repackage of the same file is used instead), and "installed" means
  size-matched rather than merely present.
- **Flipping the default.** Gated on the A/B above.

**Not chosen, and why.**

- **Linking stable-diffusion.cpp into `agent-platformd`** via `diffusion-rs`:
  the bindings are 0.1.x with a single maintainer, and this repo has already
  paid for two ggml-based sys crates in one binary (hundreds of `LNK2005`,
  fixed only by dynamic linking — see `desktop/CLAUDE.md`). A subprocess over
  HTTP dodges both, and is the pattern `worker/` and ComfyUI already use.
- **Porting a diffusion engine to Rust.** ComfyUI's Python is roughly a tenth
  graph engine and nine tenths vendored model implementations. Translating them
  is tractable; *numerically verifying* each one is not, because a transcription
  error in an attention mask produces slightly wrong pixels rather than a crash.
  And it is a permanent treadmill against an ecosystem that ships weekly.
- **Vendoring an embedded CPython + torch + ComfyUI.** Technically fine —
  `scripts/bundle_server.py` did exactly this shape before ADR 0007 deleted it
  — but it is 3.5 GB and a GPL conveyance to reach parity with a 39 MB MIT
  binary that already does both modalities.
- **Replacing ComfyUI outright.** See point 3.

## Tenancy

Unchanged from ADR 0009: media jobs are master-key resources with no workspace
column, and both backends write the same table.
