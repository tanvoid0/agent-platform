# Client operations → agent-platform API

> **All paths are under `/api/v1`.** The bare-root mirror (`/processes`, `/teams`, …) was removed with the browser UI; paths written without the prefix below are relative to it.
>
> Written for the Flow UI, which was consolidated into `web/` and then deleted by the native-desktop migration ([ADR 0005](./adr/0005-native-iced-desktop-headless-server.md)). **The table below is still current** — these are the API operations any client uses, and the native app in `desktop/crates/client/` calls the same ones.

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

**Status:** The server ships no browser UI beyond the `/tokens` dashboard — there is no `/ui` mount. Graph and approve flows live in the native desktop app; anything else reaches them through `/docs` (OpenAPI) or a direct API client. Use `GET /processes` when you need a raw process list.
