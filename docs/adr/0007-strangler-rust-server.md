# 7. Strangler-fig migration of the server to Rust

Date: 2026-08-05

## Status

Accepted. Reopens the question [ADR 0004](0004-desktop-shell-tauri-python-sidecar.md)
closed and [ADR 0006](0006-in-process-rust-core.md) closed again — on a different
mechanism. 0004 and 0006 both rejected a *port*: a rewrite with a flag day at the
end of it. This ADR proposes no flag day and no rewrite; it puts a Rust process in
front of the Python one and moves domains across it a few at a time, with the
Python server still answering everything that has not moved.

0006's inference decision (llama.cpp linked in-process, `local_llm.rs`) is unchanged
and is subsumed by this: the engine moves out of `crates/app` and into the new server
crate when the LLM proxy domain migrates, so a cloud deployment gets it too.

## Context

Two requirements were previously in conflict (see 0006): ship embedded with the
desktop app, and serve other API clients as a real server. A third has since been
stated: the platform should eventually deploy to the cloud **without** the desktop
client, while staying desktop-first for local AI management.

That third requirement is what changes the arithmetic. It rules out the shape 0006
proposed and rejected (one binary containing the iced UI *and* the API), because a
cloud deployment must not carry a GUI. It also rules out "leave it in Python
forever", because the two artifacts then diverge: the desktop gets in-process
inference and native calls, the cloud gets neither.

What has not changed:

- The Python server is 31k LOC, of which the native client touches ~36 endpoints.
- `app/` has no heavy AI dependencies. It is CRUD, an HTTP proxy, and orchestration.
- Embedded CPython already ships (`desktop/payload/runtime/`), so "no Docker" is met
  today. This ADR does not claim otherwise, and does not use that as a motive.
- `scripts/sync_contract_enums.py` already emits `enums_gen.rs` from
  `app/shared_enums.py`.

What is genuinely worse in Python and does not get better by waiting: per-request
serialization on a UI that polls at 1s/3s/800ms, and the fact that every desktop
interaction crosses loopback HTTP + JSON to reach a process that is in the same
install.

## Decision

A new crate `desktop/crates/server` (lib + bin `agent-platformd`) binds the public
port. Everything it does not implement is reverse-proxied to the Python server,
which it spawns as a child on an ephemeral loopback port.

```
client ──▶ agent-platformd :18410
             ├── migrated domains  (axum + sqlx)
             └── fallback ─────────▶ python (uvicorn) :<ephemeral>
```

The external contract does not change as domains migrate. `crates/client` and every
screen in `crates/app` stay untouched per migration.

### Rules

1. **Migrate by table-owning domain, never by route.** SQLite survives two writers
   (WAL + `busy_timeout`), but a table written by both Python and Rust is where
   invariants diverge silently. A domain moves whole or not at all.

   Two clarifications the projects slice forced. A route that only *reads*
   another domain's table is not part of this domain and stays proxied
   (`GET /projects/{id}/processes` serializes the process table, so it migrates
   with processes). A route that *writes* one — `DELETE` nullifying
   `process.project_id` — moves with its domain, because the write has to be in
   the same transaction as the delete or the FK dangles; that is the single
   statement Rust issues outside its own table, and it is named here so the next
   one has to be argued for too.
2. **Alembic stays the only migration owner.** sqlx issues runtime queries against
   the schema Alembic produces. No second migration tool, no `sqlx migrate`.
3. **A domain may not migrate until its tenancy cases pass in Rust.** The contract
   is `app/tests/test_workspace_tenancy.py` — cross-tenant reads 404, not 401.
4. **Parity is proven by the existing pytest suite wherever that suite is
   portable.** Nearly every test reaches the app through one fixture
   (`app/tests/conftest.py::client`), so a second fixture that talks HTTP to a
   live server is a one-place change — `TestClient` is already an `httpx.Client`.
   But that fixture also hands out a mocked `DAGExecutor`, and 20 of the 63 test
   files assert on those mocks while 30 monkeypatch server internals, and some
   assert on rows through the test engine directly. Those are *not* parity
   evidence: they test Python objects, not the contract. A domain whose tests are
   in that group has to have its assertions restated as HTTP behaviour before it
   can migrate. Do not re-encode the portable ones in Rust.

   The harness is `AGENT_PLATFORM_TEST_BASE_URL` (plus `AGENT_PLATFORM_TEST_KEY`)
   in `app/tests/conftest.py`: set it and the `client` fixture talks HTTP to a
   running server instead of building an in-process app. Measured on
   `test_projects_api.py`: 8 of 10 pass against Python and the same 8 against
   Rust, failing on the same two lines — both reach into the test engine for
   rows. Loud, which is the point. Those two behaviours (FK nullification on
   delete, the payload column) were then checked directly against the database.

   The strongest evidence is cheaper than any of that, though: fetch the same
   route from Rust and from the Python child and compare the parsed bodies. The
   projects list came back deep-equal, which no test asserts and no reviewer
   would have caught by reading.
5. **No behaviour is "improved" during a migration.** A domain lands byte-identical
   or it does not land. Improvements are separate commits, after.

### Order

