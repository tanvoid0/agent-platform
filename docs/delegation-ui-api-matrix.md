# Screens → agent-platform API

| Operation | HTTP | Notes |
|-----------|------|--------|
| List processes | `GET /processes` | Query: `limit`, `project_id`, `unassigned_only` |
| Process detail | `GET /processes/{id}` | DAG, tasks, status |
| Create process | `POST /processes` | `goal`, `team_template_id`, `project_id`, `auto_approve` |
| Approve DAG | `POST /processes/{id}/approve` | `dag_json` |
| Cancel / retry / sync | `POST /processes/{id}/cancel` etc. | |
| Task retry / review | `POST .../tasks/{tid}/retry`, `.../review` | |
| Process events | `GET /processes/{id}/events` | Pagination `after_id` |
| Live stream | `GET /processes/{id}/stream` | SSE; pair with REST for actions |
| Teams CRUD | `/teams` | Roster JSON matches `team_schema` |
| Projects CRUD | `/projects` | Persisted project workspace state |
| Chat completion | `POST /api/v1/chat` | Stateless; not WebSocket |

**Live updates:** Process and project state use **SSE + REST** on Agent Platform (no separate WebSocket project channel).

**Status:** The Flow UI uses Agent Platform for projects, teams, processes, finance-style rollups from in-browser usage ledgers, and `POST /api/v1/chat` for orchestrator-backed chat. Full graph/approve flows remain available via `/ui`, `/docs`, or API clients; use `GET /processes` directly when you need a raw process list.
