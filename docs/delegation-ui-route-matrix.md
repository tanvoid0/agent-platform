# Delegation UI routes (canonical) vs agent-platform

The Flow UI under `archive/flow-ui-legacy/` is not authoritative for navigation.

| Delegation route | Purpose | Agent-platform backend phase |
|------------------|---------|------------------------------|
| `/flow/` | Main workspace (3D + kanban + inspector) | Process workspace, SSE, tasks (incremental wiring) |
| `/flow/projects` | Projects list / CRUD | `GET/POST/PATCH/DELETE /projects` |
| `/flow/teams` | Team templates | `GET/POST/PATCH/DELETE /teams` |
| `/flow/finance`, `/flow/finance/project` | Finance demo | Optional / out of scope for agent-platform API unless added later |
| `/flow/settings` | Settings | Orchestrator keys / env (adapt to agent-platform config) |

Redirects: legacy Flow bookmarks used `/flow/graph`, etc.; add redirects only if still linked from outside.
