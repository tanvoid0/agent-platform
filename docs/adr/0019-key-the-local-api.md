# 19. Key the local API

Date: 2026-08-31

## Status

Accepted. Supersedes the "open loopback" half of
[ADR 0013](0013-desktop-local-open-cloud-account.md); the cloud-account half of
that ADR stands unchanged.

## Context

ADR 0013 spawned `agent-platformd` with `AGENT_PLATFORM_MASTER_KEY` empty so the
local API answered on loopback with no token, matching Ollama, LM Studio and
ComfyUI. The argument was ergonomic: other apps on the machine should be able to
point at `http://127.0.0.1:18410` the way they point at Ollama, and the person in
front of the desktop should not sign in to use SQLite.

That argument holds for an inference endpoint. It does not describe this process.
`agent-platformd` also serves:

- **`coder`'s `run_command`** — arbitrary command execution in a workspace, and
  workspace file read/write.
- **BYOK provider credentials** — the user's OpenAI, Anthropic and Gemini keys,
  readable through the providers routes.
- **The cloud session** — a Portal JWT the daemon forwards to hosted `/v1`, and
  whose refresh it owns.

ADR 0013 recorded the first of these as an accepted consequence, "the same class
of risk as an unauthenticated Ollama". It is not the same class. Ollama's worst
case is unmetered inference on hardware you own. This one's is any local
process — an npm postinstall script, a browser extension's native host, any
Electron app — reading the user's API keys and running commands as them. The
browser cannot reach it (`host_guard`) and the LAN cannot (loopback bind), but
neither of those is the boundary that matters here.

The friction ADR 0013 was avoiding also does not apply. The install key is
generated on first run and read from `master.key` beside `settings.json`; there
is no login wall, no prompt, and nothing for the user to configure. It was
already being generated — it was simply not being passed to the child.

## Decision

**The desktop spawns its daemon with the install key**, not with empty. Every
`/api/v1` route and the `/v1` proxy require a bearer on the local install, the
same as on the cloud one. `master.key` is the credential, and Settings → Status
and Settings → API show it for other local apps to copy.

**Attach-if-running is unchanged and still adopts an open daemon.** `port_owner`
probes unauthenticated `/api/v1/system/status` first, then the install key. A
pre-0019 daemon left on the port answers the first probe, is `Ours`, and
`client_key` sends no bearer to it. An upgrade does not need the two processes to
agree.

**Empty stays the degraded path, not the default.** If `master.key` cannot be
read, the app spawns an open server rather than one it cannot talk to. Off-loopback
binds still require a key explicitly (`AGENT_PLATFORM_ALLOW_OPEN` to opt out),
which is unchanged.

**The four vestigial `master_key.is_none()` 503 guards are gone**
(`chat`, `coder`, `coder_loop`, `assistant`, `todos`). They preserved a status
Python's HTTP self-call produced; the Rust loop calls
`llm::chat_completions` in-process with an unrestricted principal and needs no
credential. Under ADR 0013 they made chat, coder and the assistant answer 503 on
every desktop install.

## Consequences

- Settings → API / Status and the Account card no longer say "no token
  required on this machine". They point at the key.
- Another local app integrating with this server reads `master.key` from the app
  dir, or copies the key out of Settings. That is one step more than Ollama, and
  it is the step that makes the difference above.
- `GET /health` reports `auth.required = true` locally now, so a client that
  branches on it (ADR 0014) sees the same shape local and hosted — one fewer
  fork, not one more.
- The unauthenticated caller no longer exists locally, so
  `identity::machine_user` is reached through `with_machine_stamp` on the master
  key rather than through `resolve`'s open-mode branch. Same user row, same
  workspace; `Principal::unrestricted` is a superset of
  `principal_from_machine`, so no local route loses access.
- `master.key` becomes a real secret on disk. It is written with
  `shell::write_atomic` under `%APPDATA%\com.tanvoid0.agentplatform` and inherits
  that directory's permissions — no tightening beyond the user profile's own.
