# ADR 0015: Train task models for the job pipeline from its own decision log

**Status:** Proposed
**Date:** 2026-08-27
**Deciders:** (fill when decided)
**Tags:** model-ops, LoRA, distillation, portfolio, dataset capture, eval

---

## 1. Context

Two systems already exist and almost meet:

- **This platform owns model build and train.** The model-ops pipeline
  (knowledge merge, dataset build, LoRA fine-tune, GGUF/Ollama export, eval)
  runs as a spawned Python worker with a GPU container
  (`Dockerfile.train`, `docker-compose.train.yml`), driven over HTTP
  (`docs/model-ops-api.md`). Its input format is chat JSONL:
  `{"messages":[{"role":"user",...},{"role":"assistant",...}]}`. It already
  supports incremental training (`worker/model_ops/pipeline/incremental_train.py`)
  and per-project eval against Ollama.

- **The portfolio runs a job-application pipeline on a frontier model.** The
  assistant (`POST /api/admin/chat`) holds the Assessor, Writer and Recruiter
  roles; subagents (sourcer, screener, assessor, recruiter, writer, form-pilot,
  registrar) each take one step. Every step produces a triple of
  (input, decision, reason): a screening verdict per constraint, a fit rating
  per requirement, a recruiter finding per claim, a form-field mapping per
  input. Today those land in job rows, the audit log and chat history, shaped
  for the UI, not for training. Notably, `portfolio-core/src/lib/chat.ts`
  already converts its internal history to OpenAI-format messages "for a local
  model behind an OpenAI-compatible proxy": the consuming side of a local model
  is wired before the model exists.

The question this ADR answers: is a custom model for job-application
automation worth building, where does enough data come from, and how should
the portfolio store its decisions so training happens under controlled,
replayable conditions.

## 2. Decision

### 2.1 Train narrow task models, not a "job application model"

Fine-tuning is worth it only for sub-tasks that are **mechanical, high-volume
and verifiable**, where a small local model replaces a frontier call:

| task | input | output | why it suits a small model |
|---|---|---|---|
| advert extraction | raw page text | structured `JobResult` fields | pure structure recovery, exact-match evaluable |
| screening | advert + constraint list | `yes/no/unknown` per constraint, one-line reason | closed label set, high volume (every imported advert) |
| fit pre-scoring | advert + record slice | `green/amber/red` per requirement, evidence quote | closed label set, checkable against the advert text |
| form-field mapping | field label + surrounding context | record path, or `gate` when the answer is the owner's | small vocabulary, wrong answers are visible before submit |

**Tool calling and vision are not on this axis.** A screener is a text
classifier that answers in twenty tokens; a model that drives a browser, calls
tools and reads a screenshot of a form is a different model with a different
base (a VLM in the 7B class, instruction-tuned for tool use) and a different
budget. Conflating them gets the worst of both: a small model taught to emit
JSON tool calls it cannot reliably close, and a large one paying vision weights
to answer yes/no about a salary line. The assistant and the form pilot keep
their frontier model precisely because those two capabilities are what they are
for. If a local tool-calling model is wanted later it is its own model-ops
project, its own eval, and its own row in the registry.

Explicitly **not** trained: CV and letter prose (low volume, taste-driven, the
recruiter-facing artifact stays frontier plus human review) and the final
apply/skip decision (judgement, stays with the assistant and the owner).

One model-ops project per task (`jobhunt-extractor`, `jobhunt-screener`,
`jobhunt-scorer`, `jobhunt-fieldmapper`), each on a 4B-class base. Separate
small models beat one merged model here: each task has its own eval set,
its own activation decision in the registry, and its own rollback.

### 2.2 Data: three feeds into one JSONL shape

Volume is the binding constraint. One hunt screens on the order of 77
adverts: roughly 77 extraction rows and a few hundred constraint verdicts.
LoRA on a 4B base wants roughly 500 to 5,000 clean examples per task before
it reliably beats prompting the base model. So:

1. **Exhaust.** Every pipeline decision is persisted as a training event at
   the moment it is made (see 2.3). Free, but slow to accumulate.
2. **Distillation.** The frontier model is the teacher. Job adverts are
   public and effectively unlimited; the sourcer already imports them. Run a
   sweep: source adverts well beyond what will be applied to, run the
   existing screening and scoring prompts over them, store the labelled
   pairs, never apply. Each label costs one frontier call; this is where the
   thousands come from.
