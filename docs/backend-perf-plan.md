# Backend performance fixes — plan

Two fixes to the FastAPI server, chosen because they are cheap, safe, and stay
valuable regardless of any later framework decision. Everything here is
internal: no API surface changes, no schema changes, no new dependencies.

**Out of scope** (tracked separately): the durable job table, moving the rate
limiter onto a shared store, and the 53 genuinely-async functions whose blocking
DB work needs `asyncio.to_thread`. Those are correctness-under-multi-client work
and are larger; see the end of this doc.

---

## Fix 1 — one pooled `httpx.AsyncClient` instead of one per call

### Problem

Every upstream call constructs a fresh `httpx.AsyncClient`, so every LLM request
pays a new TCP connection and a new TLS handshake. Nothing is ever reused.

Hot path, all upstream traffic:

- `app/llm_proxy/services/upstream_http.py:217` — `get`
- `app/llm_proxy/services/upstream_http.py:255` — `post`
- `app/llm_proxy/services/upstream_http.py:316` — `open_stream`

Roughly 20 further one-off `httpx.AsyncClient(...)` sites exist outside this
class (assistant, coder, playground, workflows, model_ops, llm_ui_catalog).

### Why this is small

The seam already exists. `upstream_http.py:361` defines a module-level singleton:

```python
default_upstream_client = UpstreamHttpClient()

get_with_retry = default_upstream_client.get
post_with_retry = default_upstream_client.post
stream_chat_completion = default_upstream_client.open_stream
```

Every caller in `admin_routes.py`, `routes/llm.py`, `chat_routes.py` and
`local_backends.py` already goes through these three names. Only the internals
of `UpstreamHttpClient` change; no call site moves.

### Steps

1. **Hold one client on the instance.** Lazily created, with explicit
   `httpx.Limits`. Per-call timeouts stay per-call — httpx accepts `timeout=` on
   each request, so the existing signatures are unaffected.

2. **Stop handing the client to callers.** `open_stream` currently returns
   `(response, client)` and callers pass both to `aclose_stream`. Return
   `(response, None)` instead: `aclose_stream` at `upstream_http.py:143` already
   no-ops on `client is None`, so the response still closes and the shared client
   survives. Zero churn at the six call sites — `chat_routes.py:162` and `:177`,
   `admin_routes.py:733` and `:748`, `routes/llm.py:1168` and `:1183`.

3. **Close it on shutdown**, in the `lifespan` `finally` block in
   `app/main.py:62`, beside the existing `cache.stop_background_refresh()`.

4. **Leave the ~20 one-off sites alone for now.** They are cold paths — startup
   discovery, catalog fetches, occasional uploads. Fold them in later only if
   they show up in a measurement.

### Risk to handle: event-loop binding

A single client reused across *different* event loops will hold pooled
connections belonging to a dead loop. Production has one loop, but the test suite
creates a `TestClient` (and therefore a loop) per module.

Mitigation: create the client lazily and record the running loop alongside it;
if the current loop differs from the recorded one, discard and recreate. This is
a few lines and makes the failure mode impossible rather than unlikely.

---

## Fix 2 — drop `async` where nothing is ever awaited

### Problem

An `async def` handler runs **on** the event loop. A plain `def` handler is
dispatched by FastAPI to a worker thread. Any blocking call in the first shape
stalls every other request and every in-flight SSE stream; in the second shape it
stalls only its own thread.

An AST scan of `app/` (excluding tests) found **19 async functions that touch a
`Session` and contain no `await`, `async for`, or `async with` at all.** They pay
the cost of running on the loop and get nothing for it. The fix is to delete the
word `async`.

### Order of work

**2a — the auth dependency.** `app/api_tokens/auth.py:45`, `_dependency`.

Do this one first and alone. It runs on **every authenticated request** via
`require_valid_token`, and it performs a token lookup, a workspace lookup, and —
up to once a minute per token — a `commit`. All blocking, all on the loop today.
It has no `await` anywhere in its body. One keyword, largest single win.

**2b — the other pure-passthrough dependency.**
`app/route_dependencies.py:21`, `session_with_auth`.

**2c — `process_routes.py`, six handlers with no await:**
lines `125`, `236`, `277`, `374`, `525`, `597`. Plus `650`
(`stream_process_events`) — the outer function only builds a
`StreamingResponse`; its nested `event_generator` at `665` stays async and is
untouched.

**2d — stream-response builders**, same shape as above (outer builds the
response, inner generator stays async):
`app/coder/routes.py:161`, `app/playground/routes.py:143`,
`app/model_ops/routes.py:449`.

**2e — remaining handlers:** `app/coder/routes.py:193`, `:225`, and
`app/assistant/routes.py:150`.

**2f — the `get_thread` cascade.** Three service functions have no await:
`app/assistant/services/assistant_chat.py:637`, `app/coder/service.py:256`,
`app/playground/service.py:114`.

De-asyncing them means deleting the `await` at their only three call sites —
`app/assistant/routes.py:164`, `app/coder/routes.py:111`,
`app/playground/routes.py:100` — and `get_thread` is the *only* thing those three
routes await. So `chat_thread:159`, `coder_thread:105` and
`playground_thread:94` become plain `def` as well. Six functions moved for three
deleted keywords.

This also matches what is already there: the sibling `delete_thread` routes
sitting directly beneath each of them are plain `def` today.

### Deliberately excluded

