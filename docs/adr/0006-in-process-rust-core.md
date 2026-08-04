# 6. In-process Rust core: one binary for UI, API and inference

Date: 2026-08-04

## Status

**Proposed — deferred. Nothing is being built against this.**

[ADR 0004](0004-desktop-shell-tauri-python-sidecar.md) considered a Rust server and
rejected it. This ADR does not overturn that; it records why the question reopened,
what evidence would settle it either way, and the spike that would produce that
evidence. Revisit when one of the triggers under *What would settle this* fires.

## Context

### What ADR 0004 decided, and why

0004 rejected rewriting the server in Rust or C++ on two grounds:

> the server is I/O-bound (`httpx` to an upstream, SSE relay), per-request framework
> overhead is a rounding error next to model latency

> the training pipeline is torch/transformers and cannot leave Python regardless

Both were correct when written. Their standing today differs:

**The throughput argument was right but is now scoped to a system that no longer
exists.** It compared framework overhead against *model latency*, in a world where
the UI was a WebView2 webview that would have spoken HTTP to a local server no
matter what language the server was written in. Under [ADR 0005](0005-native-iced-desktop-headless-server.md)
the UI is a **native Rust process**, and the cost in question is no longer
per-model-call — it is HTTP plus JSON serialization on *every UI interaction*: every
list, every poll, every SSE frame. That is a different quantity from the one 0004
priced, and it did not exist as a cost until 0005 landed.

This distinction was got wrong once already during the discussion that produced this
ADR: the loopback hop was dismissed by comparing it to token-generation time (~0.5ms
against ~20-30ms/token, which is accurate and irrelevant). The hop is not the cost.
The per-interaction serialization is.

**The training argument is still true and still does not bind.** torch/transformers
cannot leave Python. But training already runs out-of-process and out-of-bundle
(`MODEL_OPS_GPU_SUBPROCESS`, default on; `Dockerfile.train`; `requirements-train.txt`
installed on the train worker only). It constrains the *training worker*, not the
*server*.

### Requirements that arrived after 0004

- **Ship the backend embedded with the app, no Docker** — the Ollama / LM Studio
  shape. Docker is a dev-loop convenience, not the distribution path.
- **Own the local inference engine** (llama.cpp / GGUF) rather than shelling out to
  an Ollama the user has to install separately.
- **Serve other API clients** — jobs, queues, multi-agent orchestration, projects,
  teams — not just the local desktop app.
- **The client is already Rust/iced** (ADR 0005), and already owns the server's
  lifetime.

The first two and the last pull toward a single native binary. The third pulls
toward a server-shaped deployment with Postgres and N workers. **These conflict**,
and no single artifact is good at both — which is the strongest argument for the
status quo, and is treated as such below.

### Measured facts

Gathered 2026-08-04, worth re-checking before deciding:

- `app/` contains **zero** heavy AI imports. No torch, numpy, pandas, transformers,
  or sklearn outside `model_ops/pipeline/`. The backend is CRUD + HTTP proxy +
  orchestration.
- **31,306 LOC** in `app/` excluding tests; **9,777 LOC** of tests.
- [shell.rs](../../desktop/crates/app/src/shell.rs) is ~450 lines whose entire job is
  spawning, port-probing, attaching to, and reaping a child process.
- Rust equivalents exist for nearly the whole dependency list: axum, sqlx, reqwest,
  serde, `tiktoken-rs`, `rmcp`. The one real downgrade is `pymupdf` →
  `pdfium-render`.

## Decision (proposed)

**One binary containing three things that are three processes today:**

- **iced UI**, calling the core through **direct function calls** — no HTTP, no
  JSON, no port.
- **axum HTTP server**, exposing that same core on a port for external API clients.
- **llama.cpp linked in** (`llama-cpp-2` or `mistral.rs`), inference in-process,
  tokens streaming straight into the UI.

The Python training pipeline stays exactly where it is: optional, out-of-process,
out-of-bundle.

## Alternatives, and why they lose

**Status quo — Python sidecar.** The serious contender. Works today, 441 tests
green, zero migration risk, and keeps the AI ecosystem within reach for whatever
lands next. Fails only the *embedded* and *own-inference* requirements, and only if
those are genuinely first-class rather than aspirational. **If they are not, this
ADR should be rejected outright.**

**Bundled CPython** (python-build-standalone, ~30MB, the ComfyUI-portable pattern).
Removes the "find a Python" fragility for a few days of build-pipeline work, and
requires no rewrite. But it is still a child process, still an interpreter, and
still a wheel matrix per platform × accelerator. **This is the cheap answer if the
real pain is distribution rather than architecture**, and it should be tried first
on that reading.

