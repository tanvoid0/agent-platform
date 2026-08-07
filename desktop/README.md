# Desktop shell

Native Rust ([iced](https://iced.rs)) app that draws its own UI and runs the
agent-platform server beside it as a child process. That server is
`agent-platformd` (`crates/server`), also in this workspace — it was a Python
process until [ADR 0007](../docs/adr/0007-strangler-rust-server.md) finished on
2026-08-07, and the child-process contract did not change when it moved.

See [ADR 0004](../docs/adr/0004-desktop-shell-tauri-python-sidecar.md) for the
sidecar rationale (written for the earlier Tauri shell) and
[`docs/native-desktop-migration.md`](../docs/native-desktop-migration.md) for the
migration off Tauri/WebView2.

## Layout

```
desktop/
  crates/client/    HTTP + SSE client, contract enums, DAG validation
  crates/app/       the iced application
  crates/server/    agent-platformd — the API server the app spawns
```

## Run it

```bash
cd desktop && cargo run -p agent-platform-desktop
```

Opens the window and spawns `agent-platformd` on a free loopback port (or attaches to one
already running, after verifying the per-install key). The window does not wait for
`/health`: Status polls, and Logs shows the server's output from the first line, so a start
that fails is visible in the app.

## What the shell owns

| | |
|---|---|
| Port | picked free at launch, so two installs never collide; fixed for the run so a restart keeps the same origin |
| Key | 256-bit, generated on first run into the app config dir |
| Data | `%APPDATA%\com.tanvoid0.agentplatform` — SQLite, workspaces, model-ops projects, `settings.json` |
| Output | server stdout and stderr are drained into a ring buffer and shown by the Logs screen |
| Lifetime | server holds the shell's stdin pipe; EOF kills it even if the shell is killed |

The API runs with **auth on**. Loopback is not a security boundary — any local process can
reach `127.0.0.1`. The user never types the key; the shell passes it.

## Not wired yet

- Native file dialogs and completion notifications.
- Packaging: Windows installer, icon embedding, signed binary. macOS/Linux signing and
  notarization are not set up. See "Packaging" in
  [`docs/native-desktop-migration.md`](../docs/native-desktop-migration.md).
- Auto-update.

## Tests

```bash
cd desktop && cargo test
```
