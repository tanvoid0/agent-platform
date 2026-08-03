# 4. Desktop shell: Tauri around the unmodified Python server

Date: 2026-08-02

## Status

Accepted

## Context

The platform is used two ways that pull in opposite directions. It is a multi-tenant server
(workspace tokens, `client_id` scoping, Docker and cloud deploys), and it is also the thing a
single developer runs on their own machine to drive local models, watch runs, and train adapters.
The second use is currently the worse experience: two processes, two ports, CORS between them, a
browser tab that looks like any other tab, and a Bearer token to paste.

LM Studio is the reference for how the local case should feel. The obvious reading of that — "make
it a native app for performance" — does not survive contact with the numbers. Tauri renders in
WebView2 on Windows, the same engine the browser uses, so the UI is frame-for-frame identical.
Inference throughput is set by the engine, quantization and VRAM, none of which care what process
owns the window. Rewriting the server in Rust or C++ was considered and rejected for the same
reason: the server is I/O-bound (`httpx` to an upstream, SSE relay), per-request framework overhead
is a rounding error next to model latency, and the training pipeline is torch/transformers and
cannot leave Python regardless.

What a desktop shell does buy is real, just not throughput: one launchable thing, a tray, native
file dialogs for model and dataset paths, notification when a long training job finishes, and an
installer. Those are worth having, and none of them require the server to change.

Three properties of the existing code made this cheap, and are the reason the decision is safe:

- `app/requirements.txt` has no torch. The shippable server is small.
- Every torch/transformers import is lazy, inside functions in `model_ops/pipeline/*`.
- GPU stages already run out-of-process (`MODEL_OPS_GPU_SUBPROCESS`, default on), so the API
  process never loads the training stack.

## Decision

Ship a Tauri 2 shell in `desktop/` that owns the lifetime of the existing Python server as a child
process. The server is not forked, wrapped, or reimplemented; everything desktop-specific is passed
as environment at spawn time.

- **UI**: the existing `web/` build, unchanged. The same `pnpm build` artifact is staged into the
  payload, mounted by FastAPI at `/app`, and served by nginx in Docker. The shell points its webview
  at the server's `/app/` rather than serving the files itself: the build is compiled for that base
  (`base: "/app/"`, `basename="/app"`), so it only resolves there — and loading it from the server
  makes the webview same-origin with the API, so the desktop needs no CORS at all.
- **Runtime**: `scripts/bundle_server.py` stages `desktop/src-tauri/payload/` with uv's managed
  CPython (python-build-standalone, relocatable — a plain `venv` is not, as it points back at a
  base interpreter the user will not have), the server source minus caches/tests/dev databases,
  and `scripts/start.py` as the entrypoint.
- **Handoff**: the port is chosen free at launch and the API key is generated on first run, so
  neither can be baked into the bundle. The shell injects both as `window.__AGENT_PLATFORM__`
  before the page loads; `web/src/api/client.ts` prefers them over its build-time env.
- **Training**: stays out of the bundle. `MODEL_OPS_PYTHON` points GPU stages at a second
  environment installed on demand, so the installer does not carry multi-GB CUDA wheels.

Loopback is treated as **not** a security boundary. Any local process, and any web page the user
has open, can reach `127.0.0.1`. The desktop therefore runs with auth enabled against a 256-bit
per-install key stored in the app config dir, which the shell hands to its own webview. The user
never sees or types a token, and desktop stops being a special case against Docker and cloud.

## Consequences

- One server, one frontend, three launch profiles. The differences are entirely environment:

  | | Desktop | Docker | Cloud |
  |---|---|---|---|
  | Bind | `127.0.0.1`, free port | container `0.0.0.0` | `0.0.0.0` |
  | Auth | per-install key, auto | master key | master key + workspace tokens |
  | Data | `%APPDATA%`/XDG | volume | volume / Postgres |
  | UI | FastAPI `/app` | nginx | nginx / CDN |

- No performance claim is attached to this change. It buys ergonomics and distribution.
- The shell holds the child's stdin; `start.py --exit-with-parent` exits on EOF, so the server dies
  with the shell even when the shell is killed rather than closed.
- Windows first. macOS and Linux need their own signing and notarization setup, but the payload
  layout and the Rust already branch on platform.
- Native file dialogs and job-completion notifications are now *possible* and are not yet wired —
  the capability file grants `core:default` only.
