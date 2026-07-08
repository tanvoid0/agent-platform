# Agent Platform — continuity plan

Handoff for the **agent-platform** package. Broader brainstorm may live under `.cursor/plans/` at the workspace root.

## What it is

FastAPI service that orchestrates multi-agent **processes** (DAG runs) via an **embedded** OpenAI-compatible LLM proxy (`app/llm_proxy/`, same process).

**Loop:** goal → planner DAG → **`approval_required`** → human approves/edits → topological execution → terminal state. Optional **`requires_review`** per task pauses until **`POST .../processes/{id}/tasks/{task_id}/review`**.

## Ports and URLs

| Surface | URL |
|--------|-----|
| HTTP API | `http://127.0.0.1:18410` — **`/processes/...`** (same routes also under **`/api/v1/processes/...`**). Team templates: **`/teams`** and **`/api/v1/teams`**. |
| React app (Vite, `base: "/app/"`) | **`/app/`** — built to `app/static/dist`, served under **`/app`**. Client routes live under `/app/...` (e.g. `/app/projects/:id` workspace). |
| Jinja shell | **`/ui`** — lightweight HTML host; **`/`** redirects to **`/ui`**. |
| Embedded LLM proxy (`/v1`) | same host as API — default `http://127.0.0.1:18410/v1` |

OpenAPI is available from the FastAPI app on the same port (paths depend on router mounts above).

## Where to edit

| Area | Path |
|------|------|
| App, mounts, static | `app/main.py` |
| Process REST, SSE, events | `app/process_routes.py` |
| DB, Alembic | `app/database.py`, `app/alembic/` |
| Models | `app/models.py` |
| Planner, executor, LLM | `app/orchestrator.py`, `app/llm_client.py`, `app/dag_schema.py` |
| Team templates | `app/teams_routes.py`, `app/team_schema.py`, `web/src/components/TeamsPage.tsx` |
| Approve / retry / auto-approve | `app/process_approval.py` |
| Tools, MCP client | `app/tools_policy.py`, `app/tool_handlers.py`, `app/mcp_streamable_client.py` |
| Frontend | `web/src/` (in-repo React workspace; TanStack Query, `@xyflow/react`; entry under **`/app/`**) |
| ADRs | `docs/adr/` |

## ADRs

1. `0001` — orchestration FSM, HTTP-first, SQLite.
2. `0002` — React + Vite + TanStack Query + `@xyflow/react` (app served at **`/app/`**).
3. `0003` — **future** 3D/R3F boundary (lazy load, server authority); see **Visual layer** below.

## Visual layer: pixel art first, 3D later

- **Near term:** Treat process/task state as a **2D pixel-art** view (sprites, tilemap, or canvas/CSS grid “chibi” agents) — much lighter than a full 3D sim. Same rule as ADR 0003: **server is authoritative**; the client only renders state from API/poll/SSE.
- **Later:** Optional **3D** (R3F, etc.) remains planned per `0003`; spike only after pixel art proves useful and orchestration stays stable. Do not block orchestration on either.
- **UI (current):** Pixel art and the lazy **3D boundary spike** are **hidden by default**. Process controls expose a single **Optional visualization** selector: **Off** (default) | **Pixel preview** | **3D boundary spike** — at most one; the pixel office under a loaded process stays **collapsed until** the user expands **Pixel office** (disabled while 3D spike is selected).

## Runbook

```bash
cd agent-platform
cp .env.example .env   # set AGENT_PLATFORM_MASTER_KEY (Bearer for /v1)
pip install -r app/requirements.txt
cd web && pnpm install && pnpm run build && cd ..
uvicorn main:app --app-dir app --host 127.0.0.1 --port 18410
```

- Migrations: `alembic upgrade head` on startup; new rev: `cd app && alembic revision --autogenerate -m "msg"`.
- **SSE:** `GET /processes/{id}/stream` tails new `EventLog` rows (~0.8s poll loop). The stream **closes** on terminal status (`completed` / `failed` / `cancelled`) or when the process needs a human gate (`approval_required`, `task_review_required`) after emitting a **`terminal`** JSON event — clients must refresh via **`GET /processes/{id}`** and approve/review as needed. See **`/api-guide`** (`app/templates/api_guide.html`).
- Web dev: `pnpm run dev` in `web/` with proxy to 18410; ship with `pnpm run build`.
- Optional: **pixel-agents raster assets** (MIT) for the full pixel office: run `python scripts/sync_pixel_agents_assets.py` from the repo root (downloads upstream `webview-ui/public/assets` into `web/public/pixel-agents/`), or pass `--source /path/to/pixel-agents` if you already have a clone. Committed PNGs in-repo match `web/public/pixel-agents/NOTICE.txt`.
- Optional: seed default team templates into an existing DB if `teamtemplate` is empty: `python scripts/seed_team_template_once.py` (expects `data/agent_platform.db`; see script).
- Tests: `pip install -r requirements-dev.txt` → `pytest`; `cd web && pnpm run test` (Vitest).

