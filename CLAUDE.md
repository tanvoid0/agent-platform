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

`.github/workflows/ci.yml` runs all three on push and PR — the server and
hygiene on Linux, the desktop app on Windows. Run them locally anyway before
claiming a change works; CI is the backstop, not the loop.

## Rules

- **`desktop/crates/client/src/enums.rs` is hand-maintained now.** It was
  generated from `app/shared_enums.py`; both that file and the generator
  (`scripts/sync_contract_enums.py`) are gone with the Python server. The values
  are the wire contract — the server writes them as strings, so changing a
  variant does not change what the server emits. Grep for the string first.
- **The schema lives in `desktop/crates/server/migrations/`**, run by
  `db::ensure_schema` (`sqlx::migrate!`) at startup. It replaced Alembic.
  `0001_initial.sql` is the squashed final Alembic head and **must not be
  edited** — sqlx checksums an applied migration and refuses to start against a
  changed copy. A schema change is a new `000N_*.sql` beside it, forward-only.
- **Desktop screens split by file**: state + `update` in `x.rs`, rendering in
  `x_view.rs`. Widgets and tokens come from `desktop/crates/app/src/ui/` —
  screens compose kit functions, they do not style ad hoc.
- **A running app locks `desktop/target/debug/agent-platformd.exe`.** Build with
  `--target-dir` pointing outside the repo instead of killing the app
  (`.gitignore` pins `desktop/target/` exactly, so a sibling dir inside the repo
  shows up untracked).
- **`agent-platformd` runs on SQLite or Postgres**, decided by `DATABASE_URL` —
  the `sqlx::Any` pool migration finished, so `Config::from_env` no longer
  refuses a DSN and there are two migration sets under `migrations/`. An *empty*
  `DATABASE_URL` is not a DSN and must be unset rather than passed on
  (`docker/entrypoint.sh` does this); the desktop is SQLite either way.
- **The Cloud Run deploy is free-tier only, and that is a hard rule.** No Cloud
  SQL, no Serverless VPC connector, no load balancer or IAP, no Cloud Build —
  each of those bills monthly whether or not anything connects, and every one of
  them costs more than the service. `DATABASE_URL` points at a Postgres outside
  GCP; the service deploys `--no-allow-unauthenticated` so an anonymous flood
  never starts a billable container, with `--max-instances=1` as the ceiling if
  it is ever opened. See `.github/workflows/deploy-cloud-run.yml`.
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
