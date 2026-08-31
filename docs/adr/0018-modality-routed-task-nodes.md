# 18. A task node has a modality, and the capability router picks the backend

Date: 2026-08-30

## Status

Accepted. Extends [ADR 0009](0009-local-media-generation.md) (the job-shaped
`media` domain), [ADR 0011](0011-stable-diffusion-cpp-media-backend.md) (its two
backends) and [ADR 0017](0017-social-advertisements.md) (the first caller that
wanted a picture from a team).

## Context

A cloud model made this problem invisible: one provider, one key, and a model id
per kind of output. Locally it is three programs — Ollama or llama.cpp for text,
ComfyUI or sd-server for pictures — and the question "which one do I use for
this step" landed on the user, in a settings page that was organised by vendor
rather than by what the user was trying to make.

The pieces were already here and not joined:

- `llm_config` has a real capability router — `Modality`,
  `resolve_provider_for_capability`, local backends preferred — but the media
  backend was not a provider in it. The only image entry was `image_local`, an
  OpenAI-shaped `/v1/images/generations` upstream that a desktop does not run.
- `media.rs` owns ComfyUI and sd-server: one job row, one waiter, one file
  route, two adapters. Nothing outside `ads.rs` could ask it for anything.
- A task node could only produce text. `teams.rs` rejected any roster role whose
  modality was not `text`, with the note "until the server resolves audio,
  video, and image routing" — this ADR is that resolution.

So a process could plan "design the hero image", and the only thing it could do
with that node was write a paragraph describing one.

## Decision

**The media backend is a capability provider.** `media_local`
(`MEDIA_API_BASE`, `[ImageGeneration, VideoGeneration]`, `sort_order: 0`) sits
in the same `PROVIDERS` table as the chat backends, so
`resolve_provider_for_capability` answers for pictures the way it already
answered for chat. `VideoGeneration` is a new modality; there was none, because
nothing could route video. "Configured" means the same thing it means for Ollama
— we know a URL to try — because reachability is an async probe and this
registry is read from sync code.

**`/v1/images/generations` pins `image_local` by name.** The media backend
outranks it and does not speak that route's OpenAI shape; asking the router
there would send an OpenAI body at ComfyUI's graph API. That proxy keeps its own
upstream, and the media backend's door stays `POST /api/v1/media/jobs`.

**A task node carries a modality.** `text` (the default, and every row that
existed before this) goes to the chat proxy unchanged; `image` and `video` go to
`media::generate_and_wait`, which starts the same job Studio and the ad
campaigns start and polls the row until it lands. The node's user message —
instructions plus dependency context, exactly what a text node would have been
given — is the prompt, because that is what the planner wrote the node to say.

**Its output stays text.** `Generated image (media job 12): /api/v1/media/jobs/12/file`.
Every consumer of a node output is a text consumer: the review screen, the
transcript, the context handed to a dependent node. `tasknode.media_job_id`
carries the machine-readable half so nothing has to parse that line.

**Failure is `LlmFailure::Llm`.** A backend that is not running, has no
checkpoint, or refused the graph is the same class of problem as a provider that
will not answer, and the node should be retryable for it.

**The per-modality default for images is `MEDIA_IMAGE_MODEL`** — the checkpoint
the backend renders with, `DEFAULT_MODEL`'s twin. There is no video twin: the
video template names its own model family, so there is nothing to choose yet.
A name that is no longer installed falls back to the family heuristic rather
than failing the job.

## Consequences

- A planner may now emit an `image` / `video` node, and the prompt documents
  when to. A run whose goal asks for a picture produces one, with nobody naming
  a provider anywhere — which is the point.
- `process.dag_json` gained a field. It is additive and defaulted, so an older
  DAG still validates and an older planner still produces valid ones; the test
  that pins the canonical dump was updated deliberately.
- `tasknode` gained two columns, forward-only and defaulted, so a process in
  flight survives the upgrade.
- A media node blocks its wave for as long as a render takes. That is inherent —
  the nodes downstream of it are waiting on the picture — but it is why
  `generate_and_wait` carries its own deadline as well as trusting the watcher:
  a restart between submit and completion leaves no watcher, and a node blocked
  forever would hold the whole wave open.
- Audio stays shut. `Modality::Speech` is in the router and has no node path, so
  a roster role asking for it is still rejected.
- The job is filed with `user_id = NULL`. `media_jobs` is master-key surface
  (ADR 0009, "Tenancy") and a process carries no user of its own — ownership
  hangs off its workspace.

## Alternatives rejected

**Route on the model alias.** A node whose `model` names an image model becomes
an image node. No migration, no schema change — and no way to tell a typo from a
checkpoint, no way to ask for a picture without knowing what is installed, and a
planner that has to guess model names to express intent.

**A separate node type.** `tasknode` is one table with one executor and one
review path; a second kind of node would have needed all three again for a
difference that is one dispatch.

**Translate ComfyUI behind `/v1/images/generations`.** Then a media node would
be an ordinary LLM call. It also means writing an OpenAI-shaped adapter over a
graph API that returns files asynchronously, and throwing away the job row, the
gallery and the file route that already exist.
