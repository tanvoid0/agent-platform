---
name: prove-domain
description: Prove a domain migrated from the Python server to the Rust agent-platformd behaves identically, by running its pytest file against both servers and diffing failure sets, then cross-rendering the same rows through both and diffing parsed bodies. Use when migrating or verifying a domain under ADR 0007 (projects, teams, todos, workflows, llm_proxy, processes, assistant, chat, coder, system_routes), or when the user says "prove this domain", "check parity", "does Rust match Python".
---

# Prove a domain

Two servers must answer a domain the same way. Tests alone are not enough — they
did not catch the timestamp-rendering or foreign-key bugs. The body diff did.

## 1. Start the daemon and find both origins

```bash
cd desktop && cargo run -p agent-platform-server
```

On startup it logs its Python child's origin (`… → http://127.0.0.1:<port>`).
That port is Python's; `127.0.0.1:18410` is Rust's. Capture both.

If a master key is set, export it as `AGENT_PLATFORM_TEST_KEY` for both runs.

**Do not point both runs at the live database.** Take a copy per run — the
suites and the cross-render both write. The data is usually in the WAL, so a
file copy loses it; use SQLite's backup API against a read-only handle:

```bash
python -c "import sqlite3 as s; src=s.connect('file:<live>.db?mode=ro',uri=True); dst=s.connect('<copy>.db'); src.backup(dst)"
```

Then start the daemon with `AGENT_PLATFORM_DB_PATH=<copy>.db` and
`DATABASE_URL=""` (empty shadows the repo `.env` for both halves, which
otherwise stops the daemon dead). Both halves read that same variable, so the
child follows.

**Two harness traps, both of which have already produced a fake divergence:**

- **Each run needs its own fresh copy.** These suites mutate rows, so a second
  run against the file the first one left answers differently. That is what made
  `test_sync_terminal_failed_hints_retry` look like a Rust/Python difference when
  it was ordering — re-running the *same* test against the *same* server
  reproduced the "divergence" on its own.
- **Check the port is free, and that the daemon actually bound.** A daemon left
  over from a previous run keeps the port; the new one spawns its child, fails to
  bind, and exits — and the suite then talks to the stale server on a different
  database. It looks like a large regression. Grep the log for `listening on`
  before running anything, and abort if it is absent.

## 2. Diff the failure sets

Run the domain's pytest file against each origin — same file, same key, only the
base URL changes:

```bash
AGENT_PLATFORM_TEST_BASE_URL=http://127.0.0.1:18410 \
AGENT_PLATFORM_TEST_KEY=$KEY pytest app/tests/test_<domain>_api.py -q
```

```bash
AGENT_PLATFORM_TEST_BASE_URL=http://127.0.0.1:<python_port> \
AGENT_PLATFORM_TEST_KEY=$KEY pytest app/tests/test_<domain>_api.py -q
```

**The two failure sets must match exactly** — same test ids, not just the same
count. Residual failures are the tests that mock in-process or touch the test
engine directly; those cannot pass over HTTP against either server and are
expected. Any test that fails on one origin and passes on the other is a real
divergence: fix it, do not annotate it.

## 3. Cross-render the same rows through both

This is the step that finds what no test asserts. For each route in the domain,
`GET` the *same* rows from both origins and diff the **parsed** bodies (parse
first — key order and whitespace are not the signal):

```bash
python - <<'PY'
import json, os, urllib.request
KEY  = os.environ.get("AGENT_PLATFORM_TEST_KEY", "")
RUST = "http://127.0.0.1:18410"
PY_  = "http://127.0.0.1:" + os.environ["PY_PORT"]
PATHS = ["/api/v1/<domain>", "/api/v1/<domain>/<id>"]   # fill in

def get(base, path):
    req = urllib.request.Request(base + path)
    if KEY:
        req.add_header("Authorization", f"Bearer {KEY}")
    with urllib.request.urlopen(req) as r:
        return r.status, json.loads(r.read() or b"null")

for p in PATHS:
    a, b = get(RUST, p), get(PY_, p)
    print(("OK   " if a == b else "DIFF ") + p)
    if a != b:
        print("  rust:", json.dumps(a[1], sort_keys=True)[:400])
        print("  py  :", json.dumps(b[1], sort_keys=True)[:400])
PY
```

Watch specifically for: timestamp format and timezone suffix, `null` vs missing
key, integer vs string ids, empty list vs absent field, and HTTP status on the
not-found / bad-path cases (path rejection has diverged before).

**Drive the writes too, not just reads** — a fixed request *sequence* through
each server, from identical starting bytes, dumping every parsed response. That
is what caught the task-id reuse: `apply_validated_planner_to_process` reads as
delete-then-insert, but SQLAlchemy's unit of work flushes INSERTs **before**
DELETEs, so Python numbers the replacement rows while the old ones still exist
and Rust — deleting first — emptied the table and let SQLite restart `rowid` at
1. Same rows, recycled ids, and a stale `/tasks/1/retry` then addressed a
different task instead of 404ing. No test asserts a row id.

**A status read straight after a route that schedules work is a race, not a
diff.** Both servers flip `approved` → `running` in a background task; whichever
is asked first may not have got there. Compare those by type, or poll until
stable. Confirming the work *did* start is a separate check — retry, then poll
the status and read the daemon log for the model-resolution line.

Write, then re-read: create a row through Rust and fetch it through Python, and
the reverse. Foreign-key and default-value bugs only appear on that crossing.

## 4. Report

State which routes are byte-identical, which failures are shared and why, and
what is still proxied for the domain (`plan.md` has the per-domain
*"Left with Python"* column — update it). Two writers on one table is a known
hazard: Rust writes one column per statement, SQLAlchemy flushes whole rows, so a
Python write can clobber a concurrent Rust one for one request's width. Say so if
the domain is still split.
