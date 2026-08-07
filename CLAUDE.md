# CLAUDE.md

`plan.md` is the continuity doc and it stays current — read its **Backlog** and
`### … next steps` sections before starting work, and update it when a step
lands. Architecture decisions live in `docs/adr/`. This file holds only what
`plan.md` does not: the shape, the commands, and the rules that are easy to break.

## Shape

- **`app/`** — FastAPI server: process orchestration (goal → planner DAG →
  approval → topological execution) plus an embedded OpenAI-compatible LLM proxy
  on `/v1/*` in the same process. All REST is under `/api/v1/…`; the only browser
  page is the `/tokens` dashboard.
- **`desktop/crates/server`** — Rust/axum `agent-platformd`. Binds
  `127.0.0.1:18410`, answers the domains that have migrated, spawns a Python
  child on an ephemeral port and reverse-proxies the rest byte-for-byte
  ([ADR 0007](docs/adr/0007-strangler-rust-server.md)).
- **`desktop/crates/app`** — iced 0.14 native app, **the only UI**.
  `desktop/crates/client` is its HTTP/SSE client.
- There is no web frontend: `web/` and the Tauri shell are deleted
  ([ADR 0005](docs/adr/0005-native-iced-desktop-headless-server.md)). A sibling
  `../flow-ui` checkout is a leftover, not part of this project.

`plan.md` → **Where to edit** maps every area to its path. Use it before grepping.

## Commands

```bash
pytest -q                                              # Python tests (root; pytest.ini sets pythonpath/testpaths)
cd desktop && cargo test                               # Rust tests
python scripts/check_repo_hygiene.py                   # tracked-path hygiene
python scripts/sync_contract_enums.py                  # regenerate client enums (see Rules)
cd app && alembic revision --autogenerate -m "msg"     # new migration; migrations run on startup
```

```bash
cd desktop && cargo run -p agent-platform-desktop      # the app (spawns agent-platformd itself)
cd desktop && cargo run -p agent-platform-server       # daemon only (spawns the Python child)
python -m uvicorn main:app --app-dir app --host 127.0.0.1 --port 18410   # Python alone
```

There is no CI. Nothing runs these but us — run them before claiming a change works.

## Rules

- **`desktop/crates/client/src/enums_gen.rs` is generated.** Source of truth is
  `app/shared_enums.py`; edit the Python enum, then run
  `python scripts/sync_contract_enums.py`. Never hand-edit the `.rs`.
- **Route modules stay thin.** HTTP in `app/*_routes.py`, logic in
  `app/services/*_service.py`.
- **Python imports are flat.** The server runs with `--app-dir app`, so it is
  `import main`, not `app.main`.
- **Desktop screens split by file**: state + `update` in `x.rs`, rendering in
  `x_view.rs`. Widgets and tokens come from `desktop/crates/app/src/ui/` —
  screens compose kit functions, they do not style ad hoc.
- **A running app locks `desktop/target/debug/agent-platformd.exe`.** Build with
  `--target-dir` pointing outside the repo instead of killing the app
  (`.gitignore` pins `desktop/target/` exactly, so a sibling dir inside the repo
  shows up untracked).
- **`agent-platformd` is SQLite-only** and refuses to start with `DATABASE_URL`
  set. `AGENT_PLATFORM_UPSTREAM` attaches to an already-running server instead of
  spawning one.
- **Moving a domain from Python to Rust is proved, not eyeballed** — run the same
  pytest file against both servers and diff the failure sets, then cross-render
  the same rows through both and diff the parsed bodies. See the
  `prove-domain` skill; that method is what surfaced the timestamp and
  foreign-key bugs.
- Do not hand-edit `*.db`, `app/data/`, or `.env`.
- Windows host: the primary shell is PowerShell, the Bash tool is git-bash. Start
  servers in the background, never in the foreground.
- **Never glob the repo root.** `desktop/target/` and `node_modules/` will drown
  the search — scope to `app/`, `desktop/crates/`, `docs/`, `scripts/`.

Rust-side detail (iced 0.14 gotchas, local inference linking, runtime contract):
[`desktop/CLAUDE.md`](desktop/CLAUDE.md).
