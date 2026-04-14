# Agent Platform

Multi-agent orchestration with an **embedded** OpenAI-compatible LLM proxy: **goal → planner DAG → human approval → parallel task execution** via `/v1/chat/completions` on the same process as the Agent Platform API.

- **API:** `http://127.0.0.1:18410` — OpenAPI (Swagger) at **`/docs`**, human-readable guide at **`/api-guide`** (prefer `/api/v1/...` for integrations). **OpenAI-compatible proxy** endpoints: **`/v1/models`**, **`/v1/chat/completions`**, **`/v1/embeddings`** (Bearer `AGENT_PLATFORM_MASTER_KEY`). Configure providers and `config.yaml` under **Flow → Settings → LLM proxy (server)** or via **`/api/v1/llm-proxy/*`**.
- **Minimal UI:** `http://127.0.0.1:18410/ui` (HTTP polling; safe on refresh).
- **React app (`/flow`):** Office-style shell (3D simulation, projects, teams) wired to **Agent Platform** REST (`GET/POST /projects`, `/teams`, `/processes`, `POST /api/v1/chat`). Build: `cd web && pnpm install && pnpm run build`. Env: copy [web/env.example](web/env.example) to `web/.env.local` — set `VITE_AGENT_PLATFORM_MASTER_KEY` when API auth is enabled. See also [docs/adr/0002-ui-stack-react-typescript-vite.md](docs/adr/0002-ui-stack-react-typescript-vite.md) and [0003](docs/adr/0003-future-3d-simulation-boundary.md). In-session agent chat uses **`POST /api/v1/chat`** on Agent Platform (browser → server → embedded `/v1`); dev UI defaults to **`http://127.0.0.1:18410`** unless **`VITE_API_ORIGIN`** overrides it.

## Setup

Set **`AGENT_PLATFORM_MASTER_KEY`** in `.env` (Bearer for `/v1` and for internal HTTP calls to the embedded proxy). Default **`LLM_ORCHESTRATOR_BASE_URL`** (legacy name) is `http://127.0.0.1:18410/v1` (same process). API Swagger: **[http://127.0.0.1:18410/docs](http://127.0.0.1:18410/docs)**.

```bash
cd agent-platform
cp .env.example .env
# Set AGENT_PLATFORM_MASTER_KEY; optional overrides for LLM_ORCHESTRATOR_BASE_URL / CONFIG_DIR.
# Optional: API auth reuses AGENT_PLATFORM_MASTER_KEY; if set, browsers must send the same key via web/.env.local (see web/env.example).
pip install -r app/requirements.txt
cd web && pnpm install && pnpm run build && cd ..
python -m uvicorn main:app --app-dir app --host 127.0.0.1 --port 18410
```

## Docker

**Default** — **agent-platform** (API + embedded LLM proxy + Vite UI):

```bash
docker-compose up --build
```

(`docker compose up --build` works the same if you use the Compose V2 plugin.)

**Agent Platform API only** (no separate Vite service in that file):

```bash
docker-compose -f docker-compose.agent-only.yml up --build
```

Uses a named volume for SQLite, workspaces, and **`/app/data/llm`** (`config.yaml` + `.env` for the proxy). Compose runs uvicorn **without** `--reload` so the API does not exit when bind-mounted code triggers WatchFiles (common on Windows).

### Performance tuning (high-core desktop)

For stronger local throughput (for example Ryzen 9 9950X + 64 GB), set these in `agent-platform/.env` before `docker compose up --build`:

```bash
AGENT_PLATFORM_UVICORN_WORKERS=8
AGENT_PLATFORM_DAG_MAX_CONCURRENT_TASKS=12
UV_THREADPOOL_SIZE=16
OMP_NUM_THREADS=16
OPENBLAS_NUM_THREADS=16
MKL_NUM_THREADS=16
NUMEXPR_NUM_THREADS=16
```

Notes:
- Increase `AGENT_PLATFORM_UVICORN_WORKERS` to `10-12` only if CPU is still underused and request latency remains stable.
- If local LLM generation becomes slower after raising thread counts, reduce `OMP_NUM_THREADS` (and related BLAS vars) to `8`.
- Docker Desktop resource caps still apply; ensure enough CPUs/RAM are assigned in Docker Desktop settings.

**LM Studio / Ollama on the same machine as Docker Desktop:** keep `LM_STUDIO_API_BASE` / `OLLAMA_API_BASE` as `http://127.0.0.1:…` in `config/agent_platform.yaml` or Flow → **Settings → LLM proxy** — the API rewrites loopback to `host.docker.internal` inside the container. To use a fixed LAN URL (as shown in LM Studio’s *Local Server* UI), set `LM_STUDIO_API_BASE` to that URL instead (e.g. `http://192.168.x.x:1234`). Opt out of rewrite with `AGENT_PLATFORM_LOCAL_LLM_DOCKER_FIX=0`.

## Tools policy (Phase 3)

See [app/tools_policy.py](app/tools_policy.py). Default is **no tools**; enable with env vars documented in `.env.example`.

## Hygiene checks

Run `python scripts/check_repo_hygiene.py` to detect:
- Backslash-tracked git paths and duplicate normalized paths.
- Parent-relative imports under `web/src` (prefer `@/...` aliases).
