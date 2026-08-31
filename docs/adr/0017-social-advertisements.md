# 17. Social advertisements: a roster writes the copy, the media seam draws the picture

Date: 2026-08-30

## Status

Accepted. Builds on [ADR 0009](0009-local-media-generation.md) (the job-shaped
`media` domain) and [ADR 0011](0011-stable-diffusion-cpp-media-backend.md) (the
backend seam it will one day swap).

## Context

The ask: generate images and video for social media advertisements, for several
different things being advertised, sized correctly for where they are going,
with the post text ready to paste — and later, posted through Meta's API
directly.

Everything in that sentence except "the post text" already exists somewhere in
this repo, which is the whole reason this ADR is short. `media.rs` renders
pictures at arbitrary sizes through a two-backend seam. `project` rows already
carry per-project blobs. `teamtemplate` rows already describe a group of roles.
The gap is that nothing joins them, and nothing knows that an Instagram story
is 9:16.

Three decisions were genuinely open.

### Where do the standing facts about a company live?

The requirement was explicitly "there could be multiple projects to demonstrate,
so we need subsections or custom instruction per each project". That is a
project-scoped blob, and the `project` table already has two of them —
`workspace_payload_json` and `planning_prefs_json`.

**Decided:** a third column, `brand_json`, not a key inside an existing one.
Two writers sharing one JSON object is how a key space collides, and the two
existing blobs belong to the Flow UI and the planner respectively. A column is
one `ALTER TABLE` in each dialect.

The brief is free text in every field, capped only by total size. Validating its
*shape* would buy nothing — every field is concatenated into a prompt — and
would cost the user the one place they can say something the schema did not
anticipate ("never mention the funding round").

### What does it mean for "the agent team" to write an ad?

There was no marketing or PR team anywhere in the repo, so one had to be
defined. The harder half was what running it should *mean*.

In this codebase a team is a `teamtemplate` row whose roster is **rendered into
a prompt** — that is literally what `executor::render_team_context_for_planner`
does for the planner, and roster roles are constrained to `modality: "text"` by
`teams.rs` for exactly this reason. The alternative reading — run the DAG
executor over a marketing team and collect the ads from its tasknodes — has two
blockers: the executor's tool path is deliberately dead, so a task cannot call
`/media/generate`; and tasknode output is free text with no structured channel
out of it.

**Decided:** the roster is prompt material. One `llm::complete_internal` call,
told to write as a strategist, a copywriter, an art director and a social lead,
answering with a JSON object of variants. A campaign may name any
`teamtemplate` row instead of the built-in roster, which is how a user changes
the voice.

The DAG path is not rejected forever — it is the right shape for review and
iteration between roles. It is rejected *now*, because taking it would mean
reopening the settled tool-path decision to ship the first version of a feature
that does not need it.

### Who owns the sizes?

**Decided: the server**, served from `GET /api/v1/ads/platforms`. Studio lets a
user pick any dimensions; an ad may not, and the list has to be the one the
media seam will honour.

This is not a formality. `media::snap` clamps to 256–2048 and rounds down to a
multiple of 16, so a preset written as Instagram's nominal **1080×1080 would be
rendered at 1072×1072** and nobody would find out until a crop looked wrong.
Every preset here is 1088 or another multiple of 16, and a unit test asserts
that every entry survives `snap` unchanged — so the day someone adds a 1080
preset, it fails at the moment it is written. Platforms rescale on upload, so
1088 costs nothing; a silent rewrite would have cost trust in the sizes.

## Decision

A new `ads` domain, `desktop/crates/server/src/ads.rs`, and a second tab on the
Studio screen.

- **`project.brand_json`** — the per-project brand brief, `GET`/`PUT` at
  `/api/v1/projects/{id}/brand`, the bare object in both directions.
- **`ad_campaigns`** — one row per campaign: the project, the platform, the
  one-line brief, the team if one was named, and `copy_json` holding the
  variants. Each variant carries its caption, hashtags, call to action, picture
  prompt, and the `media_jobs.id` drawing it.
- **`PLATFORMS`** — Instagram feed (square and 4:5), story/reel, Facebook feed,
  Threads. Each with its size *and* its caption limit and hashtag count, which
  go into the prompt: a 900-character caption for Threads is one the user has to
  cut by hand, which is the work this feature exists to remove.
- **`media::start_job`** — extracted from the `generate` route handler. It was
  the only part of a generation that could not be reached from another domain.
  `generate` is now a thin wrapper; `ads` is the second caller. No seam change.
- **Desktop**: `studio.rs` grows a `Tab`, and `studio_ads.rs` /
  `studio_ads_view.rs` are the new screen. The ads view takes the **parent**
  Studio state, because the pictures are Studio's media jobs in Studio's image
  cache — one gallery, one poll, one copy of each picture in memory.

