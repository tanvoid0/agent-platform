# Client Integration Guide

End-to-end setup for an external microservice or third-party client talking to Agent Platform.

## 0. Expose the server (only if the client is not on this machine)

`agent-platformd` binds `127.0.0.1:18410` and serves the whole API itself — there
is no Python server and no separate gateway. A client on the same box needs
nothing from this section.

```bash
AGENT_PLATFORM_HOST=0.0.0.0 \
AGENT_PLATFORM_MASTER_KEY=<key> \
AGENT_PLATFORM_CORS_ORIGINS=https://yourapp.example \
  agent-platformd
```

- **`AGENT_PLATFORM_HOST`** — bind address. Default `127.0.0.1`.
- **`AGENT_PLATFORM_MASTER_KEY`** — **required** before binding beyond loopback.
  With no master key set, auth is fully open; that is a deliberate local-dev
  convenience (`desktop/crates/server/src/auth.rs`) and it becomes an open
  database the moment the port is reachable.
- **`AGENT_PLATFORM_CORS_ORIGINS`** — comma-separated origins, only needed for
  callers that are *browsers*. Server-to-server clients never send `Origin`.
  Unset means no CORS layer at all. There is no wildcard, on purpose: a `*` here
  plus a `Bearer agp_…` token is how a token leaks to whatever page the user has
  open.

### TLS

The server speaks plain HTTP, so bearer tokens cross the wire in clear. Put a
reverse proxy in front rather than terminating TLS in the process — it is the
smaller moving part and it handles certificate renewal:

```caddy
api.yourdomain.example {
    reverse_proxy 127.0.0.1:18410
}
```

With a proxy in front, keep the bind on `127.0.0.1` — only the proxy needs to
reach it. `AGENT_PLATFORM_PUBLIC_HOST` / `AGENT_PLATFORM_PUBLIC_PORT` tell
`GET /api/v1/system/status` what to report as the reachable address when it differs
from the bind.

Buffering note: several routes stream (SSE) — the process run stream, model-ops
job stream, chat completions. A proxy that buffers responses turns those into a
single delayed blob. Caddy's `reverse_proxy` streams by default; nginx needs
`proxy_buffering off;`.

## 1. Obtain credentials

**Admin (one-time):** set `AGENT_PLATFORM_MASTER_KEY` on the server and in your local `.env`.

**Per microservice:** mint a workspace-scoped token from the dashboard or API:

```bash
# List workspaces (master key)
curl -s http://127.0.0.1:18410/api/v1/workspaces/ \
  -H "Authorization: Bearer $AGENT_PLATFORM_MASTER_KEY"

# Mint a token for workspace id 1
curl -s -X POST http://127.0.0.1:18410/api/v1/workspaces/1/api-tokens/ \
  -H "Authorization: Bearer $AGENT_PLATFORM_MASTER_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name":"my-service","scopes":["*"]}'
```

Copy the `token` field from the response — it is shown **once**.

## 2. Configure your service

```bash
# .env
AGENT_PLATFORM_TOKEN=agp_xxxxxxxx
AGENT_PLATFORM_BASE_URL=http://127.0.0.1:18410
```

## 3. Resolve your workspace

```bash
curl -s http://127.0.0.1:18410/api/v1/me/workspace \
  -H "Authorization: Bearer $AGENT_PLATFORM_TOKEN"
```

Response: `{ "id": 1, "name": "Default", "slug": "default", ... }`

No workspace id is needed in `.env` — the token binds you to one tenant.

## 4. List and create projects

```bash
# List (scoped to your workspace automatically)
curl -s http://127.0.0.1:18410/api/v1/projects/ \
  -H "Authorization: Bearer $AGENT_PLATFORM_TOKEN"

# Create
curl -s -X POST http://127.0.0.1:18410/api/v1/projects/ \
  -H "Authorization: Bearer $AGENT_PLATFORM_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"name":"my-app","workspace_id":1}'
```

## 5. Run a process

```bash
# Pick a team template
curl -s http://127.0.0.1:18410/api/v1/teams/ \
  -H "Authorization: Bearer $AGENT_PLATFORM_TOKEN"

# Start orchestration
curl -s -X POST http://127.0.0.1:18410/api/v1/processes \
  -H "Authorization: Bearer $AGENT_PLATFORM_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"goal":"Summarize the README","team_template_id":1,"project_id":1,"auto_approve":true}'
```

Poll `GET /api/v1/processes/{id}` until `status` is `completed` or `failed`.

## 6. Upload files (optional)

```bash
curl -s -X POST "http://127.0.0.1:18410/api/v1/projects/1/files/upload?dest=documents" \
  -H "Authorization: Bearer $AGENT_PLATFORM_TOKEN" \
  -F "file=@./input.pdf"
```

## Minimal Python example

```python
import os, httpx

BASE = os.environ["AGENT_PLATFORM_BASE_URL"]
TOKEN = os.environ["AGENT_PLATFORM_TOKEN"]
H = {"Authorization": f"Bearer {TOKEN}"}

with httpx.Client(base_url=BASE, headers=H, timeout=60) as c:
    ws = c.get("/api/v1/me/workspace").json()
    print("workspace:", ws["slug"])
    projects = c.get("/api/v1/projects/").json()["projects"]
    project_id = projects[0]["id"]
    proc = c.post("/api/v1/processes", json={
        "goal": "Hello from SDK",
        "team_template_id": 1,
        "project_id": project_id,
        "auto_approve": True,
    }).json()
    print("process:", proc["id"], proc["status"])
```

## Further reading

- [API_WORKSPACE_SCOPING.md](./API_WORKSPACE_SCOPING.md) — isolation rules and endpoint reference
- [model-ops-api.md](./model-ops-api.md) — build/train custom Ollama models (LoRA pipeline)
- `/openapi.json` — OpenAPI reference. (It was `/docs`, FastAPI's Swagger page,
  which went with the Python server. The document is now checked in and
  hand-maintained, so treat a surprising entry as possible drift.)
- `scripts/external_microservice_example.py` — runnable orchestration sample
- `scripts/model_ops_client_example.py` — runnable model build sample

## Model build / train

Agent Platform owns LoRA training and Ollama deployment. External apps upload knowledge and start jobs; they consume trained models via `/v1`.

```python
import os, time, httpx

BASE = os.environ["AGENT_PLATFORM_BASE_URL"]
TOKEN = os.environ["AGENT_PLATFORM_TOKEN"]
H = {"Authorization": f"Bearer {TOKEN}"}

with httpx.Client(base_url=BASE, headers=H, timeout=120) as c:
    c.post("/api/v1/model-ops/projects", json={"name": "my-coach", "ollama_tag": "my-coach"})
    job = c.post("/api/v1/model-ops/jobs", json={
        "project": "my-coach",
        "stages": ["prepare", "export", "eval"],
        "offline_eval": True,
    }).json()
    while job["status"] in ("pending", "running"):
        time.sleep(2)
        job = c.get(f"/api/v1/model-ops/jobs/{job['id']}").json()
    print("job:", job["status"], job.get("result"))
    # Chat with the new model via embedded proxy:
    # POST /v1/chat/completions with model=my-coach
```
