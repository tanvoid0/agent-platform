# Agent workspace routes — historical

> **Superseded 2026-08-04. Nothing serves these routes.** They were the browser
> routes of the `web/` Flow UI, deleted by the native-desktop migration
> ([ADR 0005](./adr/0005-native-iced-desktop-headless-server.md)). The server now
> exposes only `/api/v1/*`, `/v1/*`, `/tokens`, `/docs`, `/health` and `/ready` —
> there is no `/app/*`, no `/flow/*`, and no `/flow/* → /app/*` redirect.
>
> Kept as a map from the old routes to the native screens that replaced them, for
> anyone reading commits or issues written before the migration.

| Old browser route | Purpose | Replaced by |
|-------|---------|---------|
| `/app/` | Redirect to active project workspace or project list | `Screen::Dashboard` |
| `/app/projects/:projectId` | Agentic AI workspace (3D, kanban, inspector) for one project | `Screen::Processes` (`processes.rs`). The 3D/pixel office was dropped, not ported — see [native-desktop-migration.md](./native-desktop-migration.md). |
| `/app/projects` | Projects list / CRUD | `Screen::Projects` (`library.rs`) |
| `/app/teams` | Team templates | `Screen::Teams` (`library.rs`) |
| `/app/finance`, `/app/finance/project` | Finance demo | Dropped, not ported |
| `/app/settings/*` | Settings | `Screen::Settings` (`screen.rs`) |

The API operations these screens called are unchanged and still documented in
[delegation-ui-api-matrix.md](./delegation-ui-api-matrix.md).
