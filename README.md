# Agent Platform

Lean **AI server**: multi-agent orchestration API with an **embedded** OpenAI-compatible LLM proxy (`/v1/*` on the same process).

**Portfolio context:** Backend lives here; the optional **Flow UI** (React workspace) is at [`../flow-ui/`](../flow-ui/). Root [`docker-compose.yml`](../docker-compose.yml) runs backend only by default, or backend + Flow UI with `--profile ui`.

- **API:** `http://127.0.0.1:18410` — OpenAPI at **`/docs`**, API guide at **`/api-guide`**
- **Config UI:** `http://127.0.0.1:18410/config` — default provider, model, API keys, `config.yaml`
- **Process demo:** `http://127.0.0.1:18410/ui` — minimal polling UI for `/processes`
- **Flow UI (optional):** `http://127.0.0.1:3333/app/` when started via root compose `--profile ui`

## Deploy profiles

| Profile | Command | What runs |
|--------|---------|-----------|
| **Backend only** | `docker compose up --build` (this folder) | API + config UI on `:18410` |
| **Backend + Flow UI dev** | `docker compose --profile ui up --build` (repo root) | API `:18410` + Vite `:3333` |
| **Backend + Flow UI prod** | `docker compose --profile ui-prod up --build` (repo root) | API `:18410` + nginx `:18408` |

Local dev without Docker:

```bash
cd agent-platform
cp .env.example .env
pip install -r app/requirements.txt
python -m uvicorn main:app --app-dir app --host 127.0.0.1 --port 18410
```

Flow UI dev (separate terminal):

```bash
cd flow-ui
pnpm install && pnpm run dev
```

Set **`AGENT_PLATFORM_MASTER_KEY`** in `.env` (Bearer for `/v1` and protected `/api/v1/*`). When set, paste the same key in the config UI auth bar or set `VITE_AGENT_PLATFORM_MASTER_KEY` for Flow UI.

### Workspace tokens (external integrations)

Each microservice or Flow UI deployment gets **one workspace-scoped token** (`agp_…`). Mint tokens at `/tokens` or `POST /api/v1/workspaces/{id}/api-tokens/` (master key). The service resolves its tenant via `GET /api/v1/me/workspace`. See [docs/CLIENT_INTEGRATION.md](docs/CLIENT_INTEGRATION.md).

## Docker (backend only)

```bash
docker compose up --build
```

Open **`http://127.0.0.1:18410/config`** (LLM settings) and **`http://127.0.0.1:18410/docs`** (API).

Uses a named volume for SQLite, workspaces, and **`/app/data/llm`** (`config.yaml` + `.env`).

### Performance tuning (high-core desktop)

Set in `agent-platform/.env` before `docker compose up --build`:

```bash
AGENT_PLATFORM_UVICORN_WORKERS=8
AGENT_PLATFORM_DAG_MAX_CONCURRENT_TASKS=12
```

**LM Studio / Ollama on Docker Desktop:** keep loopback URLs in config — the API rewrites `127.0.0.1` to `host.docker.internal` inside the container (`AGENT_PLATFORM_LOCAL_LLM_DOCKER_FIX=1`).

## Tools policy (Phase 3)

See [app/tools_policy.py](app/tools_policy.py). Default is **no tools**; enable with env vars documented in `.env.example`.

## Hygiene checks

Run `python scripts/check_repo_hygiene.py` from this folder.
