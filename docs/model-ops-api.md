# Model Ops API

Agent Platform owns **model build and train**: knowledge merge → dataset → LoRA fine-tune → Ollama export → eval. Clients consume trained models via the embedded LLM proxy (`/v1/chat/completions`).

**Base URL:** `http://127.0.0.1:18410/api/v1`

**OpenAPI:** `/docs` (tag `model-ops`)

---

## Authentication

Same Bearer token as the rest of Agent Platform (`AGENT_PLATFORM_MASTER_KEY` or workspace token `agp_…`).

**Scopes:**

| Scope | Access |
|-------|--------|
| `model:read` | List projects, jobs, registry; Ollama list/show |
| `model:write` | Create projects, start jobs, pull/create/delete Ollama models |

Discover scopes: `GET /api/v1/api-tokens/scopes`

---

## Quick start

```bash
# 1. Create a training project (scaffold from template)
curl -X POST http://127.0.0.1:18410/api/v1/model-ops/projects \
  -H "Authorization: Bearer $AGENT_PLATFORM_MASTER_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name":"my-coach","ollama_tag":"my-coach","base_model":"google/gemma-3-4b-it"}'

# 2. Upload knowledge (chat JSONL under knowledge pack)
curl -X POST "http://127.0.0.1:18410/api/v1/model-ops/projects/my-coach/knowledge" \
  -H "Authorization: Bearer $AGENT_PLATFORM_MASTER_KEY" \
  -F "files=@./examples.jsonl"

# 3. Start a build job
curl -X POST http://127.0.0.1:18410/api/v1/model-ops/jobs \
  -H "Authorization: Bearer $AGENT_PLATFORM_MASTER_KEY" \
  -H "Content-Type: application/json" \
  -d '{"project":"my-coach","stages":["prepare","train","export","eval"],"offline_eval":false}'

# 4. Poll until succeeded
curl http://127.0.0.1:18410/api/v1/model-ops/jobs/1 \
  -H "Authorization: Bearer $AGENT_PLATFORM_MASTER_KEY"

# 5. Chat with the trained model
curl http://127.0.0.1:18410/v1/chat/completions \
  -H "Authorization: Bearer $AGENT_PLATFORM_MASTER_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"my-coach","messages":[{"role":"user","content":"Hello"}]}'
```

---

## Projects

| Method | Path | Description |
|--------|------|-------------|
| GET | `/model-ops/projects` | List projects |
| POST | `/model-ops/projects` | Create from `_template` |
| GET | `/model-ops/projects/{name}` | Manifest + registry entries |
| POST | `/model-ops/projects/{name}/knowledge` | Multipart file upload into `knowledge/{pack}/` |
| POST | `/model-ops/projects/{name}/files` | Multipart upload by relative path (`datasets/train.jsonl`, `project.yaml`, …) |

Knowledge files should be **chat JSONL** (`{"messages":[{"role":"user","content":"..."},{"role":"assistant","content":"..."}]}`) or raw JSONL consumed by your project schema.

---

## Build jobs

| Method | Path | Description |
|--------|------|-------------|
| POST | `/model-ops/jobs` | Start async job |
| GET | `/model-ops/jobs/{id}` | Status + log tail |
| GET | `/model-ops/jobs/{id}/stream` | SSE log stream (`event: log`, `event: done`) |
| POST | `/model-ops/jobs/{id}/cancel` | Cancel running job |

**Stages:** `prepare` (merge + dataset) · `train` (LoRA, GPU) · `export` (GGUF + Ollama create) · `eval`

**Job request body:**

```json
{
  "project": "my-coach",
  "stages": ["prepare", "train", "export", "eval"],
  "register_alias": "my-coach-alias",
  "offline_eval": false,
  "process_id": null,
  "resume": true,
  "adapter_version": "v1",
  "init_from": null
}
```

| Field | Meaning |
|-------|---------|
| `resume` | Default **true**. Pick up this adapter version's last checkpoint if one is on disk. The worker refuses it, and says why in the job log, when the dataset or the hyperparameters have changed since that checkpoint was written. `false` forces a clean run. |
| `adapter_version` | Which `adapters/<version>/` directory to train into. Default `v1`. |
| `init_from` | Continue from another version's trained weights instead of a fresh zero-initialised adapter. |

---

## Progress, resume, and continuation

A fine-tune is the one stage measured in hours, so it reports as it goes.

### Watching a run

`GET /model-ops/jobs/{id}` carries a `progress` object, and every SSE `log`
frame carries the same one beside the new log lines. It holds whatever the
running stage reported:

```json
{
  "stage": "train", "phase": "train",
  "step": 420, "total_steps": 900, "epoch": 1.4,
  "loss": 0.8312, "learning_rate": 0.00018,
  "elapsed_s": 512.4, "eta_s": 585.1,
  "resumed_from": 200,
  "gpu": {"allocated_mb": 5120, "used_mb": 6480, "total_mb": 12288}
}
```

`phase` is `load`, `train`, `save` or `done`. During `load` there is no step
count — a quantized base model takes minutes to arrive — so `phase` plus
`message` is all there is, and a client should show an indeterminate bar rather
than 0%. Progress is a gauge, not a series: only the newest report is kept, and
the history stays in the job log, which is where a loss curve should be read
from. The operator console at `/admin` has a **Training** tab that renders all
of this, and reattaches to a running job across a page reload.

### Picking a run back up

The `train` stage checkpoints about ten times per run (keeping the last two) and
writes a `fingerprint.json` beside the adapter: the dataset's SHA-256 plus the
hyperparameters that produced it. `resume: true` uses the newest checkpoint only
when that fingerprint still matches. This is not caution for its own sake —
`resume_from_checkpoint` restores an optimizer and a step counter without
checking what they were computed for, so resuming onto a grown dataset produces
a plausible adapter that quietly trained on less than it claims.

