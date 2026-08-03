# 5. Native iced desktop app, headless server

Date: 2026-08-03

## Status

Accepted. Supersedes the UI sections of [ADR 0004](0004-desktop-shell-tauri-python-sidecar.md)
(webview, `web/` payload, `window.__AGENT_PLATFORM__` handoff, free port, CORS) and
[ADR 0002](0002-ui-stack-react-typescript-vite.md) (the React/Vite frontend is deleted).
The rest of 0004 — Python server owned as a child process, loopback is not a security boundary,
per-install key, training kept out of the bundle — still holds.

## Context

ADR 0004 shipped a Tauri shell pointed at the server's `/app/` mount. That kept one frontend, but
it also kept two UIs alive in practice: the React SPA and the Jinja pages (`/config`, `/ui`,
`/api-guide`), each with its own idea of settings and docs. Every feature landed twice — once in
`web/`, once natively — and the webview inherited a browser's constraints: nothing on screen until
the server answers, `EventSource` unable to carry `Authorization`, cross-origin rules and a
`CORS_ALLOW_ORIGINS` handshake between two things that ship as one product.

The local case (the one LM Studio and Ollama set expectations for) wants the opposite: a window
that exists before the server does, a tray-resident background API on a stable port, and no
browser in the loop.

## Decision

The desktop app is a native Rust [iced](https://iced.rs) application and the **only** UI. The
server is headless: a JSON API under `/api/v1`, plus `/docs` and the `/tokens` dashboard.

- **Workspace** `desktop/` with two crates. `crates/client` is a headless, unit-tested API client
  (types mirrored from the server contract; `scripts/sync_contract_enums.py` emits `enums_gen.rs`
  from `app/shared_enums.py`). `crates/app` is the `iced::daemon` UI, which survives having zero
  windows — closing the window hides to tray while the server keeps serving.
- **Fixed port 18410** (settings-file and `AGENT_PLATFORM_PORT` overridable), with
  attach-if-running: a healthy server that accepts this install's key is adopted instead of
  spawning a second one. A foreign server on the port is reported, not attached to — a Docker
  port-forward of the same image would otherwise point the UI at another database.
- **UI vocabulary** is a native port of shadcn/ui's design language (zinc tokens, 0.5rem radius,
  Tailwind type/space scale) in `crates/app/src/ui/`; screens compose kit functions and never style
  raw widgets. Theme follows the OS by default.
- **Deleted, not ported**: `web/`, `desktop/src-tauri/`, the pixel-office canvas, the Jinja
  `/config`, `/ui` and `/api-guide` pages, the SPA mount, the CORS middleware, and the bare-root
  router mirror. `/tokens` survives as the only token dashboard.
- **Packaging**: `scripts/bundle_server.py` stages the relocatable CPython runtime and server
  source next to the cargo exe; `desktop/installer/agent-platform.iss` (Inno Setup) produces a
  per-user Windows install.

## Consequences

- **Docker and cloud are API-only.** There is no browser UI at any deploy target. This was the
  explicit trade for deleting the duplicate frontend; operators use the API, `/docs`, and the
  desktop app pointed at a remote origin.
- **Breaking for API clients**: the bare-root paths (`/processes`, `/teams`, `/projects`,
  `/workspaces`, `/me/workspace`, `/workspace`, `/actions`, `/api-tokens`) are gone; prefix with
  `/api/v1`.
- **Accessibility regresses.** iced exposes no accessibility tree, unlike the webview it replaces.
  Keyboard navigation is the only mitigation, and it is thin.
- **Text input is basic** (iced `text_input`/`text_editor`, immature IME). Acceptable because every
  input is plain short text; a CJK-heavy user would notice.
- SSE is a `bytes_stream()` reader that sends `Authorization`, so the auth bug the webview lived
  with is gone. Frames trigger refetches; payloads are ignored except terminal/error.
- Windows only, in practice: the Rust branches on platform, but icons, packaging, signing and
  notarization for macOS/Linux are unimplemented.
- Native file dialogs (`rfd`) and job-completion toasts (`notify-rust`) — listed as "possible, not
  wired" in 0004 — are now wired.
