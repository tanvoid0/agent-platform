# portal_desktop ↔ agent-platform — review, 2026-08-08

`../../../portal/portal_desktop` (SvelteKit + Tauri 2, v0.10.1) is the *other*
client of this platform. It predates the Rust migration and its README says all
its AI features need `agent-platform` running. This is what was found reviewing
it against `agent-platformd` as it stands on `rust-server-migration`.

Three questions were asked: what can be reused here, is portal broken by the
migration, and should the two apps be one.

---

## 1. Is portal_desktop broken by the migration? — **No. Its docs are.**

Every agent-platform endpoint portal calls exists on `agent-platformd`, on the
same port, with the same auth. Verified live against a running daemon rather
than read off the source:

| Endpoint portal calls | Where in portal | Live result |
|---|---|---|
| `GET /v1/catalog` | `domains/ai/catalog.rs` | **200** |
| `GET /v1/models` | `domains/ai/providers/agent_platform_provider.rs` | **200** |
| `POST /v1/chat/completions` | same | route present |
| `GET /v1/health/readiness` | `domains/ai/commands.rs` | **200** |
| `GET /health` | (desktop adopt check) | **200** |
| `POST /api/v1/coder/chat/{stream,retry,approve,tool-result}` | `domains/coder/service.rs` | routes present |
| `GET /api/v1/coder/chat/{threads,thread,context-usage}` | same | **200** |
| `GET,POST /api/v1/teams/` | `domains/disk/verify.rs` | **200** |
| `GET,POST /api/v1/processes`, `/{id}` | `domains/coder/service.rs`, `disk/verify.rs` | **400** (needs a `project_id`/`client_id` filter — correct behaviour) |

Auth matches too: portal sends `Authorization: Bearer <token>` plus
`X-Agent-Platform-Client: portal-desktop`, which is exactly what `auth.rs`
resolves and what `coder_tools::is_portal_desktop_client` keys tool delegation
off. Base URL default `http://127.0.0.1:18410` is unchanged
(`domains/ai/platform_config.rs`).

**Not agent-platform's API:** `automation_service.rs` calls `/api/v1/health`,
`/api/v1/workflows` and `/api/v1/executions/{id}` — that service's `base_url` is
an **n8n** instance, not this platform. It looked like a break in a path sweep;
it is not one.

### What actually breaks a new user

The Python server served browser pages. They are gone
([ADR 0005](adr/0005-native-iced-desktop-headless-server.md),
[ADR 0007](adr/0007-strangler-rust-server.md)) and portal still sends people to
them. A user following portal's own instructions cannot configure a provider or
mint a token, which reads as "portal doesn't work with the new platform".

| Portal says | Files | Reality |
|---|---|---|
| Configure providers at `http://127.0.0.1:18410/config` | `README.md:—`, `docs/getting-started/AGENT_PLATFORM.md:39,46`, `.github/actions/build-release/action.yml:70` | **404.** No `/config` route exists. |
| Mint a workspace token at `/tokens` | `docs/getting-started/AGENT_PLATFORM.md:49` | **404.** Minting is `POST /api/v1/workspaces/{workspace_id}/api-tokens/` (master key), or the agent-platform desktop app. |
| Confirm `http://127.0.0.1:18410/docs` loads | `docs/getting-started/AGENT_PLATFORM.md:76` | **404.** The spec is `GET /openapi.json`; there is no Swagger UI. |

`pnpm install && pnpm dev:server` **is still correct** — `package.json` here
kept `dev:server`, now `cargo run -p agent-platform-server` behind a
`kill-port`. Portal's quick-start command does not need changing.

Replacements to point at:

- **Providers / models** — the agent-platform desktop app's *Providers* screen,
  or over HTTP: `GET,POST /api/v1/llm-proxy/env` and
  `GET,POST /api/v1/llm-proxy/config-yaml`, with `GET /api/v1/llm-proxy/ui/providers`
  for the picker data. Portal could grow its own provider editor from those
  three routes without a browser at all.
- **Tokens** — `POST /api/v1/workspaces/{workspace_id}/api-tokens/`.
- **Spec** — `GET /openapi.json`.

### One dead field — now honoured (2026-08-08)

