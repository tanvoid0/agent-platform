# The coder delegation protocol

`POST /api/v1/coder/chat/stream` runs the agent loop on the server and streams
SSE. With delegation on, the *tools* run on the client instead: the server emits
a `tool_call` frame, parks the turn, and waits for the client to post the result
back. That is what lets the model live wherever the proxy points while the files
stay on the user's machine.

Two clients implement this today — the iced desktop app
(`desktop/crates/client/src/coder_stream.rs`) and portal_desktop
(`src-tauri/src/domains/coder/platform_stream.rs`). They must not disagree about
what a frame means, so this is the contract, read off
[`coder.rs`](../desktop/crates/server/src/coder.rs),
[`coder_loop.rs`](../desktop/crates/server/src/coder_loop.rs) and
[`coder_tools.rs`](../desktop/crates/server/src/coder_tools.rs).

## Turning delegation on

`make_executor` picks the delegated path when **either** holds:

- the request carries `X-Agent-Platform-Client: portal-desktop`, or
- the request body sets `"delegate_tools": true`.

`workspace_root` is then **required** and is not resolved or checked by the
server — it is a string the client owns. Without it the route answers 400,
`"workspace_root is required for desktop-delegated execution"`.

Without delegation the server executes the same six tools itself against a
`canonicalize`d root, and the client sees `tool_result` frames it did not
produce. The tool set is identical on both sides on purpose: a model must not be
able to tell which machine ran a tool.

## The frames

`event: <name>` / `data: <json>`, blank-line separated. Emitted by
`coder_loop::sse`.

| Event | Payload | Meaning |
|---|---|---|
| `heartbeat` | — | A model step is in flight. Every `CODER_HEARTBEAT_INTERVAL_SECONDS` (default 8). Tells "working" from "hung"; carries no content. |
| `plan` | `{content}` | The tool-free PLAN step's output, when `"plan": true`. Renders as an assistant row; it exists as its own event only so the stream can tell it from an answer. |
| `tool_call` | `{call_id, name, arguments}` | **Run this tool and answer.** Under delegation the turn is now parked. |
| `tool_result` | `{name, content}` | A tool's output — from the server's own executor, or echoed after your `tool-result` post. |
| `approval_required` | `{call_id, name, arguments}` | The turn stopped at the approval gate. Answer with `POST /coder/chat/approve`, not `tool-result`. Also carries `remaining` on the internal payload for the queued calls behind it. |
| `assistant` | `{content, usage}` | The turn's answer, or the iteration-cap message. |
| `title` | thread title | The thread was retitled; persist it. |
| `error` | `{detail}` | The turn failed. Always followed by `done`. |
| `done` | `{thread_id, title, workspace_root, context_window, messages, pending_call, context_usage, usage}` | Terminal. `messages` is the full persisted transcript — reconciling against it is how a reopened thread renders identically to the live one. |

There is no token-level streaming. The loop is buffered and every frame is a
whole step; `client/src/sse.rs` says so outright.

## Answering a `tool_call`

```
POST /api/v1/coder/chat/tool-result
{"thread_id": <i64>, "call_id": "<string>", "result": "<string>"}
```

Four things are load-bearing:

- **Every `tool_call` must be answered.** A dropped frame does not error. The
  turn stalls silently for `DELEGATION_TIMEOUT_SECONDS` (300) and then the model
  receives `"Error: timed out waiting for desktop to execute tool"` as the tool
  result.
- **The thread must exist before the first turn streams**, because the result is
  addressed by `thread_id`. Create it with `POST /coder/chat/threads` first.
- **A tool failure is a result, not an error.** Post the failure text as
  `result`; the model reads it and continues. Non-200 from this route means the
  *protocol* broke, not the tool.
- **A duplicate `call_id` is answered as a tool result too** —
  `"Error: duplicate tool call id …"` — rather than failing the turn. An unknown
  or already-resolved `call_id` is a **404**.

