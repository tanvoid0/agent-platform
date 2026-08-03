# Desktop app — merged information architecture

One app. Three surfaces collapse into it:

| Was | Where it lived | Becomes |
|-----|----------------|---------|
| Server UI | Jinja `/config`, `/tokens`, `/api-guide`, `/ui` | Settings, Bench, Docs |
| Flow UI | `web/` served at `/app/`, wrapped by the desktop webview | The app itself, bundled natively |
| flow-ui | `flow-ui/` (stale since 2026-07-02, 44k lines) | Rebuilt thin inside `web/`; tree deleted |

The backend keeps doing the heavy lifting. The desktop app runs it, watches it, edits its data,
and makes the `/v1` proxy easy to point an external app at.

## Native, not a wrapper

Today the shell opens a webview onto `http://127.0.0.1:PORT/app/` — the server serves the UI. That
is a wrapper: no bundle, no offline window, nothing on screen when the server is down, and no
Tauri APIs because the page is a remote origin.

Target: the UI is bundled into the app (`tauri://` asset protocol) and the server is a plain HTTP
sidecar it talks to.

| Concern | Consequence of going native |
|---|---|
| Asset base | `web/` builds twice: `base: "/app/"` for server-served, `base: "./"` + `HashRouter` for the bundle. One `VITE_DESKTOP` flag picks the mode. |
| Origin | Webview is `tauri://localhost` (Windows: `http://tauri.localhost`), API is `http://127.0.0.1:PORT`. Cross-origin. The shell passes `CORS_ALLOW_ORIGINS` to the server explicitly instead of relying on the `*` default. |
| Auth | Unchanged: per-install key injected as `window.__AGENT_PLATFORM__.masterKey`, already honoured by `api/client.ts`. |
| SSE | `EventSource` cannot carry `Authorization`, so it 401s against the authenticated `/processes/{id}/stream` — a bug that exists today and only survives because the failure is a silent reconnect loop. Replaced with a fetch-based SSE reader that sends the header. |
| Window with no server | The shell renders before `/health` answers, so startup, migration and crash output are visible in-app instead of a "cannot reach this page" page. |
| Now possible | Native file dialogs, completion notifications, window state, and the server's stdout piped into the Logs view. |

## Rail

Eight entries. Anything narrower goes in tabs inside a page, not in the rail.

| Rail | Route | What it is | Backend |
|------|-------|-----------|---------|
| **Status** | `/` | Server up/ready, port, data dir, provider reachability, active runs, quick actions (restart, open data dir, copy key) | `/health`, `/ready`, `/api/v1/system/status` |
| **Runs** | `/runs` | Process list + DAG / board / timeline / events. Monitoring. | `/processes`, `/processes/{id}/stream` |
| **Logs** | `/logs` | Dev log: level / logger / request-id filter, live tail | Tauri stdout ring in desktop; `/api/v1/logs` elsewhere |
| **Studio** | `/studio` | Projects · Teams · Models. Maintaining backend data. | `/projects`, `/teams`, `/api/v1/model-ops/*` |
| **Work** | `/work` | Assistant · Coder · Todos · Chat | `/api/v1/{assistant,coder,todos,chat}` |
| **Bench** | `/bench` | Point an external app at this server: provider/model picker, BYOK header builder, request/response with latency and token counts, token issue/revoke | `/v1/*`, `/api/v1/api-tokens/*`, `/v1/capabilities` |
| **Settings** | `/settings` | Providers, models, API keys, `config.yaml`, scene/assets, desktop prefs | `/api/v1/llm-proxy/*` |
| **Docs** | `/docs` | API guide, OpenAPI, client integration | `/openapi.json` |

Demos (finance, 3D scene) sit behind a Settings pref and only then appear as a ninth entry.

## Phases

| Phase | Delivers | Deletes |
|-------|----------|---------|
| P0 | Native bundle + shell/rail + Status page + `/api/v1/system/status` | — |
| P1 | Dev log: Rust stdout ring + `/api/v1/logs` + Logs page | — |
| P2 | Settings | `templates/config.html`, `GET /config` |
| P3 | Docs | `templates/api_guide.html`, `GET /api-guide` |
| P4 | Bench | `templates/tokens.html`, `GET /tokens` |
| P5 | Studio (fold existing pages, keep deep links) | — |
| P6 | Work, rebuilt thin against the existing routers | — |
| P7 | Demos behind a pref | `flow-ui/`, `templates/index.html`, `GET /ui` |

Each phase leaves the server-served `/app/` build working, so Docker and browser use never
regress while the desktop bundle grows.

## Rebuilt, not copied

`flow-ui` carries its own zustand stores, `integration/` layer and theme system. `web/` already has
shadcn, TanStack Query and `api/client.ts`. Porting the code would mean two of each. The phases
above re-derive the screens against `web/`'s existing client, lifting only files that are already
lean and dependency-free.
