# Agent Platform

Lean **AI server**: multi-agent orchestration API with an **embedded** OpenAI-compatible LLM proxy (`/v1/*` on the same process).

**Portfolio context:** Backend and canonical **Flow UI** live in this repo. UI workspace is under [`web/`](web/). One **`agent-platform`** image builds both; run mode is set with **`AGENT_PLATFORM_CONTAINER_MODE`** (`backend` | `ui` | `all`).

- **API:** `http://127.0.0.1:18410` — OpenAPI at **`/docs`**, API guide at **`/api-guide`**
- **Config UI:** `http://127.0.0.1:18410/config` — default provider, model, API keys, `config.yaml`
- **Process demo:** `http://127.0.0.1:18410/ui` — minimal polling UI for `/processes`
- **Flow UI (dev):** `http://127.0.0.1:3333/app/` — Vite dev server with API proxy
- **Flow UI (Docker):** `http://127.0.0.1:3333/app/` (default compose: API + UI containers). Single-container `all` mode serves UI on `:18408/app/`.

Provider catalog behavior is normalized across `/api/v1/llm/ui-catalog`, `/api/v1/llm-proxy/ui/providers`, and `/api/v1/llm-proxy/test/model-options`: each provider exposes the same capability shape (`streaming`, `tools`, `json_mode`, `model_discovery`). When a provider cannot list models live, the server falls back in order to provider aliases from `config.yaml`, then `orchestrator_ui.yaml` `fallback_models`, then the provider default model.

### BYOK (bring-your-own-key)

Clients can forward `/v1/chat/completions`, `/v1/embeddings`, and `/v1/images/generations` through **their own** provider key — the server proxies to the vendor with the caller's credential and spends none of its own quota. The platform token still gates access; BYOK only swaps the upstream credential. Activate per-request with headers (body stays OpenAI-compatible):

```
X-BYOK-Provider: openai            # openai|anthropic|gemini|aimlapi|openrouter|groq|mistral
X-BYOK-Api-Key:  sk-...            # the caller's upstream key (never logged)
X-BYOK-Base-Url: https://...       # optional; host must be allowlisted
X-BYOK-Anthropic-Version: ...      # optional; overrides the anthropic-version pin
```

```bash
curl http://127.0.0.1:18410/v1/chat/completions \
  -H "Authorization: Bearer $AGENT_PLATFORM_TOKEN" \
  -H "X-BYOK-Provider: openai" -H "X-BYOK-Api-Key: sk-..." \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}'
```

`model` is passed through untouched (use your vendor's model ids). A custom `X-BYOK-Base-Url` is accepted only for the provider's canonical host or a host in **`BYOK_ALLOWED_HOSTS`** (comma-separated), must be `https`, and cannot be a raw IP — this blocks pointing the proxy at internal services (SSRF). Unsupported capabilities return a structured `501` (e.g. Claude has no embeddings surface).

Discover supported BYOK providers, their modalities, and the header names programmatically from the `byok` block of `GET /v1/capabilities`.

## Quick start

First-time setup from this folder:

```bash
cp .env.example .env
pnpm install          # root: Python deps (postinstall) + dev tooling
cd web && pnpm install && cd ..
```

Set **`AGENT_PLATFORM_MASTER_KEY`** in `.env` (Bearer for `/v1` and protected `/api/v1/*`). When set, paste the same key in the config UI auth bar or set `VITE_AGENT_PLATFORM_MASTER_KEY` for Flow UI.

| Mode | Local (no Docker) | Docker |
|------|-------------------|--------|
| **Backend only** | `pnpm dev:server` | `pnpm docker:up:server` |
| **Backend + Flow UI** | `pnpm dev` | `pnpm docker:up` |
| **Flow UI only** (API elsewhere) | `cd web && pnpm dev` | `pnpm docker:up:ui-only` |

URLs after start:

| Mode | API / config | Flow UI |
|------|--------------|---------|
| Local backend only | `:18410` | — |
| Local backend + web | `:18410` | `:3333/app/` |
| Docker backend only | `:18410` | — |
| Docker backend + web | `:18410` | `:3333/app/` |

Verify setup (offline — no server required):

```bash
pnpm smoke
```

With API already running:

```bash
pnpm smoke:live
# or: python scripts/smoke_workflow.py --live http://127.0.0.1:18410
```

## Deploy profiles

| Profile | Command | What runs |
|--------|---------|-----------|
| **Backend only** | `pnpm docker:up:server` | API + config UI on `:18410` (no Flow UI container) |
| **Backend + Flow UI** | `pnpm docker:up` | API `:18410` + Flow UI dev container on host `:3333` |
| **Flow UI only** | `pnpm docker:up:ui-only` | Static Flow UI on `:3333` (API elsewhere) |

Equivalent npm scripts: `pnpm docker:up:server`, `pnpm docker:up`, `pnpm docker:up:ui-only`.

### Workspace tokens (external integrations)

Each microservice or Flow UI deployment gets **one workspace-scoped token** (`agp_…`). Mint tokens at `/tokens` or `POST /api/v1/workspaces/{id}/api-tokens/` (master key). The service resolves its tenant via `GET /api/v1/me/workspace`. See [docs/CLIENT_INTEGRATION.md](docs/CLIENT_INTEGRATION.md).

## Docker (unified image)

Image name: **`agent-platform`**. Main [`Dockerfile`](Dockerfile) builds FastAPI backend and Flow UI static assets from this checkout.

```bash
# Backend only
pnpm docker:up:server

# Backend + Flow UI (default compose)
pnpm docker:up
```

Set **`AGENT_PLATFORM_CONTAINER_MODE`** to `backend`, `ui`, or `all` (see `.env.example`).

Uses a named volume for SQLite, workspaces, and **`/app/data/llm`** (`config.yaml` + `.env`).

## Repo structure

- `app/` FastAPI backend, API routes, orchestration, tests
- `web/` canonical React + Vite Flow UI workspace
- `docker/` nginx + entrypoint files for unified container modes
- `docs/` ADRs, plans, integration notes

### Performance tuning (high-core desktop)

Set in `agent-platform/.env` before `docker compose up --build`:

```bash
AGENT_PLATFORM_UVICORN_WORKERS=8
AGENT_PLATFORM_DAG_MAX_CONCURRENT_TASKS=12
```

**LM Studio / Ollama on Docker Desktop:** keep loopback URLs in config — the API rewrites `127.0.0.1` to `host.docker.internal` inside the container (`AGENT_PLATFORM_LOCAL_LLM_DOCKER_FIX=1`).

## Tools policy (Phase 3)

See [app/tools_policy.py](app/tools_policy.py). Default is **no tools**; enable with env vars documented in `.env.example`.

## Hygiene and smoke checks

```bash
pnpm smoke              # hygiene + web-facing API contract tests (no running server)
python scripts/check_repo_hygiene.py   # hygiene only
```
