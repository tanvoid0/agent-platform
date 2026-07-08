# Workspace Tenancy — Implementation Plan

## Goal

Introduce a **Workspace** tenant that sits above `Project`, so independent
microservices (and the Flow UI) each own one workspace, isolated from every
other. Each microservice authenticates with a single workspace-scoped token
(one line in its `.env`) and can reach only the projects, processes, files, and
chats inside its own workspace.

```
Workspace  (tenant = one microservice / one Flow UI deployment / one token)
  └── Project          (grouping, current entity)
        └── Process / TaskNode / EventLog
        └── file sandbox (project-{id}/ on disk)
        └── todos / assistant / coder / playground / chat
```

### Decisions (locked)

| Decision            | Choice                                                              |
| ------------------- | ------------------------------------------------------------------ |
| Token scope         | **Workspace-only.** One token = one workspace = all its projects.  |
| Tenant entity name  | **`Workspace`.** File-sandbox concept renamed to "sandbox".        |
| Flow UI auth        | One workspace token in `.env`; UI resolves workspace via `/me/workspace`. |

---

## Current state (what exists today)

Isolation boundary today is **`Project`**. There is no tenant above it.

- `ApiToken.project_id` — token scoped to ONE project (`app/models.py:115`).
- `assert_token_project_access(principal, project_id)` — 404 if a token touches
  another project (`app/api_tokens/auth.py:112`).
- Token management lives under `/projects/{project_id}/api-tokens`
  (`app/api_tokens/routes.py:22`), master-key only.
- File sandbox = `project-{id}/` directory (`app/workspace_service.py:47`).
- `Process.client_id` — weak logical namespace string, NOT a security boundary
  (`app/models.py:57`).

**Naming collision to resolve:** the word "workspace" is already overloaded —
`Project`'s docstring calls itself "workspace / folder", and
`workspace_service.py` is the on-disk file sandbox. Neither is a tenant. This
plan makes `Workspace` the tenant and renames the file-sandbox wording to
"sandbox".

---

## Step 1 — Model (`app/models.py`)

Add:

```python
class Workspace(SQLModel, table=True):
    __tablename__ = "workspace"
    id: Optional[int] = Field(default=None, primary_key=True)
    name: str = Field(max_length=256)
    slug: str = Field(max_length=128, unique=True, index=True)
    description: Optional[str] = Field(default=None, max_length=4096)
    created_at: datetime = Field(default_factory=datetime.utcnow)
    updated_at: datetime = Field(default_factory=datetime.utcnow)
```

Modify:

- `Project`: add
  `workspace_id: Optional[int] = Field(default=None, foreign_key="workspace.id", index=True)`
  (nullable at column level; enforced NOT NULL after backfill — see Step 2).
  Drop "workspace / folder" from docstring → "grouping within a workspace".
- `ApiToken`: add
  `workspace_id: int = Field(foreign_key="workspace.id", index=True)`.
  Keep the existing `project_id` column but make it **nullable** (retained for
  back-compat and optional future single-project tokens; not used for the
  workspace-only scoping model).

---

## Step 2 — Migration (new Alembic revision)

PostgreSQL is the live backend (head at time of writing). Order is deliberate so
rollback stays safe — **do NOT drop `api_tokens.project_id` in this revision.**

1. `CREATE TABLE workspace`.
2. Insert one seed row: `name="Default"`, `slug="default"`. Capture its id.
3. `ALTER TABLE project ADD COLUMN workspace_id INTEGER NULL` (+ FK, index).
4. Backfill: `UPDATE project SET workspace_id = <default_id> WHERE workspace_id IS NULL`.
5. `ALTER TABLE project ALTER COLUMN workspace_id SET NOT NULL`.
6. `ALTER TABLE api_tokens ADD COLUMN workspace_id INTEGER NULL` (+ FK, index).
7. Backfill from each token's project:
   `UPDATE api_tokens t SET workspace_id = p.workspace_id FROM project p WHERE t.project_id = p.id`.
8. `ALTER TABLE api_tokens ALTER COLUMN workspace_id SET NOT NULL`.
9. `ALTER TABLE api_tokens ALTER COLUMN project_id DROP NOT NULL`.

**Downgrade:** drop the two `workspace_id` columns, drop `workspace` table,
restore `api_tokens.project_id` NOT NULL.

**SQLite path:** mirror via the existing `process_table_sqlite`-style batch
patch (SQLite can't `ALTER ... SET NOT NULL` in place — use batch table rebuild
or leave nullable + enforce in code). Follow the pattern already in
`app/process_table_sqlite.py`.

---

## Step 3 — Auth (`app/api_tokens/auth.py`)

