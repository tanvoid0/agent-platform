# 13. Desktop: open loopback, optional cloud account

Date: 2026-08-23

## Status

Accepted; the open-loopback half superseded by
[ADR 0019](0019-key-the-local-api.md) on 2026-08-31. The desktop spawn now
passes the install key. Everything below about the cloud account, the session
file, and provider `platform` still holds.

## Context

The iced app always spawned `agent-platformd` with a per-install `master.key`
and sent it on every request. That came from [ADR 0004](0004-desktop-shell-tauri-python-sidecar.md)
("loopback is not a security boundary") and made the local API a special case
against Ollama, LM Studio, and ComfyUI: those answer on loopback with no token,
and other apps on the machine just point at the origin.

The same process is also the cloud artifact ([ADR 0007](0007-strangler-rust-server.md)).
Hosted inference has to know *who* is calling: magic-link JWT, entitlements,
Stripe. Other apps talking to the public URL need a token (`agp_…` or a user
session). That is not a reason to make the person in front of the desktop sign
in to use SQLite, local llama-server, or a pasted OpenAI key.

Two questions, one decision:

1. Should the desktop's local server require a bearer token?
2. How does the same window reach hosted models without becoming a thin client
   of cloud Postgres?

## Decision

**Local loopback is open, like Ollama.** The desktop spawn sets
`AGENT_PLATFORM_MASTER_KEY` to empty so a developer `.env` cannot re-arm it.
`host_guard` stays (DNS rebinding). Binding off-loopback still requires a
master key. Other local apps call `http://127.0.0.1:18410` with no
`Authorization` header.

A leftover keyed daemon (older install still holding the port) is still
adopted: attach-if-running tries unauthenticated `/api/v1/system/status`
first, then the install key. An open server on the port is treated as ours —
one listener, same as Ollama.

**Cloud is a second credential, not a second database.** Sign-in does not
retarget the iced `Client` at `AGENT_PLATFORM_PUBLIC_URL`. Local projects,
chats, coder, and SQLite stay on the daemon this app spawned. The Account
card (Settings) magic-links against the cloud origin; the session is a JSON
file next to `settings.json`. The local daemon reads that file as provider
`platform` and forwards `/v1` with the user JWT. Entitlement is enforced on
the cloud process, not on local routes.

Store apps (`portal-desktop`, …) keep the same Portal account. The iced app's
public id is `agent-platform-desktop`.

## Consequences

- Settings → API / Status copy: no token required on this machine. The
  install key is only used when attaching to a leftover keyed daemon.
- First hosted-model use is Settings → Account, not a login wall at launch.
- Local BYOK (OpenAI/Anthropic/Gemini keys, Ollama) never needs an account.
- Coder `run_command` is reachable by other processes on this machine, the
  same class of risk as an unauthenticated Ollama. It is not reachable from
  the browser (host_guard) or the LAN (loopback bind).
- Refresh-token rotation is owned by the daemon once the session file exists,
  so the desktop Account card must not refresh in parallel.