`coder/service.rs` puts `"tools": tool_specs` in the `/coder/chat/stream` body —
a mode-filtered list including its multitask and `delegate_task` specs.
`SendRequest` in `coder.rs` has no `tools` field and `coder_loop.rs:308` always
sends this crate's own six specs upstream. So portal's extra tools never reach
the model.

**This was not a migration regression** — `git show e79cfbb~1:app/coder/routes.py`
shows the Python `CoderChatSendRequest` had no `tools` field either. It had
always been ignored.

**Resolved by honouring it.** `SendRequest` now takes `tools`, `TurnOptions`
carries it, and `call_llm_step` sends the caller's list when there is one.
Absent is the default six specs; `[]` is a tool-free turn; a non-empty list is
the caller's, verbatim. Capped at 64 entries / 64 KB and required to be objects,
because it goes straight into an upstream request body. Portal's multitask and
`delegate_task` specs now reach the model with no portal-side change at all —
it has been sending them the whole time. Contract in
[`coder-delegation-protocol.md`](coder-delegation-protocol.md).

---

## 2. What is worth reusing here

### 2a. Release pipeline + auto-update — the real gap, now closed

> **Built 2026-08-08.** One tag, `v<version>`, one release, both artifacts:
> `dist-workspace.toml` + the generated `.github/workflows/release.yml` for the
> daemon on four platforms, with `release-desktop.yml` folded in as a `dist`
> custom job for the Windows app zip. The app got the check half of an updater
> (`update_check.rs`); the daemon got `dist`'s real one. Details and the
> reasoning in `plan.md` → **Releases**. What follows is the review that led
> there.


This repo has CI as of 2026-08-08 and **nothing else**: no release workflow, no
packaged artifact, no updater. `desktop/` builds and tests; it never ships. That
is the same hole the backlog calls "macOS/Linux packaging".

Portal has the whole thing, and the *shape* is what ports:

- `release.yml` — tag-triggered (`v*.*.*`) plus `workflow_dispatch`, and a
  **cheap single-runner smoke job gates the four platform builds**. Failing fast
  on one ubuntu runner before starting a Windows + two macOS + Linux matrix is
  the bit worth copying verbatim.
- `.github/actions/build-release/action.yml` — the per-platform steps as a
  composite action, so the matrix stays four lines.
- `fail-fast: false` on the matrix — one platform's break should not cancel
  three good builds.
- Signed updater artifacts: `bundle.createUpdaterArtifacts`, minisign keypair in
  secrets, `latest.json` assembled across jobs, and the app checking
  `releases/latest/download/latest.json` on start
  (`src-tauri/src/domains/updates/`, `src/lib/domains/updates/`).

**What does not port:** `tauri-action` and `@tauri-apps/plugin-updater` are
Tauri-only. An iced binary needs either `cargo-dist` (which generates this exact
workflow shape plus an installer and a `dist-manifest.json`) or a hand-rolled
matrix over `softprops/action-gh-release`, with `self_update`/`axoupdater` for
the in-app half. The design decisions above are stack-independent; the tools are
not.

### 2b. CI — two cheap borrows

- **`concurrency: { group: ci-${{ github.ref }}, cancel-in-progress: true }`.**
  Portal's CI has it; ours does not, so a quick second push runs both to
  completion on a matrix that includes a Windows whisper.cpp build.
- **A blocking grep guard.** Portal fails the build on leftover debug telemetry
  in `src/`. The equivalent here already exists as
  `scripts/check_repo_hygiene.py` and *is* wired up, so this one is already
  covered — noted so it is not re-invented.

Not worth borrowing: portal gates `pnpm check` and `pnpm lint` but runs clippy
`continue-on-error`. Ours does the same thing for the same reason.

### 2c. Coder UI

Portal's Coder is 31 Svelte components / ~5.0k lines; ours is ~4.3k lines of
iced across eight modules. Neither is behind. **No code ports** — Svelte to iced
is a rewrite, and `plan.md`'s hearth section already settled that Monaco, xterm
and preview iframes have no native path.

What is genuinely in portal and not here, as *ideas*:

- **Agent mode and permission mode are two separate pickers**
  (`CoderAgentModeSelector` + `CoderPermissionModeSelector`, `config/agentModes.ts`,
  `config/permissionModes.ts`). Here `allow_commands` and `auto_approve_commands`
  are two switches in a wrapped header row. Portal's split — *what tools the mode
  allows* vs *how much runs unattended* — is the clearer model, and it is what
  makes `auto_approve_commands` a user decision rather than the hardcoded
  `false` it is here.
