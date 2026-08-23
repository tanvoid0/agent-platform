# CLAUDE.md — desktop (Rust)

Root [`CLAUDE.md`](../CLAUDE.md) and [`plan.md`](../plan.md) still apply. This file
is the Rust side's hard-won detail: things that cost hours to learn and are not
visible from the code.

## Working here

- Search `desktop/crates`. Never glob the repo root or `desktop/target` — the
  target dir is enormous and will drown any search.
- Tests: `cd desktop && cargo test -p agent-platform-client -p agent-platform-desktop`.
- Build needs cmake + libclang (whisper.cpp/llama.cpp via cmake). Paths are set
  per-machine in [`.cargo/config.toml`](.cargo/config.toml) `[env]` — a PATH cmake
  wins when that file is absent.
- Crate split: `client` = HTTP + SSE + generated enums + DAG validation,
  `app` = the iced application, `server` = `agent-platformd` (see ADR 0007).
- Screens are `x.rs` (state + `update`) and `x_view.rs` (render). Nav wiring lives
  in `screen.rs`/`main.rs` — coordinate edits there, it is a shared choke point.
- The index on this repo often holds someone else's half-finished work. Commit
  with an explicit pathspec (`git commit -- <paths>`) rather than trusting it.

## UI kit

`crates/app/src/ui/` is a shadcn/ui port — exact zinc HSL tokens, 0.5rem radius,
Tailwind type/space scale. **Screens compose kit functions and never style raw
widgets.** Raw colors are legitimate only in the HUD canvas and the DAG graph.
`ui::count(n, singular, plural)` exists because "1 steps"-class strings kept
shipping; explicit plurals, not guessed ones.

## iced 0.14 gotchas

- **`iced::exit()` hangs on Windows** (wgpu teardown, ~37 threads). Quit is:
  `drop(state.tray.take())` → kill the Python child → `std::process::exit(0)`.
- **`container::max_height` does not clamp a `Length::Fill` child.** Measured
  identical to uncapped. Bound such a child by its `height`, not by a max.
- **Scrollbars float over content** and clip card edges and trailing timestamps —
  `.spacing(space::SM)` on the scrollable. Never nest a `ui::page` inside another
  scrollable; use `ui::page_fixed` (that bug gave Logs two scrollbars).
- The HUD canvas paints a **fixed dark palette**, so it needs
  `theme::hud_backdrop` and a fixed height — letting it `Fill` turns a light
  theme into a black slab.
- Tray must be created in `boot` (the winit thread) and held in `State`; poll
  `MenuEvent::receiver()` from a `Subscription::run` stream. Dropping the tray
  before exit matters, or the icon lingers until hover.
- `iced::daemon(boot, update, view)` survives zero windows; reopen with
  `window::open`.

## Local inference (`local-llm` feature, off by default; `cuda` implies it)

`whisper-rs-sys` and `llama-cpp-sys-2` each statically link their own ggml →
hundreds of MSVC `LNK2005` + `LNK1169`. The fix is `llama-cpp-2/dynamic-link`
(llama + ggml as DLLs), and then `build.rs` **must** copy those DLLs out of the
sys crate's `OUT_DIR` to beside the exe *and* into `deps/`, or everything dies
with `0xC0000135`. The installer does not ship them yet.

Model-backed check: `AGENT_PLATFORM_TEST_GGUF=<path> cargo test --features cuda -- --ignored`.
Windows CUDA: a failed configure poisons the cmake cache
(`cargo clean --release -p llama-cpp-sys-2`), MSBuild reads `CUDA_PATH_V13_3` not
`CUDA_PATH`, and CUDA 13 moved redist DLLs to `bin\x64`.

## Runtime contract

- The app spawns the daemon from its own directory, so **build both**.
- Attach-if-running probes unauthenticated `/api/v1/system/status` first
  (open loopback, ADR 0013), then the leftover install key. A foreign keyed
  server on 18410 is still `Foreign`. An open agent-platform on the port is
  treated as ours — one listener, like Ollama.
- Data lives in `%APPDATA%\com.tanvoid0.agentplatform` — SQLite, workspaces,
  `settings.json`. The optional cloud session is `cloud.session.json` in that
  dir (ADR 0013). A leftover `master.key` is only used when attaching to an
  older keyed daemon; new spawns leave the local API open like Ollama.
- **Every app-state file is rewritten whole, so write it with
  `shell::write_atomic`, never `fs::write`.** `settings.json`, `chats.json`,
  `memories.json` and `master.key` all load with a silent fallback to a default,
  so a truncated file is not an error the user sees — it is their settings or
  their whole chat history quietly gone. Quit is `std::process::exit(0)`, which
  does not wait for a save in flight.
- SSE: the terminal sentinel is only sent when there is no backlog, so consumers
  gate on polled status. A sentinel is told apart from a log-row `"error"` by its
  missing `timestamp`/`task_id`.

## Seeing the UI

Screenshot it and Read the PNG: Win32 `CopyFromScreen` by process name from a
PowerShell script in the session scratchpad. `PrintWindow` returns blank for
wgpu-composited windows, and computer-use's `request_access` cannot match this
app. Screenshots alone lie about color — sample pixels (`Bitmap.GetPixel`)
against a stashed baseline build before believing a visual regression.

**Driving it: `SetForegroundWindow` fails silently from a background process.**
It returns `true` and the window stays behind, so `SetCursorPos` + `mouse_event`
click whatever is actually in front — the app looks like it ignored the button.
Tap ALT (`keybd_event 0x12` down/up) to release the foreground lock, then set,
then **verify with `GetForegroundWindow` and refuse to click if it did not
take**. Half an hour went into "the header buttons are dead" before the clicker
was the thing that was dead; a click that cannot land must fail loudly. Note also
that `GetWindowRect` is the origin for both the screenshot and the click, so
screenshot pixel == click offset regardless of the Win11 invisible border.