| # | Domain | Why here |
|---|--------|----------|
| 1 | auth (`api_auth.py`, `api_tokens/`) | every other slice needs the principal; the proxy must reject before forwarding |
| 2 | `/health`, `/` | zero coupling — the only two routes that depend on no Python-owned fact |
| 3 | projects ✅, teams ✅, then todos, workflows | leaf tables, no LLM, ~1.5k LOC |
| 4 | `llm_proxy/` | ~3k LOC; also where `local_llm.rs` moves so the cloud binary gets in-process inference |
| 5 | processes / orchestrator / action_orchestrator | FastAPI `BackgroundTasks` + `asyncio.create_task` → tokio; needs a `startup_recovery` equivalent |
| 6 | assistant, chat, coder | largest and highest-churn |
| 7 | `system_routes` | an aggregator, so it can only be last — see below |

**`system_routes` moves last, not first.** The first draft of this ADR put it
second, on the reasoning that the UI polls it hardest. Reading it says otherwise:
`/system/status` is a fan-in over facts other domains own — `llm_proxy`'s provider
readiness, the Python interpreter version, the process-status counts — and
`/system/logs` serves Python's own in-process log ring, which Rust cannot hold.
Migrating it early would mean either calling back into Python for most of the body
or changing what the fields mean. It migrates when the domains it aggregates have.

`/health` is the exception worth taking early, and it is not a pure proxy: the
daemon answers it from the child's liveness, so a dead server reports `503` with
`{"status": "down"}` instead of a transport error.

### Staying Python permanently

The MCP client (`mcp_streamable_client.py`), the `model_ops` training pipeline
(torch/peft), and PDF extraction (pymupdf). These keep their Python implementation
and live *behind* the Rust server — the inverse of today's arrangement. The fallback
proxy is their permanent home, not scaffolding.

### Desktop wiring

`shell.rs` keeps spawning exactly one child; the child is `agent-platformd`
instead of `python start.py`, and the daemon inherits the same environment and
passes it to the Python child with only the bind address overridden. Finding
Python (bundled payload, then repo checkout) moved from `shell.rs` into the
daemon, which is the only process that needs to know. Both binaries ship in the
installer.

The app also puts the daemon in a kill-on-close job object, the same way the
daemon does for Python: a crash of the UI must not leave a server holding the
port and the database.

Direct in-process calls (`crates/app` depending on the server lib and skipping HTTP
entirely) are deliberately **not** part of this ADR. They become a one-line change
per call site once a domain is in the same workspace, and they should be made where
measurement says they matter, not everywhere.

### Cloud

`agent-platformd` is the cloud artifact from day one — same binary, no iced, Python
child either spawned alongside or pointed at with `AGENT_PLATFORM_UPSTREAM`. When
the last Python-only domain is gone, the upstream is dropped and the fallback becomes
a 404.

## Consequences

- Two auth implementations exist for as long as the proxy does: Rust validates, then
  Python re-validates the forwarded request. That is deliberate — one process cannot
  be trusted to have removed a check the other still enforces. The cost is one extra
  token hash + lookup per request; the risk is divergence, which rule 4 covers.
- The in-memory rate limiter is per-process, so it now counts in two places. Both
  count every request, so the effective limit is unchanged, but both must be
  restarted together to reset a window.
- `last_used_at` is written by whichever process terminates the request. For proxied
  routes that stays Python.
- **Postgres is not supported by the Rust server yet.** Slice 1 is SQLite-only
  (`sqlx::SqlitePool`); the desktop forces `DATABASE_URL=""` anyway. Postgres support
  lands with the cloud deployment, not before, and until then `agent-platformd`
  refuses to start against a non-SQLite `DATABASE_URL` rather than silently reading a
  different database than its Python child.
- `/api/v1/system/status` would otherwise report `listening_on` as the child's
  ephemeral address. The daemon passes `AGENT_PLATFORM_PUBLIC_HOST`/`_PORT` and
  `system_routes.py` prefers them, so the field keeps meaning "the address you
  reached us on". That is the one Python change this slice needed.
- **Timestamps are read and written as text, not as `NaiveDateTime`.** Two
  separate diffs, both invisible to every test: the seeded team rows were written
  by aware-datetime code and carry `+00:00`, which Python renders with a trailing
  `Z` and a `NaiveDateTime` decode silently drops; and binding a `NaiveDateTime`
  stores *nanoseconds* on Windows, so a row Rust wrote read back as
  `…036520900` here and `…036520` from Python. `wire::sql_now` writes exactly
  what SQLAlchemy writes and `wire::iso_from_sql` renders exactly what pydantic
  renders. Anything storing a timestamp goes through them.
- The proxy forwards the caller's `Host` header. Dropping it looked tidier until
  a trailing-slash `307` came back with `location: http://127.0.0.1:<ephemeral>/…`
  — FastAPI builds redirect targets from `Host`, so the client would have been
  sent around the proxy to a port that changes on every restart.
- Rust registers both `/api/v1/projects` and `/api/v1/projects/`. Answering is a
  better contract than a redirect the caller has to follow through a proxy.
- On Windows the daemon puts the Python child in a job object with
  `KILL_ON_JOB_CLOSE`. Without it, `shell.rs` hard-killing the daemon would leave
  a uvicorn holding the database and its port — verified, and the reason the child
  is not merely relying on `--exit-with-parent`.
- Rollback at any point is deleting the Rust handler for a domain: the fallback
  already routes it back to Python.

## Kill criteria

Abandon and revert to Python-only if any of these hold after slice 3:

- A migrated domain cannot pass the existing pytest slice without changing the test.
- The proxy adds measurable latency to unmigrated routes beyond ~1ms at p99 on
  loopback.
- Maintaining the two-implementation window costs more than the domains are saving.
