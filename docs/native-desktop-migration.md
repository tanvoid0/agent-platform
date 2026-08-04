# Native desktop migration

Replacing the Tauri + WebView2 shell with a native Rust UI (iced), keeping the
existing Python server as a child process.

**Status:** Complete on Windows. Phases 0–5 done (visual pass, pixel office
dropped, old stacks and server UI-serving deleted, file dialogs + notifications
wired), the three unported web features landed 2026-08-03, and the Windows
installer was compiled and round-tripped 2026-08-04. Only macOS/Linux
packaging/signing remains, deliberately deferred for lack of platform access —
see [Still open](#still-open).

## Why

The Tauri shell renders `web/` in WebView2 — a browser tab in a window frame.
That was cheap to build (see [ADR 0004](adr/0004-desktop-shell-tauri-python-sidecar.md))
but it means shipping two UI stacks, two dependency trees, and a build that
needs Node to produce anything a user can run. The native app draws its own
widgets, so the desktop build stops depending on `web/` entirely.

What does **not** change: the Python server. It is not forked, wrapped, or
reimplemented. Everything desktop-specific is still passed as environment at
spawn time, and Docker/cloud deploys are untouched.

## Layout

```
desktop/
  crates/client/    HTTP + SSE client, generated enums, DAG validation
  crates/app/       the iced application
```

`src-tauri/` (the legacy Tauri shell) and the in-repo `web/` Flow UI it wrapped
are deleted — Phase 5 dropped both.

`crates/app/src/`:

| File | What |
|---|---|
| `main.rs` | app wiring, screens, subscriptions, tray |
| `shell.rs` | server child process, port/key handling, settings, log ring |
| `ui/` | shadcn-derived widget kit and design tokens |
| `domain.rs` | board rows, waves, timeline rows, status→tone, relative time |
| `graph.rs` | DAG + roster layout math and the canvas |
| `processes.rs` / `_view.rs` | runs: composer, list, graph/board/timeline/events, review |
| `library.rs` / `_view.rs` | projects and teams |
| `modelops.rs` / `_view.rs` | model projects, build jobs, Ollama, registry |
| `chat.rs` / `_view.rs` | chat against the LLM proxy |

Screen modules are split state/update (`x.rs`) from rendering (`x_view.rs`), so
the logic is testable without a window.

## Done

### Phase 0 — spike
iced daemon mode, tray icon, SSE against the real server on Windows. Confirmed
the window can be hidden without killing the process, and that the tray survives
window close.

### Phase 1 — client crate
Typed API client over `reqwest`, SSE stream, DAG validation ported from the web
client so a bad plan reports field errors instead of a bare 422. Enum variants
are generated from the Python contract by `scripts/sync_contract_enums.py`, so
server-side status changes surface as Rust compile errors. 21 tests.

### Phase 2 — shell + Status/Logs
The app owns the Python server's lifetime: picks a free port, generates the
per-install key on first run, holds the child's stdin so the server dies with
the app. If a server is already listening it attaches instead — after verifying
the key, so it never adopts a stranger's process. Status and Logs screens,
light/dark theme following the OS.

### Phase 3 — Processes
Run composer (goal, team, project, auto-approve), run list, and the detail pane:
board, timeline, events, subagent inspector, review modal, and the approve /
cancel / retry / sync / retry-task actions. Detail polls at 800ms while a run is
live and 4s once settled; SSE frames trigger a refetch rather than being parsed
as state. The stream is only opened for live runs, matching the web hook — a
terminal run replaying a backlog closes without a sentinel and would otherwise
reconnect forever.

### Phase 4 — graph, catalogs, model ops, chat
- **Graph**: layered grid by lineage depth, dependency edges, status colors,
  click-to-inspect, drag-pan, scroll-zoom. The lineage filter (All / Depth ≤ 1 /
  Roots) hides itself when a run has no nesting.
- **Projects / Teams**: full CRUD. The team editor renders the roster as a live
  tree on the same canvas; removing a role re-roots its children.
- **Model ops**: model projects, stage-toggle build launcher, job polling with
  log tail, Ollama list + pull, adapter registry.
- **Chat**: single thread against the platform's LLM proxy.

Current test count: **66** (15 client unit + 7 client integration + 44 app).

## Remaining work — Phase 5

### 1. Visual pass on Phase 4 screens — done
Projects, Teams, Model ops and Chat checked in a running window. No overflow
or control-sizing issues — Processes needed that class of fix earlier, these
didn't. One stale leftover found and removed: the sidebar's "Open web UI"
link (`Message::OpenWebUi`, `shell::reveal_path` to `/app/`) pointed at the
now-deleted SPA mount; it predated Phase 4 completion ("unported screens live
there" was true then, isn't now) and was never cleaned up.

### 2. Pixel office — resolved: dropped
`web/src/features/pixel/` (chibi tiles, desk-seat layout, furniture manifest)
had no native equivalent and was decorative. Decision: drop it, not port it —
it went away with `web/` rather than being reimplemented in `iced::canvas`.

### 3. Delete the old stacks — done
- `desktop/src-tauri/` — the Tauri shell, its Rust, capabilities, and payload.
- `web/` — the whole React/Vite Flow UI, including the pixel and simulation
  features.
- `scripts/bundle_server.py` no longer stages `web/` into the payload.

### 4. Server cleanup — done
The server no longer mounts anything at `/app`. Docker collapsed to
backend-only — no nginx, no UI static serving, no `ui`/`all` container modes;
the image is just the FastAPI app behind uvicorn. Nothing else wanted a browser
UI, so `web/` was deleted outright rather than kept for Docker/cloud.

Second pass, once the native app reached parity (it is now the only UI, and it
talks to `/api/v1` exclusively):

- **Bare-root router mounts removed** — `/processes`, `/teams`, `/projects`,
  `/workspaces`, `/me/workspace`, `/workspace`, `/actions`, `/api-tokens` now
  answer only under `/api/v1`. **Breaking for any external caller still on the
  legacy paths: prefix them with `/api/v1`.** The 161 test call sites were
  migrated with them.
- **Deprecated project-scoped token routes removed** —
  `/api/v1/projects/{id}/api-tokens/*` is gone (master-key-only alias of the
  workspace routes, shipped one release with a `Deprecation` header). Use
  `/api/v1/workspaces/{workspace_id}/api-tokens/`.
- **CORS middleware deleted** (`LoggingCORSMiddleware`, `CORS_ALLOW_ORIGINS`) —
  a native client sends no `Origin`, and no browser page is served any more.
- **Jinja pages deleted**: `/config`, `/ui`, `/api-guide` and their templates.
  `/tokens` stays (the only token dashboard). `GET /` no longer redirects to
  `/config`; it returns `{"service", "api", "docs"}`.
- **`app/static/`** (6.7 MB of SPA/pixel assets) deleted — it was untracked and
  mounted by nothing after `web/` went away.
- `system/status` no longer reports `spa_bundled` (dropped from
  `client/src/types.rs` with it); `scripts/start.py` opens `/docs` instead of
  `/config`.

### 5. Packaging — Windows done, macOS/Linux deferred (no platform access)
- Bundle the Python runtime the same way the Tauri payload did (uv's managed
  CPython, relocatable) — done, `scripts/bundle_server.py` produces
  `desktop/payload/`.
- Icon: `desktop/crates/app/icon.ico` (16/32/48/256px), rasterized from
  `docs/brand/app-icon.svg`. No SVG rasterizer (ImageMagick/Inkscape/cairosvg)
  was available on the build machine, so it was redrawn with PIL primitives
  (the source SVG is just rounded rects and circles) rather than adding a new
  dependency — regenerate by re-running the shapes in a Pillow script if the
  source SVG changes shape, not just color.
- `desktop/crates/app/build.rs` embeds `icon.ico` into the exe via
  `winresource` (the maintained fork of the unmaintained `winres`), wired
  through the `[target.'cfg(windows)'.build-dependencies]` stanza in
  `crates/app/Cargo.toml`.
- Windows installer: `desktop/installer/agent-platform.iss` (Inno Setup).
  Per-user install under `%LOCALAPPDATA%\Programs\AgentPlatform` (no admin),
  Start Menu + optional desktop shortcut, installer/shortcut icon set from
  `icon.ico`, free uninstaller. `payload/` is installed as `<app>\server\`,
  matching what `shell.rs::resolve_server()` looks for next to the exe.
- `scripts/build_installer.py` orchestrates `cargo build --release` →
  `scripts/bundle_server.py` → `iscc desktop/installer/agent-platform.iss`,
  producing `dist/agent-platform-setup.exe`. It fails with a clear message if
  `iscc` is missing rather than skipping the step; since Inno Setup's own
  installer never adds it to PATH, the script also checks the default
  per-user and per-machine install dirs. **Verified end-to-end 2026-08-04**:
  clean `iscc` compile (48.6 MiB setup exe), then a silent install/uninstall
  round-trip — files land as `{app}\agent-platform.exe` + `{app}\server\`
  (matching `resolve_server()`), and uninstall leaves nothing behind.
- Signing: needs the developer's own code-signing certificate — this repo
  does not generate or ship a self-signed one, since that provides no trust
  benefit and would misrepresent the binary as verified. `scripts/build_installer.py`
  runs `signtool sign /f %AGENT_PLATFORM_SIGN_CERT% ... agent-platform.exe`
  only if `AGENT_PLATFORM_SIGN_CERT` is set in the environment; otherwise it
  prints a note and ships unsigned. Manual equivalent:
  `signtool sign /f cert.pfx /p <password> /t <timestamp-url> agent-platform.exe`.
- macOS and Linux: not implemented — deliberately deferred, no machine or CI
  runner for either platform is available. The Rust already branches on
  platform (`cfg(windows)` guards) but packaging, icon formats
  (`.icns`/`.desktop`), and signing/notarization (Apple notarization, Linux
  package signing) wait until access exists; a compile check on those
  platforms comes first, since this has only ever been built and run on
  Windows.

### 6. Native file dialogs and job-completion notifications — done
Model ops has an "Upload dataset file…" button that opens a native picker
(`rfd::AsyncFileDialog`) and uploads the chosen file into the project
workspace under `datasets/<filename>` via the new
`Client::upload_project_file` multipart call against
`POST /api/v1/model-ops/projects/{name}/files`. Processes (run completion)
and Model ops (build job completion) fire a native toast (`notify-rust`) the
first time a run/job transitions into a terminal status, gated by the same
"first time we saw terminal state" pattern already used for stream/poll
gating.

## Still open

Audited 2026-08-03 by diffing the deleted `web/src` tree against
`desktop/crates/app/src`. Everything the old Flow UI routed to has a native
screen; what follows is what did not come across, plus the leftovers the
deletion cut did not sweep up.

### Web features not ported

1. ~~**Process- and subagent-scoped chat.**~~ **Done.** "Ask about this" in the
   run's action row opens a chat card scoped to what is on screen: the
   inspected subagent if the inspector is open, otherwise the run.
   `State::scope_system()` rebuilds the context per send — process id, status,
   goal, failure reason, and for a subagent its role, task status and a
   3000-char output snippet — so a run that moved on is described as it is now.
   Threads are keyed `"<run id>"` / `"<run id>:<uuid>"` in
   `processes::State::chats`, in memory only, matching the web panel's
   sessionStorage semantics. The context rides ahead of the wire history and
   never appears in the transcript (`chat::State::system`); `chat_view::panel`
   is the shared transcript + composer, used by both the Chat screen (fills the
   window) and this card (capped at 280px inside the pane's own scroll).
2. ~~**Team template presets.**~~ **Done.** `library::TEAM_PRESETS` is the
   `teamTemplatePresets.ts` table as Rust consts — the same four rosters, same
   colors, text modality throughout. A "Start from a template" section under the
   Teams list fills the editor from one; nothing is created until it is saved.
   A test asserts every preset has one root and no dangling parent, since a role
   pointing at a missing parent silently drops out of the roster layout.
3. ~~**Process export.**~~ **Done.** "Export" in the run's action row opens a
   save dialog (`rfd`) and writes `{exported_at, process, tasks, events}`.
   Events are walked to the end through the server's `after_id` cursor —
   `Client::all_process_events`, 2000 per page with the same 500-page bound the
   web export used — so the file holds the whole run, not the page on screen.
   `process_events` gained the `after_id` argument this needs, and the record
   types gained `Serialize`.

Deliberately not ported: the pixel office (dropped, see above) and the
drag-resizable inspector with its persisted width (`processWorkspaceRail.ts`) —
decorative, and iced has no pane-splitter equivalent worth the code.

All three were verified against a live run (#1, 23 tasks) on 2026-08-03, not
just unit-tested: the scoped chat answered from the run's failure reason, the
export wrote 79 KB holding 23 tasks and 76 events with strictly increasing ids,
and a preset filled the team editor with its roster.

### Packaging

The Windows installer is closed: compiled by a real `iscc` and verified with a
silent install/uninstall round-trip on 2026-08-04 (see section 5).

macOS/Linux packaging is **deliberately deferred, not open work**: no macOS or
Linux machine (or CI runner) is currently available, and the app has never
compiled off Windows — a compile check comes before any packaging. Revisit
when access to either platform exists. Nothing else in the migration is open.

### Repo leftovers from the web era

The root `package.json` is not dead weight — `pnpm start`, `pnpm smoke` and
`pnpm docker:up` are the documented entrypoints in the README and every script
in it shells to Python. The one leftover — `package-lock.json` and
`pnpm-lock.yaml` both checked in for a single devDependency (`kill-port`) —
is resolved: `package-lock.json` was dropped, `pnpm-lock.yaml` stays (the
README's commands are pnpm).

## Running it

```bash
cd desktop && cargo run -p agent-platform-desktop
```

The port is a persisted setting, not an environment variable: it defaults to
`18410` and is stored in `settings.json` in the app data dir
(`%APPDATA%/com.tanvoid0.agentplatform/` on Windows). The app passes
`AGENT_PLATFORM_PORT` down to the Python child, but does not read it itself — so
setting that variable in your shell changes nothing. To run on another port,
edit `settings.json` while the app is closed.

If something else already answers on the port, the app checks whether the key
matches: a server it owns is adopted, a stranger's is left alone and the Status
screen reports the conflict instead of starting a second server.

Tests:

```bash
cd desktop && cargo test
```

Regenerate the enum bindings after changing a status/enum on the Python side:

```bash
python scripts/sync_contract_enums.py
```
