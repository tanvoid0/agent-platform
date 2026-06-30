# Agent workspace routes (canonical)

| Route | Purpose | Backend |
|-------|---------|---------|
| `/app/` | Redirect to active project workspace or project list | — |
| `/app/projects/:projectId` | Agentic AI workspace (3D, kanban, inspector) for one project | `GET/PATCH /projects/{id}`, project-scoped state |
| `/app/projects` | Projects list / CRUD | `GET/POST/PATCH/DELETE /projects` |
| `/app/teams` | Team templates | `GET/POST/PATCH/DELETE /teams` |
| `/app/finance`, `/app/finance/project` | Finance demo | Optional |
| `/app/settings/*` | Settings | LLM proxy / env |

Legacy bookmarks under `/flow/*` redirect to `/app/*` (308).

The archived Flow UI under `archive/flow-ui-legacy/` is not authoritative.