### Copy is all-or-nothing; pictures are best-effort

A model that answers prose instead of JSON fails the whole request and starts
zero jobs — half a campaign is worse than none, because the missing half is
found by counting. There is one retry, phrased bluntly; a model that ignored an
explicit "JSON only" twice will not comply on the third ask and the user is
waiting.

Once the copy exists it is **stored even if the backend refuses every picture**.
The words cost a model round-trip and are useful alone, so a variant carries a
null `media_job_id` and the reason. This asymmetry is deliberate and is the one
piece of this design that is not obvious from the outside.

### The soft reference to `media_jobs`

`copy_json` names job ids without a foreign key. `media_jobs` is capped and
pruned, and a campaign whose picture has aged out is still a campaign worth
reading — so the variant renders its words with a note where the picture was,
rather than the row vanishing.

## Consequences

- **One video preset, at a size that is measured rather than chosen.** The
  seam does video, but `length` is a frame count capped at 241 (~10s at 24fps),
  so a 90-second reel is not something local generation produces. What *is*
  producible was settled on the hardware rather than argued about — an RTX 5080
  (16 GB), Wan 2.2 TI2V 5B, 49 frames (~2s) each, through this server's own
  `text_to_video.json`:

  | size | aspect | wall time | outcome |
  |------|--------|-----------|---------|
  | 832×480 | 16:9 | 58s | ok, 1.8 GB VRAM free at peak |
  | 480×832 | 9:16 | 45s | ok, 2.4 GB free |
  | **720×1280** | **9:16** | **430s (7.2 min)** | **ok, 3.6 GB free — the preset** |
  | 1088×1920 | 9:16 | sampled 3m24s, then died | **killed the ComfyUI process** |

  The last row is the important one. 1088×1920 is the nominal story size and
  the obvious thing to reach for; it completed sampling and then took the whole
  ComfyUI process down in VAE decode. That is not an error any client can
  report — there is no failed job and no error frame, just a backend that
  stopped existing, and every media job in flight hangs until its deadline. So
  `ig_reel` is 720×1280 (Instagram's own recommended reel minimum, the largest
  size that survived) and a test asserts no video preset may exceed it.

  Seven minutes for two seconds of footage is the honest cost, and the preset's
  `note` says so. It is a moving backdrop for a caption, not a film.

- **The picture quality ceiling is the installed checkpoint, and it bites by
  default.** `media::choose_checkpoint` prefers Flux, Z-Image, SDXL and SD3 and
  then falls back to *whatever is installed*. The machine this was written on
  has exactly one checkpoint — `v1-5-pruned-emaonly.safetensors` — so every ad
  would be drawn by a model trained at 512², at sizes starting from 1088. SD
  1.5 does not compose a taller frame at that size; it repeats the subject.

  Without a word on screen the user generates mush and concludes the feature is
  broken, so `studio_ads::undersized_model` names the model, the size and the
  fix. It warns rather than blocks — the generation does work, it just looks
  wrong, and someone testing the *copy* is entitled to run it. An unrecognised
  checkpoint warns about nothing, the same "missing information is not a
  refusal" rule `MediaStatus::supports` follows.
- **The default roster is duplicated** — once in `ads.rs` as prompt material,
  once in the desktop's Library presets as an editable `teamtemplate` seed.
  They feed different consumers and neither breaks if the other drifts. The
  alternative (the server serving its default roster for the Library to fetch)
  is machinery for a constant.
- **Tenancy follows media, not projects.** Ads are master-key surface like
  `media_jobs` (ADR 0009), with `projects::assert_access` on the project so a
  campaign cannot be filed against a tenant the caller cannot see. A workspace
  token wanting this surface is the same deferred day ADR 0009's tenancy note
  names.
- **Meta publishing is a seam, not a plan.** The hard part is already visible
  and worth writing down before anyone starts: Instagram's container API needs a
  **publicly reachable `image_url`**, and a desktop-local file has none. That
  needs either the Cloud Run deployment (still free-tier only — see the root
  `CLAUDE.md`) or a temporary upload host, and it has to be designed before the
  route is real. Scheduling, when it comes, is a `workflow_engine` step and not
  new machinery.
- **Higgsfield is a third `MediaBackend` arm**, per ADR 0011's seam: probe,
  submit, poll returning bytes. Two things it will need that the seam lacks: an
  API key inside the adapter, and remote-base semantics (already precedented —
  `media_sdcpp_process` refuses to manage a non-loopback base). If it turns out
  to be MCP-only, wrap its HTTP API directly; the server has no MCP client and
  growing one for a media backend would be the wrong reason to get one.
