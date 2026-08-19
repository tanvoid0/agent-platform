# Coder → daily driver: the plan

Written 2026-08-19, for approval before any code. Goal: make `Screen::Coder` the
place day-to-day coding happens, borrowing the best UX of Cursor (Agents
Window, plan mode ergonomics), Claude Code (checkpoint/rewind, permission
model, session management) and JetBrains Junie (plan as an approvable
document), plus Zed (review pane, follow mode) and Windsurf (plan.md as a
file, autonomy tiers) where they beat all three.

## Where the field converged (research, 2026-08)

Every serious agentic IDE now ships the same pipeline, differing only in
polish per stage:

> **editable plan gate → streaming loop with queue/steer → aggregated diff
> review → checkpoint rewind → self-verify**

The ranked feature list (top of ~15, weighted by daily pain removed):
per-hunk/per-file diff review (Zed best), prompt-level rewind (Claude Code
best), message queue with explicit queue-vs-steer (VS Code Copilot's
"Add to Queue / Steer / Stop and Send" is the clearest model), lossless
interrupt, plan-as-editable-artifact (Windsurf's `plan.md` mechanism, Junie's
document structure), live todo list, graduated autonomy with a durable command
allowlist (Claude Code's model, Windsurf's Off/Auto/Turbo as the simple UI),
AGENTS.md + @-mentions, glanceable tool rows, session fork/handoff (Amp),
parallel agents with a status board (Cursor 3), follow mode (Zed),
self-verification, second-model review pass, local↔background handoff.

Zed's **ACP** (Agent Client Protocol) is prior art for exactly our split —
server-owned loop, client-owned tools — worth reading when the frame set
grows: permission requests, diff frames, plan frames are first-class there.

## What we already have

The hearth migration left a working skeleton: server-owned loop with
delegated tools (`docs/coder-delegation-protocol.md`), six tools, a PLAN
step, shadow-git checkpoints with a diff dock tab, file tree + read-only
viewer, a real PTY terminal, a WebView2 preview pane, server-persisted
sessions that rebuild identically. What's missing is everything above the
skeleton: no stop, no queue, no editable plan, no per-file revert, no todo
list, no allowlist, no @-mentions, no edit tool, no parallel sessions.

## Constraints that shape every item

- **The wire contract is shared with portal_desktop.** Protocol changes are
  additive only, and land in `docs/coder-delegation-protocol.md` +
  `openapi.json` in the same commit. Several items below are deliberately
  **client-only** because the protocol already carries them:
  `SendRequest.tools` lets the client add its own tool specs (the delegated
  executor runs whatever the model calls), and `tools: []` already gives a
  tool-free turn — the plan gate and the todo tool need **no server change**.
- **Every `tool_call` must be answered** — new UI states (stop, steer,
  multi-session) must never drop a frame; interrupt answers the parked call
  with an error result before closing the stream.
- **Rebuild == live.** Anything rendered from the stream must render
  identically from `GET /coder/chat/thread`. New row kinds (todos, plan card)
  must either persist as ordinary messages or be derivable from them.
- **Both executors stay identical, constant for constant** — a new tool in
  `TOOL_SPECS` lands in `server/src/coder_tools.rs` *and*
  `app/src/coder_tools.rs`; the model must not tell which side ran it.
- Screens stay `x.rs` / `x_view.rs`, composing `ui/` kit. New satellite
  modules over growing `coder.rs` (already 2263 lines).
- Verified by driving it, not only `cargo test` — this screen's four worst
  bugs were all states the UI rendered as nothing.

---

## Phase 1 — Trust the loop: stop, review, steer *(client-only)*

The features that decide whether you dare leave it running.

**1.1 Stop button (lossless interrupt) — S.**
Esc / a stop control while `sending`. If a `tool_call` is parked on us,
first POST `tool-result` with `"Error: interrupted by user"` (never drop the
frame), then close the stream; server treats a failed emit as client-gone and
persists what completed. In-flight tool rows get `Err("interrupted")` — the
existing red badge, not a tick. Thread remains usable for the next send.
*Accept: mid-`cargo build` stop leaves a coherent transcript, next send works,
reopened session renders the same.*