- **Multitask / sub-agent surfacing** — `CoderMultitaskBar`,
  `CoderSubAgentCard`, `CoderSubAgentInline`, and the coordinator/sub-agent
  thread kinds behind them. This platform's `delegate_task` starts a real
  process; the iced screen has no surface for a fan-out.
- **`CoderActivitySummary`** — a per-turn roll-up above the transcript, distinct
  from our status line, which names one wait at a time.

What is here and not in portal: `.agent/notes.md` carried into every turn,
`.agent/git` checkpoints with a timeline and restore, a real
`alacritty_terminal` emulator, `repo_map`. Portal has a git *changes* panel and
a commit dialog against the user's own repo — a different thing from a
checkpoint store, and arguably both are wanted.

The honest recommendation is **not** to port UI in either direction. It is to
stop having two Coder *clients* of the same server loop diverge in what they
render for the same SSE frames — see the next section.

---

## 3. One app or two?

**Two apps. Do not merge.** The reasoning, in the order it matters:

1. **The merge was already decided against, twice, and documented.**
   [ADR 0005](adr/0005-native-iced-desktop-headless-server.md) and
   [ADR 0007](adr/0007-strangler-rust-server.md) deleted the web frontend and
   the Tauri shell from this repo on purpose. Merging portal in means
   re-adopting a webview stack that was removed by decision, not by neglect.
2. **The stacks cannot share a line of UI.** Svelte/Tauri renders in a webview;
   iced draws with wgpu and has no webview. Every screen would be written twice
   no matter which repo it lived in.
3. **Most of portal is not about this platform.** SDK management, Kubernetes
   navigation, deployments, credentials, package managers, GitHub browsing,
   cloud, terminal — roughly two dozen domains, of which four touch
   agent-platform. Merging drags all of it into a repo whose CI is a Rust
   workspace.
4. **They are not the same kind of thing.** The iced app *is* the platform's own
   UI and it spawns `agent-platformd`. Portal is a client that assumes the
   daemon is someone else's problem. Merging makes one binary that both is and
   is not the server.

**The complementary split, stated plainly:** portal has a webview, so it is the
one that can ever have Monaco, a preview iframe and xterm.js — the three things
`plan.md` item 6 closed as impossible here. The iced app is the platform's
operations surface (providers, model ops, processes, tokens, logs, plans,
assistant) plus a coding agent good enough to not need a second window. Let
portal be the IDE-grade coder, let this be the platform console, and stop
scoring them against each other.

**What that costs, and the one thing to actually fix:** two clients render the
same coder SSE frames, and they must not disagree about what a frame means. The
contract is the fix, not the merge:

- `openapi.json` is hand-maintained and drifts silently — already in the
  deployment-hardening backlog, and now it has a second consumer, which raises
  it from tidiness to correctness.
- The delegation protocol (`tool_call` frame → `POST /coder/chat/tool-result`,
  300s park) is documented in `plan.md` prose and in nothing a portal
  contributor would read. It belongs in `docs/`.
- Decide the `tools` field (§1). A caller-supplied tool list is exactly the kind
  of thing two divergent clients need.

---

## Recommended order

All five landed on 2026-08-08:

1. ~~**Fix portal's dead URLs**~~ — `/config`, `/tokens` and `/docs` in
   `docs/getting-started/AGENT_PLATFORM.md` and the release-notes template in
   `.github/actions/build-release/action.yml`, replaced with the llm-proxy
   routes, an `api-tokens` curl, and `/health`.
2. ~~**`concurrency` block on this repo's `ci.yml`.**~~
3. ~~**Document the delegation protocol**~~ —
   [`coder-delegation-protocol.md`](coder-delegation-protocol.md).
4. ~~**Decide `tools`**~~ — honoured in `SendRequest`. §1 above.
5. ~~**Release + updater**~~ — §2a above.

Still open, and the reason this document exists:

- **`openapi.json` drifts by hand.** A drift test landed the same day
  (`tests/openapi_drift.rs`) but it checks that documented operations reach a
  handler, not that request *schemas* match — `tools` is a new field on
  `SendRequest` and nothing would notice if the spec never learned about it.
  With two clients, the spec is the contract.
- **The iced app's updater cannot install**, only notice. It needs a published
  `v*` tag to test a download against.
