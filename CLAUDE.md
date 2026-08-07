# CLAUDE.md

`plan.md` is the continuity doc and it stays current — read its **Backlog** and
`### … next steps` sections before starting work, and update it when a step
lands. Architecture decisions live in `docs/adr/`. This file holds only what
`plan.md` does not: the shape, the commands, and the rules that are easy to break.

## Shape

- **`desktop/crates/server`** — Rust/axum `agent-platformd`, **the whole server**.
  Binds `127.0.0.1:18410`, answers every route itself. All REST is under
  `/api/v1/…`, plus the OpenAI-compatible LLM proxy on `/v1/*` in the same
  process. There is no `/tokens` browser page; the desktop app is the only UI.
- **`desktop/crates/app`** — iced 0.14 native app, **the only UI**.
  `desktop/crates/client` is its HTTP/SSE client.
- **`worker/`** — the model-ops LoRA training pipeline. Python, because it is
  torch and peft, but **not a server**: `agent-platformd` runs each build stage
  as a subprocess and reads results off its stdout. Nothing imports a server
  from it and it imports nothing from one.
- There is no web frontend and no Python server. `app/` (FastAPI), `web/` and
  the Tauri shell are all deleted — [ADR 0005](docs/adr/0005-native-iced-desktop-headless-server.md),
  [ADR 0007](docs/adr/0007-strangler-rust-server.md). A sibling `../flow-ui`
  checkout is a leftover, not part of this project.

`plan.md` → **Where to edit** maps every area to its path. Use it before grepping.

## Commands

```bash
cd desktop && cargo test                               # the test suite
cd desktop && cargo build                              # run this too — see plan.md's runbook
python scripts/check_repo_hygiene.py                   # tracked-path hygiene
```

```bash
cd desktop && cargo run -p agent-platform-desktop      # the app (spawns agent-platformd itself)
cd desktop && cargo run -p agent-platform-server       # the server alone
```

There is no CI. Nothing runs these but us — run them before claiming a change works.

## Rules

- **`desktop/crates/client/src/enums.rs` is hand-maintained now.** It was
  generated from `app/shared_enums.py`; both that file and the generator
  (`scripts/sync_contract_enums.py`) are gone with the Python server. The values
  are the wire contract — the server writes them as strings, so changing a
  variant does not change what the server emits. Grep for the string first.
- **The schema is `desktop/crates/server/src/schema.sql`**, applied by
  `db::ensure_schema` at startup. It replaced Alembic, and it **creates rather
  than migrates**: a new column has nowhere to go until someone builds a
  versioned runner. Read the doc comment on `ensure_schema` before changing a
  table.
- **Desktop screens split by file**: state + `update` in `x.rs`, rendering in
  `x_view.rs`. Widgets and tokens come from `desktop/crates/app/src/ui/` —
  screens compose kit functions, they do not style ad hoc.
- **A running app locks `desktop/target/debug/agent-platformd.exe`.** Build with
  `--target-dir` pointing outside the repo instead of killing the app
  (`.gitignore` pins `desktop/target/` exactly, so a sibling dir inside the repo
  shows up untracked).
- **`agent-platformd` is SQLite-only** and refuses to start with `DATABASE_URL`
  set. The `sqlx::Any` pool that lifts that restriction is half-converted; see
  `AppState`.
- **Diagnostics go through `logd!`, not `eprintln!`.** It writes the same line to
  stderr *and* into the ring `GET /system/logs` serves. There is exactly one
  `eprintln!` left in the crate, inside `observability::diagnostic`.
- **`worker/` must not import from the server.** It is spawned by subprocess and
  talks back over stdout (`@@AGP:<kind>@@ {json}`), which is what let the
  build-job routes leave Python. A `from database import …` in there re-creates
  the two-writer problem that blocked them.
- Do not hand-edit `*.db`, `data/`, or `.env`.
- Windows host: the primary shell is PowerShell, the Bash tool is git-bash. Start
  servers in the background, never in the foreground.
- **Never glob the repo root.** `desktop/target/` and `node_modules/` will drown
  the search — scope to `desktop/crates/`, `worker/`, `docs/`, `scripts/`.

Rust-side detail (iced 0.14 gotchas, local inference linking, runtime contract):
[`desktop/CLAUDE.md`](desktop/CLAUDE.md).