**1.2 Per-file revert + turn review card — M.**
The shadow git already has the turn's commit. Add to the diff dock: changed
file list for the selected checkpoint, per-file **Revert** = `git --git-dir
.agent/git checkout <parent> -- <file>` (no patch matcher needed — this is
why per-hunk stays deferred), and a **Review** card on the latest turn
summarizing files ± lines with one click into the diff tab. Restore-all keeps
its two-step arm. `coder_git.rs` grows two functions; view work in the dock.
*Accept: a two-file turn can have one file reverted and the other kept; tree
and viewer refresh.*

**1.3 Message queue + steer — M.**
Composer stays enabled during a turn. Enter = **queue** (chips above the
composer, removable, sent one per turn-end in order). A **Stop & send** action
= 1.1 then send immediately — the Copilot three-way made explicit with two
verbs. No server change: queue drains client-side on `done`.
*Accept: type three follow-ups during a long turn; they run in order; stop &
send preempts cleanly.*

**1.4 Follow mode — S.**
Toggle in the header: when on, a `write_file`/`edit_file` result auto-opens
that file in the File dock tab (and flashes the tree row). Zed's crosshair,
one afternoon here because we own the pane.
*Accept: watching a multi-file turn shows each file as it's written.*

## Phase 2 — Plan gate + live todos *(client-only, via existing protocol)* — **landed 2026-08-19**