### Adding to a model

A second round of examples needs `init_from`, or it replaces what the first
round taught rather than adding to it:

```bash
curl -X POST .../model-ops/jobs -H "..." -d '{
  "project": "my-coach", "stages": ["train", "export", "eval"],
  "adapter_version": "v2", "init_from": "v1"
}'
```

`worker/model_ops/pipeline/incremental_train.py` is the same thing with a replay
mix built in: it blends the approved feedback examples with a sample of the
original training set, because a few hundred corrections on their own will
overwrite everything else.

---

## The `prepare` stage refuses personal data

`build_dataset` scans every example before it writes anything and fails the job
if it finds an email address, a phone number, a UK postcode, a National
Insurance number, or an account-length digit run. See
[ADR 0015](adr/0015-job-pipeline-task-model.md) §2.4: a dataset becomes weights
that cannot be edited afterwards, so the gate is a hard failure rather than a
silent drop, and its message names the row and the kind without quoting what it
matched.

It scans **shapes, never literals** — the platform is deliberately not given the
values it is protecting. Money is not a shape it looks for: `£55,000 - £65,000`
is the training signal in a job advert, not a leak, and a gate that fires on
every honest dataset gets switched off. Set `pii_scan: false` in `project.yaml`
for a corpus that is public data with no subject in it.

---

## Orchestration: `model.build`

```bash
curl -X POST http://127.0.0.1:18410/api/v1/model-ops/operations/build \
  -H "Authorization: Bearer $AGENT_PLATFORM_MASTER_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "operation": "model.build",
    "input": {
      "project": "my-coach",
      "stages": ["prepare", "export", "eval"],
      "offline_eval": true
    }
  }'
```

Response:

```json
{
  "operation": "model.build",
  "job_id": 1,
  "poll_url": "/api/v1/model-ops/jobs/1",
  "stream_url": "/api/v1/model-ops/jobs/1/stream"
}
```

---

## Registry

| Method | Path | Description |
|--------|------|-------------|
| GET | `/model-ops/registry` | All trained model entries |
| POST | `/model-ops/registry/{id}/activate` | Set active tag for project |

---

## Ollama lifecycle (proxy)

| Method | Path | Maps to |
|--------|------|---------|
| GET | `/model-ops/ollama/models` | Ollama `/api/tags` |
| GET | `/model-ops/ollama/models/{name}` | Ollama `/api/show` |
| POST | `/model-ops/ollama/models/pull` | Ollama `/api/pull` (sync; `async: true` → tracked job) |
| POST | `/model-ops/ollama/models/copy` | Ollama `/api/copy` (async by default) |
| POST | `/model-ops/ollama/jobs` | Enqueue pull or copy job explicitly |
| POST | `/model-ops/ollama/models/create` | Ollama `/api/create` |
| DELETE | `/model-ops/ollama/models/{name}` | Ollama `/api/delete` |

**Create from Modelfile:**

```json
{
  "name": "my-model",
  "modelfile": "FROM gemma4:latest\nSYSTEM You are a helpful assistant."
}
```

**Pull (async job):**

```json
{ "name": "llama3.2:latest", "async": true }
```

Returns a `ModelBuildJobOut` with `job_type: "ollama_pull"`. Poll via `GET /model-ops/jobs/{id}`.

**Copy:**

```json
{ "source": "llama3.2:latest", "destination": "my-app:latest", "async": true }
```

---

## Process linking

When starting a build job with `process_id`, the orchestration `Process` row gets `model_build_job_id` set for cross-reference in process detail responses.

---

## Environment

| Variable | Purpose |
|----------|---------|
| `MODEL_OPS_DATA_DIR` | Project artifacts (default: `{CONFIG_DIR}/model_ops`) |
| `OLLAMA_API_BASE` | Ollama host for export/eval |
| `MODEL_OPS_PYTHON` | Interpreter (with torch) the server spawns stages with — **required** for build jobs |
| `MODEL_OPS_WORKER_PATH` | Where `worker/` lives; defaults to beside the executable, then the checkout |
| `LLAMA_CPP_BIN` / `LLAMA_CPP_CONVERT` | GGUF conversion tools |
| `HF_TOKEN` | HuggingFace gated models |

**Docker GPU worker** (optional):

```bash
docker compose -f docker-compose.yml -f docker-compose.train.yml --profile train up --build
```

Uses `Dockerfile.train` with `worker/requirements.txt` and NVIDIA GPU reservations.

**Every stage is a subprocess now.** `MODEL_OPS_GPU_SUBPROCESS` is gone: it used
to gate whether `train`/`export` ran in a child process while `prepare`/`eval`
ran inside the API server. The server is Rust ([ADR 0007](adr/0007-strangler-rust-server.md))
and the pipeline is Python, so there is no in-process option — all four stages
run as `MODEL_OPS_PYTHON -c …` children against `worker/`, and they report
structured results by printing `@@AGP:<kind>@@ {json}` lines the server parses
out of the job log.

---

## Wingbot / domain apps

Domain-specific ingest (e.g. LoL match data) stays in the domain app. Upload merged knowledge via `POST …/knowledge` and trigger `POST …/jobs`. Do **not** run local `train_lora.py` in wingbot after migration.

---

## Errors

| Code | Typical cause |
|------|----------------|
| 401 | Missing/invalid Bearer token |
| 403 | Insufficient scope |
| 404 | Unknown project or job |
| 409 | Duplicate project name; job not cancellable |
| 503 | Ollama unreachable |