3. **Corrections.** When the owner overrides a verdict in the dashboard
   (re-screens a parked row, re-rates a fit), that becomes a gold row that
   outranks the teacher row for the same input. Human-corrected rows are
   held out for eval, never trained on, so the eval set stays honest.

### 2.3 The portfolio captures decisions as replayable events

New collection `decision_events` (MongoDB, beside the job rows), one document
per decision:

```json
{
  "task": "screening",
  "input": { "advertText": "...", "recordSlice": { }, "promptVersion": "..." },
  "output": { "rightToWork": {"verdict": "yes", "reason": "..."}, ... },
  "actor": { "kind": "model", "id": "claude-opus-5" },
  "jobId": "...",
  "outcome": null,
  "createdAt": "..."
}
```

Rules that make it a controlled environment rather than a log:

- **Inputs are snapshotted in full**, advert text and the record slice
  included, not referenced. A referenced record mutates; a snapshot replays.
  Replaying every stored input through a candidate model and diffing against
  stored outputs **is** the eval, and it feeds the model-ops `eval` stage.
- **Prompt and schema versions ride along.** `JobResult` and the constraint
  set will drift; an event is only trainable with the schema it was made
  under, so the export filters by version.
- **Outcomes back-fill.** When an application progresses (response,
  interview, offer), the registrar writes it onto the chain of events for
  that job. Not used for supervised training initially; it is the label
  source a later preference-tuning pass would need, and it costs nothing to
  record now.
- **`actor` distinguishes teacher, student and human.** A deployed local
  model's own decisions are recorded too, but never trained on unless a
  human or frontier pass confirmed them; a model must not feed on itself.

Export is one admin route, `GET /api/admin/training/export?task=...&split=...`,
emitting chat JSONL in exactly the shape model-ops ingests: **two messages, user
first** — the input snapshot as the user turn, the output as the assistant turn.
The task's system prompt is *not* a third message; it lives in the project's
`export/system.txt` and `export_ollama` bakes it into the Modelfile. A row that
carries its own system turn is dropped silently by `build_dataset`, which reads
`messages[0]` as the input JSON, and `eval.py` likewise reads `[0]` and `[1]` as
prompt and expected answer. Guarded by a new agent-token scope `training:read`.

### 2.4 Sensitive data never reaches the weights

A fine-tuned model memorises its dataset; anything trained in can be prompted
back out, and an exported GGUF file carries it wherever the file goes. So the
rule is not "mask on export", it is **mask at capture**: a `decision_events`
document never holds a sensitive value in the first place, and no later bug in
the export path can leak what was never stored.

Classification, following the record's own split (public content vs
`/api/admin/private`):

| class | examples | treatment in events |
|---|---|---|
| public by design | advert text, company, project names, stack, public summary | stored verbatim |
| identity | name, email, phone, address, links | replaced with stable placeholder tokens: `{{NAME}}`, `{{EMAIL}}`, `{{PHONE}}` |
| private-record fields | salary expectation, notice period, right-to-work detail | never stored raw; stored as the **derived predicate** the decision actually consumed, e.g. `salary: "meets_floor"`, `rtw: "eligible_no_sponsorship"` |
| third parties | recruiter names and emails in adverts or inbox rows | placeholder tokens, same as identity |

Two consequences fall out and both improve the models:

- **Placeholders generalise.** A screener trained on `{{NAME}}` and
  `salary: "meets_floor"` learns the decision function, not the owner. The
  same weights would work for a different profile, which is the correctness
  property wanted anyway.
- **Form-field mapping needs no values at all.** Its output is a record
  path or `gate`, never the content behind the path; the value is joined in
  at fill time by the form-pilot, which reads the live private record. The
  training data for the sharpest task is therefore PII-free by construction.

Mechanics: masking runs inside the capture write, as one shared function that
takes the private record and returns (masked slice, predicate set). The same
function runs over advert text and reasons/evidence strings before storage,
since a model-written reason can quote an input. The replay property survives
because masking is deterministic: replaying a candidate model over the masked
snapshot reproduces the conditions the label was made under.

That is the policy. What makes it an architecture rather than a convention
is the layering below: one control per failure mode, each verifiable on its
own, so no single bug is sufficient for a leak.

