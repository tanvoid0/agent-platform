# Agent workspace routes (canonical)

> **All paths are under `/api/v1`.** The bare-root mirror (`/processes`, `/teams`, …) was removed with the browser UI; paths written without the prefix below are relative to it.

| Route | Purpose | Backend |
|-------|---------|---------|
| `/app/` | Redirect to active project workspace or project list | — |
| `/app/projects/:projectId` | Agentic AI workspace (3D, kanban, inspector) for one project | `GET/PATCH /projects/{id}`, project-scoped state |
| `/app/projects` | Projects list / CRUD | `GET/POST/PATCH/DELETE /projects` |
| `/app/teams` | Team templates | `GET/POST/PATCH/DELETE /teams` |
| `/app/finance`, `/app/finance/project` | Finance demo | Optional |
| `/app/settings/*` | Settings | LLM proxy / env |

Legacy bookmarks under `/flow/*` redirect to `/app/*` (308).
