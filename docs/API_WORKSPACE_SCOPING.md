# Workspace Scoping in Agent Platform API

> **Note:** This supersedes [API_PROJECT_SCOPING.md](./API_PROJECT_SCOPING.md) for auth and tenant isolation. Project scoping still applies *within* a workspace.

## Hierarchy

```
Workspace  (tenant — one token = one workspace)
  └── Project  (grouping for processes, files, assistant, todos)
        └── Process / files / chats
```

## Authentication

| Credential | Scope | Use case |
|------------|-------|----------|
| `AGENT_PLATFORM_MASTER_KEY` | All workspaces | Admin, dashboard, token minting |
| Workspace API token (`agp_…`) | One workspace | External microservices, Flow UI |

Mint workspace tokens at **`POST /api/v1/workspaces/{workspace_id}/api-tokens/`** (master key only).  
Resolve the caller's workspace with **`GET /api/v1/me/workspace`** (workspace token only).

## Isolation rules

1. A workspace token can access only projects where `project.workspace_id` matches the token's workspace.
2. Cross-workspace access returns **404** (not 403) to avoid leaking resource existence.
3. Master key bypasses workspace checks (`principal.workspace_id is None`).
4. Global team templates (`workspace_id IS NULL`) are visible to all tokens but read-only for workspace tokens.

## Key endpoints

| Endpoint | Purpose |
|----------|---------|
| `GET /api/v1/workspaces/` | List workspaces (master key) |
| `POST /api/v1/workspaces/` | Create workspace (master key) |
| `GET /api/v1/me/workspace` | Resolve token → workspace |
| `GET /api/v1/projects/` | List projects (filtered to token workspace) |
| `POST /api/v1/projects/` | Create project (requires `workspace_id`) |
| `GET /api/v1/workspaces/{id}/api-tokens/` | Manage tokens (master key) |

## File sandbox

Prefer the canonical path:

```
GET /api/v1/projects/{project_id}/files/list
```

Legacy alias (deprecated, one release):

```
GET /api/v1/projects/{project_id}/workspace/list
```

## Token management (deprecated alias)

```
POST /api/v1/projects/{project_id}/api-tokens/   →  use /workspaces/{workspace_id}/api-tokens/
```

Responses include a `Deprecation: true` header.

## See also

- [CLIENT_INTEGRATION.md](./CLIENT_INTEGRATION.md) — end-to-end setup guide
- [API_PROJECT_SCOPING.md](./API_PROJECT_SCOPING.md) — process query filters within a project
