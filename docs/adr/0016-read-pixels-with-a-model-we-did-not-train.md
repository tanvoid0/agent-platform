# ADR 0016: Read pixels with a model we did not train

**Status:** Proposed
**Date:** 2026-08-27
**Deciders:** (fill when decided)
**Tags:** model-ops, LoRA, vision, VLM, job pipeline, capture, scope

---

## 1. Context

[ADR 0015](0015-job-pipeline-task-model.md) commits to training small task
models for the job pipeline from its own decision log. The first, the screener,
is trained and measured.

Two jobs in this system genuinely start from pixels, which is what prompted the
question of whether model-ops should learn to train on images:

- **Captured website screenshots.** The browser extension takes them today,
  `chrome.tabs.captureVisibleTab(windowId, { format: 'jpeg', quality: 60 })`,
  and captures reach the portfolio at `POST /api/admin/ingest`. Not every advert
  is a board API; plenty are rendered pages the extension is already standing in
  front of.
- **CV data.** Extracting structured fields out of a CV that arrived as a file
  rather than as a record.

What exists in model-ops today is text LoRA training and nothing else. There is
no diffusion, DreamBooth or textual-inversion path in `worker/model_ops/`. The
only mention of vision in the pipeline is `_MULTIMODAL_BLOCKLIST =
("vision_tower", "audio_tower")` in `pipeline/lora_targets.py`, which exists to
*exclude* vision layers when a multimodal base is adapted for a text task. Image
work in this platform is generation and lives elsewhere,
[ADR 0009](0009-local-media-generation.md) and
[ADR 0011](0011-stable-diffusion-cpp-media-backend.md).

## 2. Decision

**Both jobs are real. Neither is a reason to train an image model.** Use a
vision language model we did not train, and reach for it only where the cheaper
source has actually failed.

### Why not train one

Training needs labelled pairs, and the thing this pipeline is short of is
labels. The screening corpus has 23 real adverts and no labels on any of them;
that shortage is the binding constraint on the *text* model already. A
screenshot-to-fields model needs the same labels plus a rendering of every
example, and an off-the-shelf VLM does the job zero-shot today. Training earns
its place when a small model replaces an expensive call on a task done tens of
thousands of times, which is the argument in ADR 0015 for the screener. Neither
of these tasks is that yet.

### Screenshots: the DOM first, and know what a capture is not

The extension holds a live DOM whenever it can take a screenshot. Field names,
link targets, headings and the full text are all there exactly, where a JPEG at
quality 60 has them approximately. Two properties of the capture decide this:

- **It is the visible tab.** A real advert measures 7.5k to 16k characters,
  median 9.8k. That does not fit in a viewport, so one capture structurally
  cannot contain the advert. Reading adverts from screenshots means stitching
  scrolled captures, which is work to recover something the DOM already had
  whole.
- **It is lossy on purpose.** Quality 60 is chosen for transport, not for
  reading small text.

So a VLM over screenshots is the fallback for pages where the DOM does not
answer: canvas rendered content, an advert embedded as an image, a PDF job spec.
That is a narrow and real set, and it is a fallback rather than the path.

### CV data: extract text before looking at pixels

A CV that is a text PDF or a DOCX gives its content up to a parser with no model
involved, and most CVs are one of those. A VLM earns its place on the scanned or
image-only ones, where there is no text layer to extract. Ordering matters here
because the model is the expensive, least predictable option and it is being
proposed for the case a library already handles.

Note this cuts against the pipeline's own documents: the CVs and letters this
system sends are generated from a record it owns, so it never needs to read one
of those back. The case is CVs arriving from outside.

### Form filling stays on the DOM

Raised and refused separately. The extension fills forms from named fields, and
that traceability is what makes it safe to point at somebody's live browser
session. A model reading a screenshot would re-derive coordinates for what the
page hands over exactly, and would trade an auditable mapping for a guess.

## 3. Consequences

- Model-ops keeps its install footprint, GPU budget and eval story. Adding image
  training changes all three at once.
- `_MULTIMODAL_BLOCKLIST` stays and stays load bearing: a multimodal base is
  still a fine choice for a text task, and the blocklist keeps its vision tower
  out of the adapter.
- A VLM call is a new external dependency in a path that currently runs local.
  Whichever model is chosen, the capture reaching it is a page from the owner's
  own logged-in browser, so it is subject to the same masking rule as the
  training export: nothing carrying a value from the private record leaves.
- Widening the real evaluation set means more board APIs, Lever, Ashby and
  Workable share Greenhouse's public pattern, rather than more screenshots.
- This reopens if a screenshot task ever becomes high volume and label rich at
  the same time. That is the condition ADR 0015 sets for training anything, and
  it is the condition to check against, not the appeal of the technique.

## 4. Non-goals

Says nothing about image *generation* or about training LoRAs for it, which is a
separate subsystem with its own ADRs. Scoped to whether model-ops should learn
to train on images in order to serve the job pipeline.
