# Client Integration Guide

End-to-end setup for an external microservice or Flow UI deployment talking to Agent Platform.

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
- `/api-guide` — in-app HTTP guide
- `scripts/external_microservice_example.py` — runnable orchestration sample