**Quarkus / JVM.** Considered because clustered Quartz, JTA and virtual threads suit
the multi-tenant half well. Rejected: `java-llama.cpp` bindings are the weakest of
the available options, and GraalVM native image plus FFI into GPU libraries is
exactly where native image stops being cheap. It loses hardest on the requirement
that motivated the question.

**Go.** What Ollama actually does (cgo → llama.cpp). Fine, but there is no Go in the
stack and Rust is already here.

## Consequences

**Good**

- No spawn, no port probe, no PID file, no interpreter, no orphan reaping. ~450
  lines of [shell.rs](../../desktop/crates/app/src/shell.rs) stop existing.
- UI interactions lose HTTP and serialization entirely.
- Inference is in-process: model load/unload policy, VRAM budgeting, KV-cache reuse
  across requests, GBNF grammar-constrained output — none of which Ollama's API
  exposes.
- One artifact to sign, install and update.
- Two classes of bug become unrepresentable rather than merely fixed: connection
  pooling is default in `reqwest`, and `sqlx` is async end-to-end, so there is no
  sync-DB-in-async-handler mistake to make. (Both were live issues; see
  [backend-perf-plan.md](../backend-perf-plan.md).)

**Bad**

- **31,306 LOC to port.** This does not shrink because the destination is nicer.
- **Rust lags Python when a genuinely new AI technique lands.** Partly covered:
  GGUF/llama.cpp tracks mainstream local inference closely, remote providers are
  just HTTP+SSE (the codebase already hand-rolls `httpx` rather than using vendor
  SDKs), MCP has `rmcp`. Not covered: PDF extraction quality, and anything exotic.
  Mitigation is that a Python-only capability becomes an *optional* out-of-process
  worker, as training already is — never a required runtime.
- **The build matrix does not go away.** Per platform × per accelerator, the same
  matrix Ollama ships. GraalVM would have had it too; so does a Python wheel bundle.
- The multi-tenant server story gets no better from this and arguably worse: a
  desktop-shaped binary is not what you scale to N workers.

**During transition**

The Python child process survives until the last route group moves. A strangler
migration means living with *both* for its duration — this buys nothing until it is
nearly finished, which is the main reason to be honest about the cost up front.

## What would settle this

**Decide yes if:**

- Shipping without a separate runtime is a real product requirement rather than a
  preference — e.g. install-failure reports traceable to Python, or a signing /
  distribution constraint the sidecar cannot meet.
- Owning inference becomes load-bearing: VRAM policy, multi-model residency, or
  grammar-constrained output that Ollama's API cannot express.
- The Phase 0 spike clears its bars below.

**Decide no if:**

- Bundled CPython removes the distribution pain and nothing else hurts. **Try this
  first.**
- The multi-client API becomes the primary product and desktop becomes one of its
  clients. That inverts the requirement conflict, and Python-with-Postgres — or a
  JVM stack — wins instead.
- The spike shows GPU llama.cpp bindings on Windows are unreliable, or the build
  matrix is worse than shipping wheels.
- Product velocity matters more than architecture for the next two quarters. A
  strangler migration is months of work that ships no user-visible feature.

## Phase 0 spike

Same shape as the Phase 0 that de-risked the iced migration: throwaway code against
reality, days not weeks.

**Build:** one binary that opens an iced window, serves an axum route, and generates
tokens from a GGUF model, on Windows with GPU acceleration.

**Measure:** binary size, cold start, idle RSS, tok/s against the same model under
the current Ollama path.

**Confirm:** tray and hidden-window behavior from the ADR 0005 spike still hold when
iced shares a runtime with axum; and that streaming tokens reach the UI without a
frame-rate cost.

**Kill criteria:** GPU bindings unreliable or unbuildable on Windows; iced and axum
cannot share a tokio runtime without fighting; tok/s materially below the Ollama
baseline.

## Open questions

- Does iced 0.14 share a tokio runtime with axum cleanly, or does its executor need
  bridging? Unverified — first thing the spike answers.
- Which binding: `llama-cpp-2` (thin, tracks upstream llama.cpp) or `mistral.rs`
  (more batteries, smaller community)?
- Does the axum surface stay byte-compatible with `/api/v1` so existing API clients
  and the contract enum generator (`scripts/sync_contract_enums.py`) keep working
  through the migration?
- Where do Alembic migrations land — port to `sqlx` migrations, or keep the Python
  tool for schema and let Rust only read?