**2.1 Plan as an editable, gated artifact — M.**
Today `plan: true` plans and immediately executes. New **Plan gate** mode
(third state of the header switch: Off / Inline / Gate):
turn 1 sends with `tools: []` (the protocol's tool-free turn) → the plan
renders in an editable card (`iced::text_editor` — its first use here, the
widget plan.md already named as "one widget away") with **Run / Edit /
Discard**. Run sends the (possibly edited) plan as the instruction for the
real turn, plan-step off. The plan also writes to `.agent/plan.md` in the
workspace — Windsurf's trick: a file the user can edit in their own editor,
and `.agent/` is already ours. Junie's structure (steps + what-to-verify) via
prompt, not schema.
*Accept: edit step 3 of a plan, run, reopened session shows plan + execution
as ordinary rows.*
**Landed** as `PlanMode::{Off, Inline, Gate}`. Two departures from the sketch
above: the "write the plan only" ask rides in `mode_instruction` rather than on
the message (appending it to the message would persist it, and the rebuilt row
has to be the message that was sent), and there is no separate **Edit** — the
card is a `text_editor`, so Run and Discard are the only two verbs left. See
`plan.md` for the 4096-byte field the ask now shares with the workspace notes.

**2.2 Live todo list — M.**
Add a client-supplied `update_todos` tool spec via `SendRequest.tools`
(default six + ours) and handle it in the desktop executor: model posts
`[{text, done}]`, executor stores it in state, returns "ok". Render as a
pinned checklist between header and transcript, items ticking live —
Claude Code's TodoWrite. Rebuild: derive last-known list from the persisted
tool-call arguments in `done.messages`. Prompt nudge in `mode_instruction`
("keep update_todos current on multi-step tasks").
*Accept: a 5-step task shows steps ticking; reopening mid-way shows the same
list.*
**Landed.** The prompt nudge went into the tool's own `description` rather than
`mode_instruction` — same effect, and it leaves the capped field alone. The cost
this item incurs: `tools` *replaces* the server's list, so
`app/src/coder_tools.rs` now carries a verbatim copy of the server's six specs
beside its own seventh. **3.1's `edit_file` lands in three places, not two.**

## Phase 3 — Precise edits + context — **landed 2026-08-19**

**3.1 `edit_file` tool — L.**
Exact-match string replace (old → new, count-checked, whitespace-tolerant
fallback), in `TOOL_SPECS` and **both** executors. Whole-file `write_file`
stays for new files. Wins: token cost on large files collapses, diffs get
surgical, and the matcher is the missing piece that later unlocks per-hunk
revert. This is the one Phase-3 item with server code; contract doc +
`openapi.json` updated in the same commit, portal notified (additive — their
non-delegating callers just never see it… they do delegate, so their executor
needs it too before their models call it; gate: only in the spec list when
the client supplies executors that have it — i.e. ship in the default
`tool_specs()` only after both in-tree executors implement it).
*Accept: model edits a 1k-line file changing 3 lines; diff shows 3 lines; a
failed match returns a readable error the model recovers from.*
**Landed.** `openapi.json` did not need touching after all — `tools` was
already in `CoderChatSendRequest`, so advertising `edit_file` from the client
that implements it needed no schema change. The gate is exactly as written: both
in-tree executors have it, the server's default list does not.

**3.2 @-mentions — M.**
`@` in the composer opens a fuzzy file picker fed by the existing tree walk;
selection inserts `@path`. On send, mentioned files are inlined (bounded,
same 512 KB/binary sniff rules as the viewer) into the message. Client-only.
*Accept: `@src/main.rs what does boot do` answers without a read_file round
trip.*
**Landed** without the picker: `@path` typed into the composer is expanded on
send, which is the whole accept criterion. The picker is a convenience over a
thing that already works, so it waits for a session that wants it.

**3.3 AGENTS.md — S.**
If the workspace has `AGENTS.md` (the converging standard: Codex, Junie, Amp,
Copilot), append it to `mode_instruction` next to `.agent/notes.md`. Shown as
a chip in the header so its influence is visible.
*Accept: a rule in AGENTS.md observably steers a turn.*

## Phase 4 — Graduated autonomy + verification — **4.1/4.2 landed 2026-08-19**

**4.1 Autonomy presets + command allowlist — M.** *(client-only)*
Three tiers in the header — **Ask** (today), **Allowlist**, **Auto**
(`auto_approve_commands: true`, exists server-side, pinned off until now).
Allowlist = per-workspace prefix rules persisted in `settings.json`
(`cargo test`, `cargo build`, …), seeded by an **"Always allow"** button on
the approval card itself — the Claude Code trick that makes ask-mode livable.
Matching `approval_required` frames are auto-answered; the tool row says
"auto-approved by rule". Checkpoints exist, but the tier switch carries the
warning plan.md wrote: undo does not cover `pip install` or writes outside
the root.
*Accept: approve `cargo test` once with Always-allow; next turn runs it
unprompted and says why.*
**Landed** as four tiers rather than three: `Autonomy::{Off, Ask, Allowlist,
Auto}`, because "no commands at all" is what the screen shipped with and it is
not a tier of autonomy, it is the absence of one — so the old Commands checkbox
became the control instead of sitting beside it. Two things the sketch above did
not say and the code had to: a rule matches on a **word boundary** and refuses
any command carrying a shell operator (`cargo test; rm -rf /` starts with
`cargo test`), and the allowlist is answered **client-side** —
`auto_approve_commands` goes true for `Auto` only, because the rules live in this
machine's `settings.json` and a server cannot honour a list it cannot see.

**4.2 Review pass — S.** *(client-only)*
Button on the turn review card: send the turn's diff to a fresh tool-free
turn — "review this diff: bugs, missed requirements, project-rule
violations" — optionally on a different (stronger) model via the existing
picker. Amp's Oracle, minimum viable form: fresh context, zero new protocol.
*Accept: review of a seeded buggy diff names the bug.*
**Landed** on the "changed files" bar as *Ask the model*. The cost: `planning:
bool` had to become `TurnKind::{Work, Plan, Review}` — two tool-free turn kinds
that are not the same turn. The review is also the one turn whose `@`s are left
unexpanded; its prompt is a diff this screen built, and `@@ -1,7 +1,7 @@` is not
a mention.

**4.3 Agent commands in the visible terminal — L.** *(the open item from
plan.md step 5)* Run approved `run_command`s in the PTY drawer via sentinel
echo (`<cmd>; echo <mark>$?` scraped from the grid; OSC 133 where the shell
supports it), so long commands are watchable and promptable. Headless stays
the fallback. Do last in the phase; it has the most edge cases (exit
detection, interleaved user typing).
*Accept: an approved `cargo test` streams in the terminal; its output still
reaches the model; the user can answer a y/N prompt.*
**Blocked on the crate, not deferred.** `iced_term` 0.8 has the scraper this
needs — `Backend::selectable_content()` — but `Terminal::backend` is
`pub(crate)` and `Terminal` exposes no content accessor of its own, so nothing
outside that crate can read the grid. The ways through are a vendored
`iced_term` carrying one `pub fn`, or an upstream PR: a dependency decision
rather than a screen change. Not worked around — a second command-output panel
beside the terminal we already have is the run bar step 5 deleted.

## Phase 5 — Multi-session board *(the Cursor Agents Window; biggest lift)* — **landed 2026-08-19**

**5.1 Concurrent sessions with a status board — XL. — landed 2026-08-19.**
Refactor `coder::State` into `Session` (per-thread: turns, pending, stream,
checkpoints, queue, todos) + screen state; N sessions run concurrently, each
its own SSE stream and delegated executor (the invariant holds per-thread —
tool results are keyed `(thread_id, call_id)`, so parallel streams are
already safe server-side). Sessions pane becomes the board: ● running /
⏸ awaiting approval / ✓ idle per row; switching sessions is a tab switch;
OS-level notification (tray exists) when a background session finishes or
parks on approval.
**Landed, and not as an XL.** `coder::State` was already one session's worth of
state, so nothing was split out of it: `coder_board.rs` holds `Vec<Slot>` and
**derefs to the active session**, which leaves `main.rs`, `coder_view` and the
4000-line `update` reading the tab in front exactly as they did. What the sketch
above missed is the part that had to be built: routing. Every task a session
starts is tagged `Message::For(id, …)` and routed back to it, an untagged
message goes to the active session, and a frame for a closed session is
**dropped** — without that a background stream writes its transcript into
whichever tab is in front. The parallel-stream safety this item asked to verify
holds server-side (`(thread_id, call_id)`), but the *checkpoint* repo is one per
folder: a turn is refused while another session is mid-turn in the same
checkout, and mid-turn includes parked on the approval card, where `sending` is
false and the commit has not been taken.
**5.2 Worktree isolation option — M.** Per-session checkbox: run in
`git worktree add .agent/worktrees/<thread>` when the workspace is a real
repo; session's root points there; a Merge-back action surfaces `git diff`
against the main tree. Skipped for non-repo folders.
**5.3 Fork / handoff — S. — landed 2026-08-19.** "New session from here":
tool-free summarize turn seeds a fresh thread (Amp's `/handoff`) — kills the
polluted-context restart tax.
**Landed** ahead of 5.1, which it does not depend on: *Hand off to a new one*
beside New session. The summary lands in the next session's **composer**, not
sent — a handoff nobody read is the restart tax with extra steps — and stays
in the old thread as its last row. `TurnKind` gained a fourth member for it.
An empty summary leaves the session standing and says why; throwing a session
away on a failed call is the one failure here that loses work.
*Accept (5.1): two sessions on two folders run simultaneously; approving in
one doesn't touch the other; a finished background session notifies.* — **met
live**, `llama3.1:8b` over Ollama on a sandboxed daemon: one session parked on
an approval in the project while a second streamed in its own worktree, *Run*
resumed only the first, and a turn finishing while the user was on Home posted a
toast naming the session. See plan.md.

## Deliberately not doing

- **Token-level streaming** — the loop is buffered by design; heartbeat +
  glanceable tool rows cover "working vs hung". Revisit only if whole-step
  frames feel dead in daily use.
- **Tab completion / inline Cmd-K** — that's an editor; ours is a viewer by
  decision (plan.md item 4). Junie doesn't have it either.
- **LSP / Problems** — still the same deferral: diagnostics for the language
  the agent writes least here.
- **Semantic indexing** — literal `search` + `repo_map` + @-mentions first;
  Claude Code ships grep-only on purpose.
- **Cloud agents / CI triggers** — no cloud substrate in this product; 5.1's
  local background sessions are the analog.

## Order and sizing

| Phase | Items | Size | Protocol change |
|---|---|---|---|
| 1 | stop, per-file revert, queue+steer, follow | S+M+M+S | none |
| 2 | plan gate, todos | M+M | none |
| 3 | edit_file, @-mentions, AGENTS.md | L+M+S | edit_file only |
| 4 | autonomy tiers ✓, review pass ✓, terminal runs (blocked) | M+S+L | none |
| 5 | session board ✓, worktrees ✓, handoff ✓ | S+M+S | none (parallel streams verified) |

Each item ships alone: own commit(s), driven live before claimed done,
`plan.md` updated as steps land. Phases 1–2 are the daily-driver threshold;
3–4 make it pleasant; 5 makes it Cursor.