**Design stance:** treat every trained artifact as if it will eventually be
public. A GGUF file is copied, backed up, and shared the way any file is;
the architecture must hold even when the weights do.

| layer | where it runs | mechanism | failure it stops |
|---|---|---|---|
| L0 classify | private record schema | every field carries a sensitivity tag at definition; the masker derives its rules from the tags, not from a hand-kept list | "we forgot that field was sensitive" when the record grows |
| L1 structure | `decision_events` schema | the event type has **no fields that can hold raw sensitive values**: identity slots are placeholder-typed, private-record inputs are typed predicate enums | masking skipped on one code path; illegal states unrepresentable beats remembering to call a function |
| L2 capture | the one write path | deterministic masker: (private record) → (masked slice, predicate set), also run over advert text and model-written reasons, which can quote inputs | raw values at rest |
| L3 export | portfolio export route | literal-aware re-scan against the live private record; a hit **fails the export**, never silently drops the row | masker regression after a record edit |
| L4 prepare | platform worker (`build_dataset`) | pattern-based PII scan (email, phone, postcode, National Insurance, account-length digit runs), needing no literals; hard-gates the build | unmasked upload from any client, this portfolio or the next one |
| L5 post-train | model-ops `eval` stage | extraction battery: prompts that ask the model for the owner's details must yield placeholders or refusals; canary check below | memorisation that survived everything upstream |
| L6 lineage | model-ops registry | each entry records dataset manifest hashes and masker version; a found leak taints exactly the models built from that data, which are then deleted and rebuilt | "which models are dirty" being unanswerable |

**Money is not one of the shapes.** An earlier draft of this ADR listed
salary-figure patterns at L4, and implementing it showed why that is wrong:
`£55,000 - £65,000` is the training signal in a job advert, not a leak. A gate
that fires on every honest dataset in the domain is a gate that gets switched
off. Salary stays an L3 concern, where the exporter holds the record and can
compare literals.

The trust boundary runs both directions. The platform does not trust the
portfolio to have masked: L4 runs regardless of the client, on patterns
alone, precisely because the platform must never be handed the literals it
would need for an exact check. The portfolio does not trust the platform's
storage or logs: masking upstream at L2 means job logs, knowledge files in
the shared volume, and eval transcripts only ever see masked text.

**Canaries make the controls measurable.** Two kinds, testing different
things:

- *Masker canaries:* synthetic secrets shaped like real PII (a fake phone
  number, a fake salary figure) injected upstream of the masker in tests.
  Zero occurrences in the produced dataset is a unit-test assertion; the
  masker is only trusted while it passes.
- *Memorisation canaries:* a handful of unique synthetic strings placed
  **deliberately** in the training set (the secret-sharer method). The eval
  stage measures how readily the model reproduces them. This calibrates the
  real risk: it answers "if one real value had slipped through, how
  extractable would it be" with a number instead of a guess.

**Deferred, with a named trigger:** differentially private fine-tuning
(DP-SGD) is the heavyweight control and is not justified while weights are
built and served on the owner's own machine; the layers above are the
proportionate defence. The trigger that revisits this line is distribution:
if a trained model is ever to be published or shipped to another party,
DP training plus a formal extraction audit becomes a precondition, not an
option.

### 2.5 The loop across the two systems

```
portfolio: decision_events  --export JSONL-->  POST /model-ops/projects/{p}/knowledge
                                              POST /model-ops/jobs  (prepare, train, export, eval)
                                              registry activate on eval pass
portfolio: sub-task call  --OpenAI format-->  /v1/chat/completions  (local tag)
                          fallback: frontier when local is down or low-confidence
```

Routing lives where the sub-task's model call is made today: try the local
tag, fall back to the frontier model on error, and for a burn-in period run
**shadow mode**, local model decides, frontier decides, both are recorded,
only the frontier's answer is used, disagreement rate gates promotion.

## 3. What gates activation

A task model replaces frontier calls only when, on the held-out
human-corrected split:

- extraction: field-level exact match at or above the frontier baseline
- screening and scoring: verdict agreement at or above 95% of the frontier's
  self-consistency (the frontier disagrees with itself on re-runs; that is
  the ceiling to measure against, not 100%)