- `TokenPrincipal`: add `workspace_id: int | None` (None => master key /
  unrestricted).
- `verify_project_api_token`: return
  `TokenPrincipal(workspace_id=row.workspace_id, project_id=row.project_id, token_id=row.id, scopes=...)`.
  Master-key branch returns `workspace_id=None`.
- Add:

  ```python
  def assert_token_workspace_access(principal, workspace_id: int | None) -> None:
      if principal.workspace_id is None:      # master key
          return
      if workspace_id is None or principal.workspace_id != workspace_id:
          raise HTTPException(status_code=404, detail="Not found")
  ```

- Rewrite `assert_token_project_access(principal, project_id)`: master key
  bypasses; otherwise load the project's `workspace_id` and compare to
  `principal.workspace_id` (404 on mismatch or missing project). One extra query
  per call — acceptable; optionally cache project→workspace within the request.

---

## Step 4 — Routes

### 4a. New `/workspaces` admin router (master-key only)

New file `app/workspaces_routes.py`, prefix `/workspaces`. CRUD mirroring
`projects_routes.py`. Guard every endpoint with the same
`_require_dashboard_caller` pattern used in `api_tokens/routes.py` (reject
project/workspace-scoped principals — only the master key manages tenants).

### 4b. Move token management

`app/api_tokens/routes.py`: change prefix
`/projects/{project_id}/api-tokens` → `/workspaces/{workspace_id}/api-tokens`.

- `create_api_token`: set `workspace_id` from the path; drop the required
  `project_id`. Validate the workspace exists.
- `_require_token`: check `row.workspace_id == workspace_id` (was `project_id`).
- All handlers still master-key only.

### 4c. Project routes (`app/projects_routes.py`)

- `create_project`: accept + persist `workspace_id` (from path or body).
- `list_projects`: filter to the caller's workspace
  (`assert_token_workspace_access` + `WHERE workspace_id = ?`); master key sees
  all.
- `get/update/delete/{project_id}`: call `assert_token_project_access(principal,
  project_id)` — every one currently lacks `principal`; thread it in.

### 4d. Process / file / chat routes

These already call `assert_token_project_access`. The rewritten version (Step 3)
now enforces workspace membership automatically. **Audit** every project-scoped
route to confirm it passes `principal` into the check. Rename the file router
path (Step 5).

### 4e. Register routers (`app/main.py`)

Add `workspaces_router` to both the bare and `/api/v1` include blocks
(`app/main.py:70-89`), with `dependencies=_api_deps`.

### 4f. `/me/workspace` (Flow UI convenience)

Add `GET /me/workspace` returning the workspace resolved from the caller's
token (`principal.workspace_id`), so the Flow UI needs only the token in `.env`
— no workspace id baked into config. Master key → 400 or a selectable list.

---

## Step 5 — Naming de-collision

- Rename `workspace_service.py` internals to "sandbox" wording
  (`project_sandbox_dir` already good; scrub "workspace" from docstrings and
  helper names). Keep the public function API stable where imported.
- Rename the file HTTP router path
  `/projects/{id}/workspace` → `/projects/{id}/files`
  (`app/workspace_routes.py`) to avoid confusion with the new tenant
  `/workspaces`. Keep the old path as a deprecated alias for one release if
  external callers exist.
- `Project` docstring: "grouping within a workspace" (drop "workspace / folder").

---

## Step 6 — Flow UI (`web/` in this repo)

- `.env`: single `AGENT_PLATFORM_TOKEN=agp_...` (a workspace token).
- On boot, call `GET /me/workspace` to learn the workspace id + name; scope all
  subsequent calls to it. No workspace id stored in `.env`.
- Deploying a second Flow UI instance for another tenant = swap the token only.

---

## Step 7 — Tests

- `app/tests/test_workspace_tenancy.py` (new):
  - Token for workspace A gets 404 on a workspace B project.
  - `list_projects` returns only the caller's workspace projects.
  - Master key sees all workspaces/projects.
  - `create_api_token` binds to the path workspace.
  - `/me/workspace` returns the token's workspace.
- Update existing suites for the moved prefixes + new `workspace_id`:
  `test_api_tokens*`, `test_standalone_api.py`, `test_projects_api.py`,
  `test_workspace_service.py` (renamed concepts).
- Migration test: fresh DB → Default workspace created, all projects + tokens
  backfilled to it.

---

## Step 8 — Documentation (client-facing) — REQUIRED

Any change that moves a route, adds `workspace_id`, or changes the token model
MUST update the docs below in the same PR. Clients follow these to integrate —
stale docs = broken integrations. Do not mark the feature done until every item
here reflects the workspace structure.

Update:

