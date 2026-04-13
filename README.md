# Agent Platform

Multi-agent orchestration on top of [llm-orchestrator](../llm-orchestrator): **goal → planner DAG → human approval → parallel task execution** via OpenAI-compatible `/v1/chat/completions`.

- **API:** `http://127.0.0.1:18410` — OpenAPI (Swagger) at **`/docs`**, human-readable guide at **`/api-guide`** (prefer `/api/v1/...` for integrations). **Do not confuse** with [llm-orchestrator’s integration guide](http://127.0.0.1:18408/docs) on port **18408** (OpenAI-compatible proxy wiring, `/v1/models`, embeddings, streaming).
- **Minimal UI:** `http://127.0.0.1:18410/ui` (HTTP polling; safe on refresh).
- **React app (`/flow`):** Office-style shell (3D simulation, projects, teams) wired to **Agent Platform** REST (`GET/POST /projects`, `/teams`, `/processes`, `POST /api/v1/chat`). Build: `cd web && pnpm install && pnpm run build`. Env: copy [web/env.example](web/env.example) to `web/.env.local` — set `VITE_AGENT_PLATFORM_API_KEY` when the API uses `AGENT_PLATFORM_API_KEY`. See also [docs/adr/0002-ui-stack-react-typescript-vite.md](docs/adr/0002-ui-stack-react-typescript-vite.md) and [0003](docs/adr/0003-future-3d-simulation-boundary.md). In-session agent chat uses **`POST /api/v1/chat`** on Agent Platform (browser → server → orchestrator); dev UI defaults to **`http://127.0.0.1:18410`** unless **`VITE_API_ORIGIN`** overrides it.

## Setup

**llm-orchestrator** must be running and reachable (default `http://127.0.0.1:18408`). Human-readable proxy docs: **[http://127.0.0.1:18408/docs](http://127.0.0.1:18408/docs)** (`OPENAI_BASE_URL` … `/v1`, Bearer key, endpoints). This repo’s API Swagger is **[http://127.0.0.1:18410/docs](http://127.0.0.1:18410/docs)** when agent-platform is up.

```bash
cd agent-platform
cp .env.example .env
# Set ORCHESTRATOR_MASTER_KEY to match llm-orchestrator; ensure orchestrator is on 18408.
# Optional: AGENT_PLATFORM_API_KEY — if set, browsers must send the same key; configure web/.env.local (see web/env.example).
pip install -r app/requirements.txt
cd web && pnpm install && pnpm run build && cd ..
python -m uvicorn main:app --app-dir app --host 127.0.0.1 --port 18410
```

## Docker

**Default** — builds and runs **llm-orchestrator + agent-platform** (needs `../llm-orchestrator/.env`):

```bash
docker-compose up --build
```

(`docker compose up --build` works the same if you use the Compose V2 plugin.)

**Agent Platform only** — if orchestrator is already on the host at `18408`:

```bash
docker-compose -f docker-compose.agent-only.yml up --build
```

Uses `./data` for SQLite. Compose runs uvicorn **without** `--reload` so the API does not exit when bind-mounted code triggers WatchFiles (common on Windows). If you already run llm-orchestrator from its own `docker-compose`, stop that stack first (same host port `18408` and duplicate image otherwise).

## Tools policy (Phase 3)

See [app/tools_policy.py](app/tools_policy.py). Default is **no tools**; enable with env vars documented in `.env.example`.