- form-field mapping: zero wrong-path mappings on the eval set; an `unknown`
  is acceptable, a wrong record path is not
- privacy: the L5 extraction battery passes and memorisation-canary
  reproduction is at the noise floor; a model that fails privacy eval does
  not activate regardless of its accuracy

Until then the trained model runs in shadow mode or not at all.

## 4. Consequences

**Gains**

- Mechanical calls (the bulk of a sweep) move to local, cutting cost and
  latency; a screening sweep stops costing a frontier call per advert.
- Private-record slices in those calls stay on the machine.
- The eval set doubles as a regression harness for frontier prompt changes:
  a prompt edit that shifts verdicts on replayed inputs is visible before it
  ships.

**Costs and risks**

- Schema drift: `JobResult`, constraints and prompts must version together
  with the events, or old events silently poison new datasets. The
  `promptVersion` filter is load-bearing.
- Teacher noise: distilled labels inherit frontier mistakes. Corrections and
  the self-consistency ceiling bound this but do not remove it.
- Form filling is the sharp edge: a wrong mapping puts wrong data in a real
  application. Mitigated structurally, the form-pilot keeps its gate and the
  model only proposes; the zero-wrong-path bar above is the other half.
- One more collection and one more route in the portfolio; the platform side
  needs no new code, only projects.

## 5. Implementation status

Built (2026-08-27), platform side:

- **L4** — `worker/model_ops/pipeline/pii_scan.py`, called from `build_dataset`
  before the train/eval split and before anything is written. Fails the job,
  names the row and the kind, and never quotes what it matched.
- **L6 lineage** — the training stage registers `dataset_sha256`, `init_from`,
  `steps` and `train_loss` with the model, and `GET /model-ops/registry`
  exposes them.
- **Progress and resume** — the `train` stage reports phase, step, loss, epoch
  and ETA on `@@AGP:progress@@` marker lines; the server keeps the newest on the
  job row and streams it beside the log. It checkpoints as it runs and resumes
  only when a fingerprint of the dataset and the hyperparameters still matches.
  `init_from` continues a previous adapter. `/admin` has a Training tab that
  renders it.

Built, portfolio side:

- **L0** — `portfolio-core/src/lib/masking.ts`, `CONTACT_SENSITIVITY`. Every
  field of the private contact block is tagged `public`, `identity` or
  `predicate`, and the masker derives its rules from the tags. A field added to
  `PrivateData` without a tag is a TypeScript error rather than a quiet leak.
- **L1** — `decision-events.ts`. The event type has no field able to hold a raw
  sensitive value: identity arrives masked, the record arrives only as
  `Predicates`, whose members are closed enumerations. A path that skips the
  masker cannot produce a valid event.
- **L2** — `maskText`, `maskDeep`, `maskedContact`, called at the capture point
  in `POST /api/admin/jobs`. Predicates are computed in code, not asked of the
  model: `salaryPredicate` compares two numbers, so the owner's figure never
  needs to enter an example at all.
- **L3** — `GET /api/admin/training` re-scans every selected row against the
  live record and answers 409 rather than exporting. Findings name the field and
  the path and never the value.

The one deviation from §2.3 worth recording: the student's input format is not
the frontier's. The teacher gets the record and reasons; the student gets a
masked advert plus pre-computed predicates. That is ordinary distillation —
teacher and student share the *label*, not the prompt — and it is what lets the
private figures stay out of the corpus entirely. Replay-as-eval is unaffected,
because a stored input is already in the student's format.

Not built yet:

- Capture for the other three tasks. Only `scoring` is wired; extraction,
  screening and field mapping have their `DecisionTask` and system prompt and no
  call site.
- **L5** — the extraction battery and memorisation canaries in the `eval` stage.
- The four task projects: `jobhunt-screener` is scaffolded and empty (manifest,
  input schema, system prompt, installed by `worker/install_project.py`); the
  other three are not created. The distillation sweep that fills them and the
  shadow-mode routing of §2.5 are not built either — the corpus is the blocker,
  not the pipeline.

## 6. Non-goals

- A generative CV or letter writer.
- Online or continuous learning: builds are batch, versioned, and evaluated
  before activation, per the existing model-ops registry flow.
- Preference tuning on outcomes (interviews, offers): recorded from day one,
  acted on only if supervised task models prove out first.
