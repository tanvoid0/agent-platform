# 6. In-process inference, not an in-process core

Date: 2026-08-04

## Status

**Accepted, rescoped.** The proposal this ADR opened with — one binary containing
UI, API and inference — is **rejected**. The narrow piece of it, llama.cpp linked
into the desktop binary, is **adopted** pending the [Phase 0 spike](#phase-0-spike).
The Python server keeps serving every route it serves today.

Proposed and reviewed the same day, against the tree at `69a8317`. The original
proposal is preserved below under [The shape that was proposed](#the-shape-that-was-proposed),
because the reasoning that reopened the question is still the reasoning that would
reopen it again.

[ADR 0004](0004-desktop-shell-tauri-python-sidecar.md) rejected a Rust server. This
ADR does not overturn that.

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

It is also, as the review found, **still unmeasured** — see
[finding 5](#5-the-serialization-cost-is-asserted-never-measured).

**The training argument is still true and still does not bind.** torch/transformers
cannot leave Python. But training already runs out-of-process and out-of-bundle
(`MODEL_OPS_GPU_SUBPROCESS`, default on; `Dockerfile.train`; `requirements-train.txt`
installed on the train worker only). It constrains the *training worker*, not the
*server*.

### Requirements that arrived after 0004

Four requirements reopened the question. Their status after review:

| Requirement | Status |
|---|---|
| Ship the backend embedded with the app, no Docker — the Ollama / LM Studio shape | **Already met.** The bundled CPython payload ships. See [finding 1](#1-the-distribution-requirement-is-already-met). |
| Own the local inference engine (llama.cpp / GGUF) rather than shelling out to an Ollama the user installs separately | **Live and unmet.** This ADR's decision. |
| Serve other API clients — jobs, queues, multi-agent orchestration, projects, teams | **Live, and argues against a desktop-shaped binary.** |
| The client is already Rust/iced ([ADR 0005](0005-native-iced-desktop-headless-server.md)), and already owns the server's lifetime | An enabler, not a requirement. |

The first two and the last pull toward a single native binary. The third pulls toward
a server-shaped deployment with Postgres and N workers. **These conflict**, and no
single artifact is good at both. With the first requirement satisfied by other means,
the conflict resolves in favour of leaving the server where it is.

### Measured facts

Gathered and re-verified 2026-08-04 against `69a8317`:

- `app/` is **31,365 LOC** excluding tests, across 253 files. **435 test functions**
  across 61 files.
- `app/` contains **zero** heavy AI imports. torch, numpy, transformers and sklearn
  appear only under `model_ops/pipeline/` — four files, all training. The backend is
  CRUD + HTTP proxy + orchestration.
- `app/` exposes **185 route decorators** across 17 routers. The native client
  ([crates/client](../../desktop/crates/client/src)) hits **36 distinct endpoints** —
  roughly a fifth of the surface.
- **31 Alembic migrations**; **28 `table=True` models** across 8 modules.
- [shell.rs](../../desktop/crates/app/src/shell.rs) is **569 lines** whose entire job
  is spawning, port-probing, attaching to, and reaping a child process.
- `desktop/payload/runtime/` is an embedded **CPython 3.12, 171 MB**, site-packages
  populated. `desktop/installer/agent-platform.iss` installs it per-user, no UAC.
- `whisper-rs 0.16` (whisper.cpp) is **already linked into `crates/app`** and runs
  local STT in-process ([stt.rs](../../desktop/crates/app/src/stt.rs)).
- Rust equivalents exist for nearly the whole dependency list: axum, sqlx, reqwest,
  serde, `tiktoken-rs`, `rmcp`. The one downgrade is `pymupdf` → `pdfium-render`,
  and it is confined to two files behind an existing availability guard.

## Decision

**Link llama.cpp into the desktop binary. Leave the server in Python.**

- Add `llama-cpp-2` (or `mistral.rs` — see [Open questions](#open-questions)) to
  `crates/app` as an additive dependency, alongside the `whisper-rs` already there.
- Inference runs in-process: model load/unload policy, VRAM budgeting, KV-cache reuse
  across requests, GBNF grammar-constrained output. Tokens stream straight into the
  iced UI.
- Expose it to external clients, if and when they need it, behind the existing
  `/api/v1/model-ops/ollama/*` shape, so nothing downstream notices the swap.
- Everything else stays: 185 routes, 31 migrations, the bundled CPython payload, the
  Alembic tooling, the Python training pipeline.

**Explicitly not decided here:** moving any route out of Python, adding axum to the
desktop binary, or removing [shell.rs](../../desktop/crates/app/src/shell.rs).

## The shape that was proposed

Recorded because it is what a future revisit would revive. **One binary containing
three things that are three processes today:**

- **iced UI**, calling the core through **direct function calls** — no HTTP, no
  JSON, no port.
- **axum HTTP server**, exposing that same core on a port for external API clients.
- **llama.cpp linked in**, inference in-process.

Rejected on cost against realised benefit, not on feasibility. Nothing about it is
technically blocked — every dependency has a Rust counterpart, the C++ toolchain is
proven in-tree, and iced already hosts tokio tasks. A competent team could land it.
The objection is that of the four requirements motivating it, one is already met by
the shipped payload, one argues against this shape, one is an enabler, and the only
live unmet requirement needs none of the 31,365 lines to move.

That leaves months of strangler migration whose honest payoff is an unmeasured
serialization saving and 569 lines of `shell.rs` — against the original draft's own
accurate warning that a strangler "buys nothing until it is nearly finished."

## Alternatives

**Status quo, plus the bundled CPython payload.** The winner for everything except
inference. Works today, tests green, zero migration risk, and keeps the AI ecosystem
within reach for whatever lands next. The original draft named bundled CPython as
"the cheap answer if the real pain is distribution rather than architecture" and said
to try it first. It had already shipped by the time that was written.

**Shell out to Ollama, as today.** The alternative to the decision actually taken.
Loses on the second requirement: Ollama's API exposes no VRAM policy, no multi-model
residency control, no KV-cache reuse across requests, no grammar-constrained output.
It also makes the user install and maintain a second application.

**Quarkus / JVM.** Considered because clustered Quartz, JTA and virtual threads suit
the multi-tenant half well. Rejected: `java-llama.cpp` bindings are the weakest of
the available options, and GraalVM native image plus FFI into GPU libraries is
exactly where native image stops being cheap. It loses hardest on the requirement
that motivated the question.

**Go.** What Ollama actually does (cgo → llama.cpp). Fine, but there is no Go in the
stack and Rust is already here.

## Consequences

**Good**

- The second requirement is met without a rewrite: VRAM policy, multi-model
  residency, KV-cache reuse, GBNF grammars.
- No second application for the user to install or keep current.
- Tokens reach the UI without crossing a process boundary.
- Additive: the change is contained to `crates/app`, and reverts by deleting a
  dependency.

**Bad**

- **The build matrix grows.** Per platform × per accelerator, the same matrix Ollama
  ships. `whisper-rs` already imposes part of this, but on CPU features only.
- **Binary size and build time both rise**, on top of a 171 MB payload.
- **Rust lags Python when a genuinely new AI technique lands.** Partly covered:
  GGUF/llama.cpp tracks mainstream local inference closely, remote providers are just
  HTTP+SSE (the codebase already hand-rolls `httpx` rather than using vendor SDKs),
  MCP has `rmcp`. Where it is not covered, the capability stays an optional
  out-of-process worker, as training already is — never a required runtime.

**Not addressed by this decision**

- The multi-tenant server story. Unchanged, and answered by the durable job table
  queued in [backend-perf-plan.md](../backend-perf-plan.md), not by anything here.
- UI interaction latency. Still HTTP + JSON, still unmeasured. If it turns out to
  matter, the first move is fewer or narrower polls, not a rewrite — see
  [finding 5](#5-the-serialization-cost-is-asserted-never-measured).
- `shell.rs` and the Python child process both survive.

## Phase 0 spike

Two measurements. Throwaway code against reality, days not weeks.

**Build:** `crates/app` with `llama-cpp-2` on CUDA/Vulkan feature flags, on Windows,
generating tokens from a GGUF model.

**Measure:** tok/s against the same model under the current Ollama path. Plus build
time and binary size as reporting, not as criteria — both are noise beside a 171 MB
CPython payload that ships regardless.

**Kill criteria:** accelerator feature flags will not build or are unstable on
Windows; or tok/s materially below the Ollama baseline.

Deliberately dropped from the original spike: whether iced and axum share a tokio
runtime (answered — [finding 3](#3-iced-already-hosts-tokio-tasks)), and binary size
as a decision input.

## What would reopen the full port

- Install-failure reports traceable to the bundled runtime that bundling cannot fix,
  or a signing / distribution constraint the payload cannot meet. The distribution
  trigger is otherwise spent.
- A *measured* UI-latency problem that survives fixing the poll intervals.

**What would settle it the other way**, permanently: the multi-client API becoming
the primary product, with desktop as one of its clients. That inverts the requirement
conflict, and Python-with-Postgres wins outright.

## Open questions

- Which binding: `llama-cpp-2` (thin, tracks upstream llama.cpp) or `mistral.rs`
  (more batteries, smaller community)? The spike decides.
- Does in-process inference need to appear on `/api/v1` at all, or only to the UI?
  Deferred until an external client asks for it.

Answered, kept so they are not re-asked: iced 0.14 already shares a runtime with
tokio ([finding 3](#3-iced-already-hosts-tokio-tasks)). Route byte-compatibility and
Alembic-vs-`sqlx` are moot — no route and no migration moves.

---

## Review findings — 2026-08-04

Evidence behind the rescope. Line references are to `69a8317`.

### 1. The distribution requirement is already met

`desktop/payload/runtime/` is an embedded **CPython 3.12, 171 MB**, with
site-packages fully populated (alembic, anyio, certifi, cffi, …).
`desktop/installer/agent-platform.iss` is a per-user Inno Setup installer, no UAC,
installing that payload as `{app}\server\`.
[shell.rs:303](../../desktop/crates/app/src/shell.rs) `resolve_server()` looks for
`<exe dir>\server\runtime\python.exe` and only falls back to a PATH `python` in a
repo checkout. The installer was compiled and round-tripped on 2026-08-04 per
[native-desktop-migration.md](../native-desktop-migration.md).

A user installing this app does not install Python, does not see a Python, and
cannot fail to find one. The original draft's own decide-no condition — "Bundled
CPython removes the distribution pain and nothing else hurts. **Try this first.**" —
did not merely go untried; it had already fired before the draft was written.

### 2. The ggml-on-Windows risk is mostly retired

`whisper-rs 0.16` — whisper.cpp, the same ggml upstream llama.cpp lives in — is
already a dependency of `crates/app` and already runs local STT in-process
([stt.rs](../../desktop/crates/app/src/stt.rs), with
`whisper_rs::install_logging_hooks()` at [main.rs:879](../../desktop/crates/app/src/main.rs)).
The MSVC + cmake + C++-static-lib path that usually breaks on Windows builds in this
repo, today, in this binary.

What remains unknown is narrower than "GPU bindings unreliable on Windows":
whisper-rs is pulled on **default (CPU) features**, so the accelerator feature flags
specifically — CUDA/Vulkan — are the untested part. That is what the spike measures.

### 3. iced already hosts tokio tasks

`iced = { version = "0.14", features = ["tokio"] }` sits beside a direct
`tokio = { features = ["rt-multi-thread", "time", "process"] }` in
[Cargo.toml](../../desktop/crates/app/Cargo.toml). `tokio::time::sleep` runs inside
`iced::stream::channel` at [main.rs:801](../../desktop/crates/app/src/main.rs), and
`tokio::task::spawn_blocking` is used for TTS and audio at
[assistant.rs:582](../../desktop/crates/app/src/assistant.rs). Adding an axum
listener to that runtime was never the risk worth de-risking first.

### 4. 31k LOC was the wrong denominator

185 route decorators across 17 routers; 36 endpoints consumed by the native client.
A migration motivated by *UI latency* never has to move the other four fifths, and a
migration motivated by *owning inference* has to move none of it. The original draft
priced an all-or-nothing port against a benefit that two much smaller changes deliver
separately.

### 5. The serialization cost is asserted, never measured

The draft is right that 0005 changed the *kind* of cost, and right that comparing the
loopback hop to token latency was the wrong comparison. But the replacement claim has
the same problem: nothing in the tree measures it. Meanwhile the UI polls on
`iced::time::every` at 1s (logs), 3s, 800ms while a run is live, plus model-ops
intervals — the volume is set by how often those fire and how much each refetch
pulls, not by serde's throughput. Deleting a poll is a far smaller diff than deleting
HTTP, and it is available without a rewrite. Instrument one poll-heavy screen before
believing the rewrite is the fix.

### 6. Two of the proposed wins were already collected in Python

Commit `8b452be` pooled the upstream `httpx` client and moved blocking work off the
event loop, with `test_upstream_pooling.py` and `test_auth_context_propagation.py`
guarding both against regression. "Connection pooling is default in `reqwest`" and
"no sync-DB-in-async-handler mistake to make" describe a class of bug that is now
fixed and fenced. Sunk win, not an argument for moving.

### 7. The pymupdf downgrade is smaller than billed

`pymupdf` appears in two files. [pdf_extraction.py](../../app/pdf_extraction.py)
already guards on `pymupdf_available()` and
[document_service.py:142](../../app/document_service.py) already degrades to a clear
user-facing error when it is absent. Optional capability with a working fallback, not
a load-bearing dependency.

### What held up

The draft's measured facts were close to accurate — 31,306 vs the re-counted 31,365,
and the heavy-import claim exactly right. `shell.rs` is 569 lines, not "~450"; the
draft understated its own strongest number. The requirement conflict is real and the
draft was right to lead with it. Nothing above resolves it — a desktop-shaped binary
and an N-worker server still want different artifacts.
