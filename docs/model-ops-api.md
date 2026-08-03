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
  "process_id": null
}
```

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
| `MODEL_OPS_GPU_SUBPROCESS` | `1` (default) run train/export in child process |
| `LLAMA_CPP_BIN` / `LLAMA_CPP_CONVERT` | GGUF conversion tools |
| `HF_TOKEN` | HuggingFace gated models |

**Docker GPU worker** (optional):

```bash
docker compose -f docker-compose.yml -f docker-compose.train.yml --profile train up --build
```

Uses `Dockerfile.train` with `app/model_ops/requirements-train.txt` and NVIDIA GPU reservations.

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