- `app/services/startup_recovery.py:48`, `recover_interrupted_processes` — the
  scan flags it, but its caller awaits it. Must stay async.
- **The 53 functions that genuinely await.** Their blocking DB work still runs on
  the loop, and fixing that means wrapping DB blocks in `asyncio.to_thread` —
  a real change to real logic, not a keyword deletion. Separate piece of work,
  worth doing after these land and after there is a measurement to justify it.

**Net: 18 functions moved off the event loop, almost entirely by deletion.**

---

## Verification

`app/tests/` is ~9,800 lines and covers these routes. Run the suite green before
starting, then after **each** phase — not once at the end. A de-async that breaks
something breaks it loudly and immediately in this suite.

```bash
python -m pytest app/tests -q
```

For Fix 1 specifically, tests passing only proves nothing regressed, not that
pooling works. Add one small check that issues two requests against a local
handler and asserts the same connection served both — that is the thing that
silently stops being true if someone reintroduces a per-call client.

Note there is no benchmark harness in the repo and this plan does not add one.
The connection-reuse assertion is the check; wall-clock numbers against a real
provider are noise next to token-generation time anyway.

---

## Sequence

| Phase | Change | Size |
|---|---|---|
| 1 | Fix 2a — auth dependency | 1 keyword |
| 2 | Fix 2b–2f — remaining 17 functions | ~20 lines, mostly deletions |
| 3 | Fix 1 — pooled client + loop guard + lifespan close | ~30 lines |

Tests green at each boundary. Phases are independently revertable.

---

## Outcome — all three phases landed

Suite went from **436 passed / 2 failed** to **441 passed**. Four deviations from
the plan above, all discovered during implementation:

1. **Phase 1 did not de-async the auth dependency.** `_dependency` calls
   `update_request_context`, which writes a **contextvar**. A `def` dependency runs
   in a threadpool, and anyio gives that thread a *copy* of the context — the write
   would be discarded, silently dropping tenant attribution from any log line
   emitted inside a handler. (The `request.completed` log is unaffected; the
   middleware re-derives workspace from `request.state` at `observability.py:194`.)
   Instead the blocking half moved to `_resolve_agp_token` and is called via
   `asyncio.to_thread`. Same win, contextvar writes stay on the loop.

   **Generalized rule for the remaining 53:** check for contextvar writes before
   de-asyncing anything. The Phase 2 targets were scanned and are all clean.

2. **Unplanned: `conftest.py` speech-env scrub.** Baseline was not green —
   `SPEECH_API_BASE`/`SPEECH_DEFAULT_VOICE` in a developer's `.env` leak into
   `test_capabilities.py`, which passes in CI and fails locally. Same hazard the
   `AGENT_PLATFORM_MASTER_KEY` fixture already guards against, so it got the same
   treatment. Needed before per-phase verification meant anything.

3. **Two test fakes updated.** `test_coder_api.py` and
   `test_upstream_rate_limit_retry.py` stub `httpx.AsyncClient` for the old
   per-call contract — no `is_closed`, no `timeout` kwarg. Note
   `monkeypatch.setattr("coder.service.httpx.AsyncClient", ...)` patches the httpx
   module *globally*, so that fake reaches the pool; pre-existing smell, left alone
   beyond making it work.

4. **`open_stream` keeps its 2-tuple**, now always returning `None` as the second
   element, so the six `response, client = await ...` call sites are untouched.

### Regression guards

Both were checked against the broken version, not just the working one — a guard
that cannot fail is not a guard.

- `app/tests/test_upstream_pooling.py` — 3 sequential GETs share **1** connection.
  Per-call clients produce **3**, confirmed directly.
- `app/tests/test_auth_context_propagation.py` — a log record emitted *inside* a
  handler carries `workspace_id`. Confirmed a `def` dependency loses the write
  (handler sees `None`) while the `async def` form keeps it, so this fails if
  anyone "simplifies" the auth dependency to a plain `def`.

### Live smoke test

Booted the real server (not just TestClient), minted a workspace token, and drove
the changed paths: the `agp_` auth path through `asyncio.to_thread` (5 consecutive
authenticated calls, all 200), the de-asynced `get_thread` cascade, and the
de-asynced stream-response builders. 22 requests served, **zero errors or
tracebacks** in the log.

One honest caveat from that run: every `workspace_id` log line came from
`agent_platform.request`, the middleware logger, which re-derives from
`request.state`. So the live run did *not* by itself prove the contextvar path —
that is what the dedicated test above is for.

## What this does not fix

Worth stating plainly so it is not mistaken for done. Under a **single** desktop
user these two fixes are most of the available win. Under **multiple API
clients**, the binding constraints are elsewhere:

- `app/api_tokens/rate_limiter.py` keeps counters in a process-local dict, so
  `ApiToken.rate_limit_per_minute` is unenforceable across workers. This is a
  correctness bug, not a performance one.
- DAG work runs on `BackgroundTasks` (27 call sites), so a restart strands
  in-flight processes; `app/services/startup_recovery.py` exists solely to clean
  up after that.
- `app/workflows/engine.py:224` is a bare poll loop — N workers means N
  schedulers and duplicate fires.

All three are answered by the same durable job table (Postgres
`SELECT ... FOR UPDATE SKIP LOCKED`, degrading to single-worker polling on
SQLite). That is the next piece of work, and it is a prerequisite for running
more than one worker at all.