## Implemented (summary)

REST: processes list/create/detail, approve, cancel, retry, task review, **`GET /processes/{id}/events`** (filterable; `limit` capped server-side, `after_id` for pagination), **`GET /processes/{id}/stream`** (SSE). Duplicate **`/api/v1/...`** paths for the same routers. Statuses: process (`planning` … `completed`/`failed`/`cancelled`); task (`pending`, `running`, `awaiting_review`, …). Planner JSON validated (Pydantic, acyclicity) on plan and approve. Task review + sub-DAG expansion after approval; failure fields, `total_cost`, tool invocations, optional timeouts and auto-approve. SQLite + Alembic (legacy stamp path). **Web UI (`/app/`):** graph/board/timeline/**events**, inspector, approve DAG, cancel, retry, review mutations, recent processes, 2D polish (edges, minimap, status visuals), **graph lineage** (`parent_client_uuid`: depth layout, tint, parent hint, visibility **All / ≤1 / Roots**). **Export JSON** in `ProcessMainPane` (`downloadProcessExport` → process + tasks + **all** events via paginated fetch). Tools: echo, http_fetch, MCP streamable, optional nested chat; planner/expansion retries and subdecompose env knobs. Tests under `app/tests/` and `flow-ui/src/**/*.test.ts`.

## Team templates page — Delegation as reference (not a port)

**Today:** **Teams** (`TeamsPage` route) is `web/src/components/TeamsPage.tsx`: library + editor backed by **`GET/POST/PATCH/DELETE`** on **`/teams`** (and **`/api/v1/teams`**) — `app/teams_routes.py`, `team_schema.py`, DB templates. Roster is JSON (**roles** with `id`, `name`, `description`, optional `parent_id`, `default_model`, optional **`accent_color`**) for planner / LLM alias use — no simulation state.

**Reference UX (The Delegation):** `the-delegation/src/interface/TeamManagementPage.tsx` → **`VisualConfigurator`** (`VisualConfigurator.tsx`): React Flow roster graph, **`TeamsPanel`** (library + “Create New Team”), **`VisualFlowNode`** (border + **`Avatar`** + LEAD/SUB tags + model line), **`TeamCard`** density. Avatars live in **`the-delegation/src/interface/components/Avatar.tsx`** (`user` | `lead` | `sub`, tint `color`). Brand tints: **`the-delegation/src/theme/brand.ts`** (`USER_COLOR`, etc.). Our port stays **lean**: reuse **visual patterns** (palette, node chrome, tags), not `AgenticSystem` or client-only persistence.

**Incremental upgrades (ordered):**

1. **Roster chrome (done / in flight):** optional per-role **`accent_color`** in API schema; roster map uses a **fixed palette** (Delegation-like blues/greens/purples/yellows/reds) when unset. Custom Flow node: avatar strip, LEAD vs SUB badge (first root = lead), monospace model hint.
2. **Library cards:** compact list; **category** (optional API field) + **agent count** on cards; **Filter by category** in the library sidebar; subtitle from **description**.
3. **Starter templates:** ship **preset JSON** in the web app (`teamTemplatePresets.ts`) aligned with **proxy model aliases** (e.g. `gemini-flash`, `local` — not vendor-specific IDs unless documented in `.env.example`). **Apply preset** fills the editor (new template or confirm overwrite). Optional DB seed script: `scripts/seed_team_template_once.py`.
4. **Not in scope (short term):** human-in-the-loop toggles, capabilities checklist, separate “User (You)” node — those belong to a richer agent model than `RosterRole` today.

**Deep links:** `?team=<id>` / `?team=new` + **Copy link** (see `web/src/lib/teamUrl.ts`).

## Backlog (product)

See **`docs/practical-assistant-roadmap.md`** for the phased plan toward a practical multi-agent daily planning assistant (domain templates, interactive forms, server authority).

- **Project sub-groups:** `Project` is a flat folder today (no `parent_id`, `category`, or tags). For career workflows, consider nested groups (e.g. *Job search 2026* → *Acme SWE*, *Beta PM*) with list/filter in `/app/projects` and optional default `project_id` on new processes. Schema: `parent_project_id` + `category` (or tags JSON); API + UI filters mirror team template **category** chips.
- **Document routing (later):** per-model native PDF/vision vs derived markdown (capability flags on providers); optional page PNGs for vision models.

## Documents (implemented)

- **`POST /api/v1/projects/{id}/workspace/upload`** — multipart PDF / `.txt` / `.md`; PDF → `pymupdf` extract to `<path>.derived/structured.md` + `manifest.json`.
- **`GET .../workspace/file`** — PDF paths return derived markdown (`content_kind: pdf_derived_markdown`).
- **DAG `workspace_read`** — same derived content for PDFs when tools are enabled.
- **Chat** — paperclip on `ChatComposer`; requires server project for PDFs; text can inline without project. Content is injected into the user turn for agents.
- **Dependency:** `pymupdf` in `app/requirements.txt`.

## Next steps

**Status:** The numbered checklist below is **complete**. Optional backlog: **deeper 3D/sim** if the product asks; **raster strip** — shared RAF across tiles (**done** via `PixelStripRafProvider` + strip subscription in `PixelRasterChibiTile`); further polish (e.g. more animation frames) if desired. Strip raster tiles already **pause animation** when the document is hidden (`visibilitychange`).

1. ~~**Board tab**~~ — **Done:** toolbar in `TaskBoardView` (search, **Needs attention**, counts, clear filters) + column chrome.
2. ~~**Processes list**~~ — **Done:** card grid, status chips, relative `created_at`, selected highlight (`RecentProcessesList` / equivalent).
3. ~~**Event log in web app**~~ — **Done:** `GET /processes/{id}/events`, **Events** tab, filters + task-linked selection (`ProcessEventsView`).
4. ~~**Graph / `parent_client_uuid`**~~ — **Done:** `DagGraphView` + `dagGraphLayout.ts` — layered positions by depth, primary tint, `↑ parent role` in label, edges respect visible set; **All / ≤1 / Roots** filters.
5. **Teams page (template UX)** — **Done:** library + editor, **`?team=` / `?team=new`**, **Copy link**, **`TeamRosterGraph`**, **`accent_color`**, **starter presets**. Library cards: **`#id`**, relative **Updated**, **role count**, optional **category** chip when set (`TeamTemplateCard`).
6. ~~**Pixel art slice**~~ — **Done:** **`PixelProcessStrip`** (also exported as **`PixelRunStrip`**) — live task tiles + optional pixel office; **`TASK_STATUS_COLORS`** via `taskStatusColor`; subtle pulse on a subset of **running** tiles. **`PixelHomeTeaser`** when **Pixel preview** is on and no process is loaded. Optional viz **off by default**.
7. ~~**Export (JSON)**~~ — **Done:** **Export JSON** in `ProcessMainPane`; **`downloadProcessExport`** paginates events (server `limit` max 2000 per request) so large logs export in full.
8. ~~**3D boundary spike (ADR 0003)**~~ — **Done:** lazy **`SimulationSpike`** (`web/src/features/simulation/SimulationSpike.tsx`): R3F + Three + `OrbitControls`; rotating box tint from read-only **`GET /processes/:id`** status; heavy deps stay out of the main graph bundle. ~~SSE semantics~~ — **Done:** documented in **`plan.md`** (runbook) and **`/api-guide`**.
9. ~~**Raster sprite sheets (office)**~~ — **Done:** MIT assets from [pixel-agents](https://github.com/pablodelucca/pixel-agents) ship under `web/public/pixel-agents/` (characters, floors, walls, furniture); refresh via `scripts/sync_pixel_agents_assets.py`. Task strip defaults to **`PixelChibiTile`** (CSS); **optional raster tiles** use the same character sheets as the office (`PixelRasterChibiTile`), with **Strip task tiles** under Process controls (persisted). Falls back to CSS if assets fail to load.

## Notes

- Workspace **`AGENTS.md`**: agent-platform **18410** (API + embedded LLM proxy); UI at **`/ui`**, **`/app`**; REST uses **`processes`** / **`teams`** (and **`/api/v1/...`** mirrors). Orchestration before heavy visual coupling.

---

*Handoff: 2026-04-12 — **Docs** aligned with Process/`/flow`/`/api/v1` reality; **export** paginated; **SSE** behavior documented in runbook + api guide; **team templates** support optional **category** (API + UI). **Board** cards: hover lift/shadow, focus ring, depth badge (`d{n}`), instruction tooltip. **Pixel office:** raster PNGs vendored + **`sync_pixel_agents_assets.py`**; strip **CSS or optional raster** (`PixelRasterChibiTile`, persisted preference; **visibility** pauses strip RAF when the tab is hidden). **Raster strip:** one shared RAF loop for all tiles when **Strip task tiles** = raster (`PixelStripRafContext`). **3D:** lazy R3F spike (`SimulationSpike`). **Optional next:** deeper 3D/sim; more sprite frames if needed.*
