# 20. Operator UI moves to portal_desktop; server runs as its sidecar

Date: 2026-11-04

## Status

**Proposed.** Reverses the UI decision of
[ADR 0005](0005-native-iced-desktop-headless-server.md) (iced is the only UI)
and reopens the shape [ADR 0004](0004-desktop-shell-tauri-python-sidecar.md)
proposed (a desktop shell owning the server as a child process) — on different
grounds, and with a shell that already exists in another repository rather than
a new one built here.

Nothing in this document is implemented. It records the decision and what it
retires, for acceptance before any code moves.

## Context

Two products grew up next to each other:

- **agent-platform** — the backend is the product (a local AI provider: the
  Rust `agent-platform-server` crate = `agent-platformd`, an OpenAI-compatible
  proxy plus orchestration API on `:18410`, shipped headless in Docker since
  [ADR 0007](0007-strangler-rust-server.md)). Its operator UI is the native
  **iced** app in `desktop/crates/app` ([ADR 0005](0005-native-iced-desktop-headless-server.md)).
- **portal_desktop** (separate repo) — a **Tauri 2 + SvelteKit** developer
  desktop: terminal, Coder workspace, dev tools, AI. It is **already an HTTP
  client of `:18410`** — its README requires `agent-platform` running in a
  second terminal, and its `src-tauri` is already an axum + tokio +
  reqwest-rustls host (same async stack and versions as the server).

The friction the owner stated: the iced UI "isn't doing the most possible
configuration" (0005 itself conceded accessibility, basic text input/IME, and
Windows-only packaging as its costs), while portal_desktop has a richer
shadcn-based UI and would be "boosted" by surfacing the platform's own
features. And running the backend as a second thing beside portal_desktop
**feels like duplication** — two apps, two launches, and a config UI built
twice (iced and Svelte).

### Why 0005 left the webview, re-examined

0005 moved off 0004's Tauri webview for four reasons. Each has either been
resolved by what changed since, or is now an acceptable trade:

| 0005's reason to leave webview | Status now |
|---|---|
| Two UIs alive (React SPA + Jinja pages), every feature landed twice | Gone. The Jinja pages and the React SPA were deleted in 0005/0007. There is one webview UI candidate now — portal's Svelte — and this ADR retires iced so it stays one. The doubling recurs **only during migration** (iced and Svelte coexist until iced is cut); it is a transition cost, not the end state. |
| Blank screen until the server answers | Gone. portal_desktop is a full Tauri app with its own Rust side and local SQLite (sea-orm); it renders instantly and shows platform data once the sidecar is up, rather than a blank webview waiting on `/app`. |
| `EventSource` cannot carry `Authorization` | Moot. portal is a Tauri app, not a browser tab: it reads SSE/ws through reqwest / `tokio-tungstenite` with headers it controls, the same way 0005's iced client does (`bytes_stream()` + `Authorization`). |
| CORS handshake between two things that ship as one product | Mostly moot. The sidecar is on loopback and portal already reaches it today without CORS being the blocker; requests that need it can route through portal's Rust side. |

What 0004 established and this ADR **inherits unchanged**: the server is owned
as a **child process**, loopback is **not** a security boundary so the server
runs with auth against a **per-install 256-bit key**, fixed port **18410 with
attach-if-running** (0005), and training stays **out of the bundle**. The
"rewrite the server in Rust buys nothing" caveat from 0004 is spent — 0007
already made it Rust for other reasons.

## Decision

The operator UI for agent-platform becomes **portal_desktop**. `agent-platformd`
runs as portal_desktop's **managed sidecar** — spawned on launch, killed on
close — exactly the child-process lifetime 0004 chose. The iced app retires.

- **One binary, two run modes.** `agent-platformd` stays a standalone headless
  build (Docker, cloud, and a bare local AI provider for other tools). The
  artifact portal ships as its Tauri sidecar is the **same binary from the same
  crate** — so bundling the backend with portal is *not* a second copy; it is
  one build target with two ways of being started.
- **Sidecar, not crate-link.** portal depends on `agent-platformd` as an
  `externalBin`, not by linking `agent-platform-server` into portal's own
  binary. Reasons: (1) linking the crate would pull the server's ~1100-dep tree
  into portal's build; the sidecar keeps the two builds independent. (2) a
  single linked binary fights the standalone-provider goal — the sidecar *is*
  the standalone provider. (3) portal is already an HTTP client of `:18410`, so
  the integration surface is HTTP that already exists.
- **Honor attach-if-running (0005).** On launch portal adopts a healthy server
  that accepts this install's key instead of spawning a second one, and reports
  a foreign server on the port rather than attaching (a Docker port-forward of
  the same image points at another database). portal must not blindly spawn.
- **Retire in agent-platform:** `desktop/crates/app` (iced UI),
  `desktop/crates/client` (the Rust API client that existed for iced), and
  `desktop/installer/` (portal has its own updater and installer). **Keep:**
  `desktop/crates/server` (→ `agent-platformd`), `desktop/crates/local-llm`,
  and `worker/` (training).
- **Config/operator screens are ported, not bridged.** The screens worth
  keeping from iced (providers, model-ops, teams, runs) are rebuilt in portal's
  Svelte against the existing `/api/v1` contract and `/openapi.json`. The iced
  UI being incomplete is why this is low loss.

### Out of scope

**agent-monitor does not merge.** It is Node + SQLite + React, local-first, and
monitors *Claude Code* (an external tool) rather than this platform — different
language, different data source, and its standalone independence is a feature.
It may later dock into portal as a webview section; its code stays its own.

## Consequences

- **The portfolio stance in the README changes.** "The operator UI is a native
  desktop app (iced)" becomes "the operator UI is portal_desktop; Docker/cloud
  stay API-only." agent-platform's README and a counterpart ADR in the
  portal_desktop repo need updating.
- **Cross-repo coupling.** The operator UI now lives in a different repo from
  the server. The seam between them is the versioned HTTP contract
  (`/api/v1`, `/openapi.json`) — the same contract 0005's `crates/client`
  mirrored. portal pins a server version it ships as its sidecar; the headless
  binary can still run ahead for API clients.
- **Release.** `agent-platform-server` and `-desktop` ship lockstep on one tag
  today. That lockstep moves to portal's release: portal tags carry the
  `agent-platformd` version they bundle. agent-platform still tags its own
  headless releases for Docker/cloud.
- **Accessibility and text input recover.** The webview restores the a11y tree
  and real inputs/IME that 0005 listed as regressions (the "CJK-heavy user would
  notice" case).
- **Packaging.** portal builds `agent-platformd` per target and drops it in as a
  Tauri sidecar. macOS/Linux signing/notarization — unfinished for the iced app
  — is already portal_desktop's concern, so this consolidates rather than adds.
- **Transition window.** Until the ported screens cover what the owner uses,
  iced and Svelte both exist — the one doubling this ADR otherwise removes.
  Cut iced once the Svelte screens reach parity on the screens actually used,
  not before.
- **Reversible.** The sidecar boundary is a process boundary over HTTP. If this
  proves wrong, `agent-platformd` is untouched and the iced app can be revived
  from history; nothing in the server was forked for the merge.
