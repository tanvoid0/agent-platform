# Web API Contract Coverage

This document defines the web-facing API responses that must remain compatible for the in-repo UI and other thin clients.

The contract suite is the pytest `contract` marker. `pnpm smoke` runs that suite through `python scripts/smoke_workflow.py`, so CI can fail fast on response-shape drift.

## Selected endpoints

These routes were chosen because a shape change here would break the current UI flow quickly and visibly:

1. `GET /api/v1/llm/ui-catalog`
2. `GET /v1/catalog`
3. `POST /api/v1/chat`
4. `GET /api/v1/chat/resolved-defaults`
5. `GET /teams/` and `GET /api/v1/teams/`
6. `POST /teams/`
7. `POST /processes` and `POST /api/v1/processes`
8. `GET /api/v1/todos/boards`
9. `GET /api/v1/todos/boards/{id}`
10. `GET /workspaces/`, `GET /me/workspace`, and workspace-scoped `GET /projects/`

## Compatibility expectations

The tests intentionally assert stable top-level keys and representative nested fields instead of every database field.

- LLM catalog routes return object containers with `providers` plus resolved/default model fields used by the config and test UIs.
- Chat routes return OpenAI-style `choices[*].message.content` data and fail closed when proxy auth is unavailable.
- Team list responses return a `teams` array whose items expose identity, display metadata, and roster-derived fields such as `role_count`.
- Team create/detail responses preserve nested `roster.roles[*]` shape including parent/child and accent-color fields used by the editor UI.
- Process creation returns a minimal stable envelope with `process_id` and `status`.
- Todo board list responses return a `boards` array. Board detail responses include `categories` and `items`.
- Workspace and project tenancy routes preserve the distinction between `401`, `403`, and `404` so clients can tell auth failure from cross-workspace isolation.
- Workspace identity responses expose `id`, `name`, and `slug`, which clients use to bind tenant-scoped state.

## Representative error contracts

The suite also covers the error responses most likely to affect the UI:

- Missing or wrong Bearer auth for protected routes returns `401`.
- Workspace tokens calling admin-only workspace routes return `403`.
- Cross-workspace reads and writes return `404` to preserve tenant isolation.
- Unknown provider selection for `GET /v1/catalog` returns `400`.
- Invalid todo status updates return `400`.
- Invalid team roster payloads return `422`.

## Test files

The current contract suite lives in:

- `app/tests/test_standalone_api.py`
- `app/tests/test_v1_catalog.py`
- `app/tests/test_teams_api.py`
- `app/tests/test_todos_api.py`
- `app/tests/test_workspace_tenancy.py`