Results are truncated server-side to a token soft cap before reaching the model.

## Approvals

`run_command` behind `allow_commands` stops the turn with
`approval_required` instead of `tool_call`. Resume with
`POST /api/v1/coder/chat/approve` (`thread_id`, `call_id`, `approve`, optional
`edited_command`), which reopens a stream and emits `tool_call` for the approved
call.

**A decision is not final until a frame off the resumed stream says so.** Keep
the pending card until then. Clearing it optimistically leaves the server
holding the call and the client with nothing to answer from, and every later
send comes back *"thread has a command awaiting approval"* — unrecoverable
without a new thread.

`auto_approve_commands: true` skips the gate entirely. The iced app pins it
`false`; that is a product decision, not a protocol one.

## Request fields the loop reads

`SendRequest` in [`coder.rs`](../desktop/crates/server/src/coder.rs): `message`,
`thread_id`, `model`, `provider`, `workspace_root`, `allow_commands`,
`auto_approve_commands`, `max_tokens`, `delegate_tools`, `tools`,
`mode_instruction`, `agent_mode`, `plan`.

- `provider` **is** honoured — the proxy pins the hint, 400s an unsupported id
  and 503s an unconfigured one, and only resolves from the model alias when no
  hint is sent.
- **`tools` replaces the default specs for every step of the turn.** Three
  states, and they are distinct:
  - **absent** — this crate's six `tool_specs()`, as before;
  - **a non-empty list** — yours, verbatim, and the model may call any of them.
    Only useful if you are delegating: whatever it calls comes back as a
    `tool_call` for you to run. A non-delegating caller gets
    `"Error: unknown tool '…'."` as the result for anything the local executor
    does not implement;
  - **`[]`** — a tool-free turn, the same shape the PLAN step uses. Not the
    default set.

  Validated on the way in: at most 64 entries, each a JSON object, 64 KB
  serialized. Anything else is a 422 naming `tools`.
- The server's resolved default model is `llama3`, which **cannot hold a tool
  loop** — it reads a file and then ends the turn silently. A client without a
  model picker will look broken.

### `retry` and `approve` accept the whole of `SendRequest`, and ignore parts of it

`RetryRequest` and `ApprovalRequest` both `#[serde(flatten)]` a `SendRequest`, so
on the wire those two routes accept **every** field listed above. They do not
*read* every field, and nothing tells you which ones went nowhere — an ignored
field is not an error, it is silence.

| Route | Reads | Accepts and ignores |
|---|---|---|
| `POST /coder/chat/retry` | `thread_id`, `workspace_root`, `allow_commands`, `delegate_tools`, `tools`, `mode_instruction`, `model`, `provider`, `max_tokens` | `message` — a retry replays the stored history, it does not take a new turn |
| `POST /coder/chat/approve` | `thread_id`, `call_id`, `approve`, `edited_command`, `delegate_tools`, `tools`, `mode_instruction`, `model`, `provider`, `max_tokens` | `message`, `workspace_root`, `allow_commands`, `plan` |

The approve exclusions are deliberate, and each has a reason worth knowing:

- **`allow_commands` is forced `true`.** The user has just approved this command;
  the session-level switch is not consulted a second time. Sending `false` here
  does not veto the approval you already gave.
- **`workspace_root` is forced to the thread's own.** A resume may only run where
  the thread already runs, so a different root in the body is dropped rather than
  honoured.
- **`plan` is forced `false`.** The plan, if there was one, happened on the turn
  that got parked.

This is why `openapi.json` documents `CoderApprovalRequest` and `CoderRetryRequest`
as *narrower* than what the flatten accepts. The spec describes what the route
honours, which is the more useful contract — but it means "the server took my
field without complaining" is not evidence the field did anything.

`portal_desktop` currently sends `allow_commands` and `auto_approve_commands` on
approve. Harmless — the route wants `true` and hardcodes it — but it is a no-op,
not a setting.