- `docs/API_PROJECT_SCOPING.md` — rewrite for workspace scoping. New title
  (e.g. `API_WORKSPACE_SCOPING.md`); explain Workspace → Project hierarchy, that
  a token is workspace-scoped, and the 404-isolation behavior across workspaces.
  Keep a short "renamed from project scoping" note or redirect stub.
- `app/templates/api_guide.html` — update every example endpoint
  (`/projects/...` → `/workspaces/...` for token mgmt; add `workspace_id`;
  document `GET /me/workspace`). This is the in-app guide clients read.
- `app/templates/tokens.html` — update the token create/list UI copy + any
  hardcoded paths to the `/workspaces/{id}/api-tokens` prefix.
- `app/templates/config.html` — update if it references token/project setup.
- `README.md` — update the API/auth section: one workspace token per
  microservice, `.env` usage, quickstart curl examples pointed at the new paths.
- `docs/action-orchestrator-api.md` and `docs/delegation-ui-route-matrix.md` —
  audit for `/projects/...` paths that moved or now require workspace context;
  update route tables.
- **New** `docs/CLIENT_INTEGRATION.md` — single canonical guide a new client
  follows end to end: obtain a workspace token, put it in `.env`, resolve
  workspace via `/me/workspace`, list/create projects, run processes, use files.
  Include copy-paste curl + a minimal code snippet. Link it from README.

Verification: grep the repo for `/projects/{project_id}/api-tokens` and other
moved paths — zero stale hits in docs/templates before merge.

---

## Rollout order

1. Model + migration (Steps 1–2) — ship, verify backfill on staging.
2. Auth + `/workspaces` admin + moved token routes (Steps 3, 4a, 4b).
3. Project/process/file route scoping + `/me/workspace` (Steps 4c–4f).
4. Naming de-collision (Step 5).
5. Flow UI wiring (Step 6).
6. **Docs update (Step 8) — ship in the same PR as the route/model changes, not
   after.** Merge blocked until client docs reflect the workspace structure.
7. After one stable release: drop `api_tokens.project_id` and any deprecated
   path aliases.

## Resolved decisions (were open items)

- **`api_tokens.project_id`** — **keep nullable, unused** for now. Zero-risk to
  retain; enables optional single-project tokens later without a second
  migration. Revisit at the "drop" step in Rollout only if it stays unused for a
  full release.
- **Old `/projects/.../api-tokens` path** — **keep a deprecated alias for one
  release.** These endpoints are master-key only (`_require_dashboard_caller`,
  `app/api_tokens/routes.py:25`), so the only caller is the dashboard/admin —
  low blast radius, but a one-release alias avoids breaking any scripted admin
  tooling. Emit a `Deprecation` header; remove at Rollout step 7.
- **`/projects/{id}/workspace` → `/files` rename** — **keep alias one release.**
  This path is data-plane (used by Flow UI + any client browsing the sandbox),
  higher blast radius than token mgmt. Old path proxies to the new handler,
  logs a deprecation warning, removed at Rollout step 7 alongside the token
  alias.

---

## Risks / gotchas

- **Backfill correctness (Step 2, item 7).** The `api_tokens` backfill joins
  through `project`. Any token whose `project_id` is NULL or dangling backfills
  to NULL `workspace_id`, then the `SET NOT NULL` (item 8) fails. Pre-check:
  `SELECT count(*) FROM api_tokens WHERE project_id IS NULL OR project_id NOT IN
  (SELECT id FROM project)` — must be 0, or assign those to the Default
  workspace before item 8.
- **`assert_token_project_access` extra query.** Rewritten check (Step 3) loads
  project→workspace per call. Hot process/file routes call it every request —
  cache the mapping on the request state (or a short TTL) to avoid an N+1 under
  load. Flagged as "acceptable" in Step 3, but measure before shipping 4d.
- **Master key = `workspace_id None`.** Every new guard must treat None as
  bypass (Step 3). A missed `None` check turns the master key into a
  locked-out principal or, worse, denies all — audit each call site added in 4c.
- **SQLite divergence (Step 2).** SQLite can't `ALTER ... SET NOT NULL`; the
  batch-rebuild path leaves columns nullable and enforces NOT NULL only in code.
  Tests must run against both backends or the SQLite path silently permits null
  tenants.
- **Two aliased paths at once.** Both the token-mgmt and file-sandbox renames
  keep aliases for one release — track both in a single "remove aliases" ticket
  so Rollout step 7 doesn't drop one and forget the other.
- **Docs drift is a merge blocker (Step 8), not follow-up.** The grep gate
  (`/projects/{project_id}/api-tokens` → zero stale hits) must run in CI or a
  pre-merge check, else stale client docs ship silently.
