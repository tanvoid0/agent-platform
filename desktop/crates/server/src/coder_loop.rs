//! The Coder agent's turn — `run_agent_turn` and the one LLM step under it,
//! ported from `app/coder/service.py`.
//!
//! One send is one agent run: the model may issue several tool calls before it
//! produces the final assistant text. The loop is provider-agnostic — it speaks
//! the OpenAI tools format, and [`crate::llm::complete_internal`] is the
//! `/v1/chat/completions` handler's own resolution, coercion, capability guard,
//! retry policy and usage normalisation, minus the socket Python takes back
//! over loopback to reach the same code.
//!
//! **Nothing here streams and that is not a compromise.** Every LLM call in the
//! loop is buffered; the SSE the routes emit is the server's own framing of
//! whole steps, which `client/src/sse.rs` states outright. The only thing
//! resembling streaming is the `heartbeat` frame emitted while a step is in
//! flight, so a client can tell "still working" from "hung".
//!
//! Events are pushed through an [`Emitter`] rather than yielded, because the
//! non-streaming `POST /coder/chat/send` runs the same turn and drops them.
//! **An emit that fails means the client is gone**, and the turn stops there —
//! Python's generator gets `GeneratorExit` at its next `yield` and its `finally`
//! persists whatever the agent had completed. Same outcome, same commit.

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{json, Map, Value};

use crate::chat_usage::{merge_llm_usages, parse_llm_usage_dict, LlmStepUsageOut};
use crate::coder_tools::Executor;
use crate::context_budget::{
    fit_chat_messages_for_request, max_output_tokens_default, tool_result_soft_cap_tokens,
    truncate_text_to_tokens,
};
use crate::dag_schema::{python_json, sanitize_llm_model_alias};
use crate::error::ApiError;
use crate::AppState;

/// `PLAN_PROMPT` / `PLAN_ACK` — one tool-free call before the loop, ported from
/// hearth's agent, which measures it as the single biggest quality lever for a
/// local model. Tool-free is the point: a model handed tools uses them.
const PLAN_PROMPT: &str = "Before touching anything, write a short numbered plan for this task: \
which files you expect to read or change, what the change is, and what you will check afterwards. \
At most five steps. Do not call any tools yet and do not write out the code — just the plan.";
const PLAN_ACK: &str =
    "Now carry that out with the tools, adjusting the plan if what you read contradicts it.";

/// `APPROVAL_REQUIRED_TOOLS`.
const APPROVAL_REQUIRED_TOOLS: [&str; 1] = ["run_command"];

/// `_max_iterations`, `CODER_MAX_ITERATIONS`.
fn max_iterations() -> usize {
    match crate::env_opt("CODER_MAX_ITERATIONS").as_deref().map(str::parse::<i64>) {
        Some(Ok(n)) => n.max(1) as usize,
        // Unparseable falls back to the default, exactly as the `except
        // ValueError` does — including a float, which `int()` also rejects.
        _ => 15,
    }
}

/// `_heartbeat_interval_seconds`, `CODER_HEARTBEAT_INTERVAL_SECONDS`.
fn heartbeat_interval() -> std::time::Duration {
    let seconds = match crate::env_opt("CODER_HEARTBEAT_INTERVAL_SECONDS").as_deref().map(str::parse::<f64>)
    {
        Some(Ok(n)) if n.is_finite() => n.max(1.0),
        _ => 8.0,
    };
    std::time::Duration::from_secs_f64(seconds)
}

// ---------------------------------------------------------------------------
// SSE framing and the event sink
// ---------------------------------------------------------------------------

/// `_sse`: `json.dumps(data, ensure_ascii=False)`, which is `", "`/`": "`
/// separators — not serde's compact ones, and this goes on the wire.
pub(crate) fn sse(event: &str, data: &Value) -> String {
    format!("event: {event}\ndata: {}\n\n", python_json(data, false))
}

/// Where a turn's events go. `Discard` is `POST /chat/send`, which runs the
/// same turn for its persisted result and ignores the frames.
pub(crate) enum Emitter<'a> {
    Discard,
    Sse(&'a tokio::sync::mpsc::UnboundedSender<String>),
}

impl Emitter<'_> {
    /// `false` once the client is gone — the caller stops the turn and persists.
    pub(crate) fn emit(&self, event: &str, data: Value) -> bool {
        match self {
            Emitter::Discard => true,
            Emitter::Sse(tx) => tx.send(sse(event, &data)).is_ok(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tool calls
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub(crate) struct ToolCall {
    pub id: String,
    pub name: String,
    /// Always an object: `_parse_tool_calls_raw` forces a non-dict to `{}`.
    pub arguments: Value,
    /// The provider's own entry, persisted verbatim on the assistant message
    /// and replayed into `pending_call.remaining`.
    pub raw: Value,
}

/// `_parse_tool_calls_raw`. `function.arguments` is a JSON string per spec, but
/// some local backends return the object already — both are accepted, and
/// anything else becomes `{}` rather than failing the turn.
pub(crate) fn parse_tool_calls_raw(raw_calls: &[Value]) -> Vec<ToolCall> {
    raw_calls
        .iter()
        .enumerate()
        .map(|(i, tc)| {
            let function = tc.get("function");
            let name = function
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let raw_args = function.and_then(|f| f.get("arguments"));
            let arguments = match raw_args {
                Some(Value::Object(map)) => Value::Object(map.clone()),
                Some(Value::String(s)) if !s.is_empty() => {
                    serde_json::from_str::<Value>(s).ok().filter(Value::is_object).unwrap_or(json!({}))
                }
                _ => json!({}),
            };
            let id = tc
                .get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("call_{i}"));
            ToolCall { id, name, arguments, raw: tc.clone() }
        })
        .collect()
}

fn parse_tool_calls(message: &Value) -> Vec<ToolCall> {
    match message.get("tool_calls") {
        Some(Value::Array(calls)) => parse_tool_calls_raw(calls),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Leaked tool calls — `coder/tool_call_parse.py`
// ---------------------------------------------------------------------------

/// Every tool in `TOOL_SPECS`. `search` and `repo_map` belong here: a model
/// that leaks `<function=search>` as text would otherwise have the call dropped
/// while the markup was stripped from its answer, and the recovery path exists
/// for exactly the weak local models this screen targets.
const KNOWN_TOOLS: [&str; 6] =
    ["read_file", "write_file", "list_dir", "search", "repo_map", "run_command"];

/// One `<function=name …>body(</function>|next tag|end)` span.
struct LeakedBlock {
    start: usize,
    end: usize,
    name: String,
    body: String,
}

/// Python's two regexes in one scan.
///
/// `LEAKED_TOOL_BLOCK_RE` uses a lookahead (`(?=<function=)`) to end a body at
/// the next tag, and the `regex` crate has none — so the tag starts are found
/// first and each body simply runs to whichever comes first of `</function>`,
/// the next start, or the end of the text. That is what the lazy alternation
/// resolves to, not an approximation of it.
fn leaked_blocks(content: &str) -> Vec<LeakedBlock> {
    static TAG: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let tag = TAG.get_or_init(|| regex::Regex::new(r"(?is)<function=(\w+)(?:[^>]*)>").unwrap());

    let starts: Vec<(usize, usize, String)> = tag
        .captures_iter(content)
        .map(|c| {
            let whole = c.get(0).unwrap();
            (whole.start(), whole.end(), c.get(1).unwrap().as_str().to_string())
        })
        .collect();

    let lower = content.to_lowercase();
    let mut out = Vec::with_capacity(starts.len());
    for (i, (start, body_start, name)) in starts.iter().enumerate() {
        let next_tag = starts.get(i + 1).map(|(s, _, _)| *s).unwrap_or(content.len());
        let closing = lower[*body_start..next_tag].find("</function>").map(|p| body_start + p);
        let (body_end, end) = match closing {
            Some(at) => (at, at + "</function>".len()),
            None => (next_tag, next_tag),
        };
        out.push(LeakedBlock {
            start: *start,
            end,
            name: name.clone(),
            body: content[*body_start..body_end].to_string(),
        });
    }
    out
}

/// `strip_leaked_tool_syntax`. Every block goes, known tool or not — the markup
/// is never something to show the user.
pub(crate) fn strip_leaked_tool_syntax(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for block in leaked_blocks(text) {
        out.push_str(&text[cursor..block.start]);
        cursor = block.end;
    }
    out.push_str(&text[cursor..]);
    out.trim().to_string()
}

/// `_parse_args_blob`: JSON object or `{}`, never a failure.
fn parse_args_blob(blob: &str) -> Value {
    let blob = blob.trim();
    if blob.is_empty() {
        return json!({});
    }
    serde_json::from_str::<Value>(blob).ok().filter(Value::is_object).unwrap_or(json!({}))
}

/// `uuid.uuid4().hex[:12]`.
fn leaked_call_id() -> String {
    let mut bytes = [0u8; 6];
    // A failure here would mean the OS entropy source is gone; a fixed id is
    // still unique enough for one turn's dedup, which is all this is for.
    let _ = getrandom::getrandom(&mut bytes);
    format!("leaked_{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// `parse_leaked_tool_calls`: one call per *tool*, first occurrence wins.
pub(crate) fn parse_leaked_tool_calls(content: &str) -> Vec<ToolCall> {
    if content.is_empty() || !content.to_lowercase().contains("<function=") {
        return Vec::new();
    }
    let mut calls = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for block in leaked_blocks(content) {
        if !KNOWN_TOOLS.contains(&block.name.as_str()) || seen.contains(&block.name) {
            continue;
        }
        seen.push(block.name.clone());
        let arguments = parse_args_blob(&block.body);
        let id = leaked_call_id();
        let raw = json!({
            "id": id,
            "type": "function",
            // `json.dumps(args)` — the default, so `ensure_ascii` is on here
            // where the SSE framing has it off.
            "function": {"name": block.name, "arguments": python_json(&arguments, true)},
        });
        calls.push(ToolCall { id, name: block.name, arguments, raw });
    }
    calls
}

// ---------------------------------------------------------------------------
// One LLM step
// ---------------------------------------------------------------------------

/// Everything the turn passes through to each step. Grouped rather than
/// threaded one by one because `send`, `stream`, `retry` and `approve` all
/// build the same set from their own request body.
pub(crate) struct TurnOptions {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub max_tokens: Option<i64>,
    pub auto_approve_commands: bool,
    pub plan: bool,
    /// The caller's tool list, replacing [`crate::coder::tool_specs`] for every
    /// step of this turn. `None` is the default set; `Some(&[])` is tool-free.
    pub tools: Option<Vec<Value>>,
}

/// `_call_llm_step`. `tools=false` is the PLAN step: the tools are left out of
/// the request entirely rather than discouraged in the prompt, because a model
/// handed tools uses them.
async fn call_llm_step(
    state: &Arc<AppState>,
    messages: &[Value],
    opts: &TurnOptions,
    tools: bool,
) -> Result<(Value, Option<Value>), ApiError> {
    if state.master_key.is_none() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "AGENT_PLATFORM_MASTER_KEY is not set.",
        ));
    }

    let (fitted, _) = fit_chat_messages_for_request(messages.to_vec());
    let mut payload = Map::new();
    payload.insert("messages".into(), Value::Array(fitted));
    payload.insert(
        "max_tokens".into(),
        json!(opts.max_tokens.unwrap_or_else(max_output_tokens_default)),
    );
    if tools {
        // The caller's list wins when it sent one. A delegating client runs
        // whatever the model calls, so the set it advertises is its business,
        // not this crate's — but an empty list still means "no tools", so a
        // client that filtered everything out gets a tool-free step rather
        // than a surprise default set.
        let specs = opts.tools.clone().unwrap_or_else(crate::coder::tool_specs);
        if !specs.is_empty() {
            payload.insert("tools".into(), Value::Array(specs));
            payload.insert("tool_choice".into(), json!("auto"));
        }
    }
    let sanitized = opts.model.as_deref().and_then(sanitize_llm_model_alias);
    if let Some(model) = &sanitized {
        payload.insert("model".into(), json!(model));
    }
    let provider = opts
        .provider
        .as_deref()
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty());
    if let Some(provider) = &provider {
        payload.insert("provider".into(), json!(provider));
    }

    // Python pre-checks capabilities here as well as inside the proxy, and the
    // two report differently: this one raises its own status (a 400 the client
    // sees as-is), where the proxy's would come back as a 502 wrapping it.
    let effective_model = sanitized
        .clone()
        .or_else(|| opts.model.as_deref().map(str::trim).filter(|m| !m.is_empty()).map(str::to_string));
    if let Some(model) = effective_model {
        let effective_provider = provider.clone().unwrap_or_else(|| {
            crate::provider_catalog::resolved_defaults()
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        });
        crate::model_capabilities::ensure_chat_request_supported(
            &state.http,
            &effective_provider,
            &model,
            &payload,
        )
        .await?;
    }

    // Python reads the proxy's HTTP response and re-reports it; the same
    // failures arrive here as an `ApiError` carrying the status the public
    // route would have answered with. The body snippet Python appends is the
    // one thing lost — the status is what callers branch on.
    let data = crate::llm::complete_internal(state, payload).await.map_err(|e| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            format!("LLM proxy returned HTTP {}", e.status.as_u16()),
        )
    })?;

    let usage = data.get("usage").filter(|u| u.is_object()).cloned();
    let message = data
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.get("message"))
        .filter(|m| m.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    Ok((message, usage))
}

/// `_call_llm_step_with_heartbeat`: the same call, emitting `heartbeat` while
/// it runs. A single agent step is one blocking call that produces no output
/// until it resolves and can take minutes on a queued local model.
async fn call_llm_step_with_heartbeat(
    state: &Arc<AppState>,
    messages: &[Value],
    opts: &TurnOptions,
    tools: bool,
    emit: &Emitter<'_>,
) -> Result<(Value, Option<Value>), TurnStop> {
    let interval = heartbeat_interval();
    let call = call_llm_step(state, messages, opts, tools);
    tokio::pin!(call);
    let mut waited = 0.0f64;
    loop {
        tokio::select! {
            result = &mut call => return result.map_err(TurnStop::Failed),
            () = tokio::time::sleep(interval) => {
                waited += interval.as_secs_f64();
                if !emit.emit("heartbeat", json!({ "waited_seconds": round1(waited) })) {
                    return Err(TurnStop::ClientGone);
                }
            }
        }
    }
}

/// `round(x, 1)` on a float that serializes as a number, not a string.
fn round1(value: f64) -> Value {
    json!((value * 10.0).round() / 10.0)
}

// ---------------------------------------------------------------------------
// The turn
// ---------------------------------------------------------------------------

/// Why a turn stopped early. `Failed` is Python's `HTTPException` escaping the
/// generator — the caller persists, emits `error`, then `done`. `ClientGone` is
/// its `GeneratorExit`: persist and say nothing, because there is nobody left
/// to say it to.
pub(crate) enum TurnStop {
    Failed(ApiError),
    ClientGone,
}

/// Everything a turn produces that its caller has to persist or report.
#[derive(Default)]
pub(crate) struct TurnOutcome {
    pub new_history: Vec<Value>,
    /// `{call_id, name, arguments, remaining}` when the turn paused for
    /// approval. At most one, as in Python.
    pub pending: Option<Value>,
    pub usage_steps: Vec<LlmStepUsageOut>,
}

/// `run_agent_turn`.
///
/// If a tool in `APPROVAL_REQUIRED_TOOLS` is hit and `auto_approve_commands` is
/// false the turn pauses: the pending call plus any not-yet-executed calls from
/// the same batch land in [`TurnOutcome::pending`] and the loop returns without
/// calling the model again. Resume by re-invoking with `resume_calls` set to
/// the parsed remaining calls after resolving the pending one.
pub(crate) async fn run_agent_turn(
    state: &Arc<AppState>,
    llm_messages: &mut Vec<Value>,
    executor: &Executor,
    opts: &TurnOptions,
    resume_calls: Option<Vec<ToolCall>>,
    emit: &Emitter<'_>,
    out: &mut TurnOutcome,
) -> Result<(), TurnStop> {
    let mut calls = resume_calls;
    let mut usage_steps: Vec<LlmStepUsageOut> = Vec::new();
    let mut step_num = 0usize;

    // Never on a resume: either the plan is already in the history this turn is
    // picking up from, or the turn died before it produced one — and re-planning
    // after a tool result would plan around work already done.
    if opts.plan && calls.is_none() {
        llm_messages.push(json!({ "role": "user", "content": PLAN_PROMPT }));
        let (planned, plan_usage) =
            call_llm_step_with_heartbeat(state, llm_messages, opts, false, emit).await?;
        usage_steps.push(parse_llm_usage_dict(plan_usage.as_ref(), Some("plan")));
        let plan_text = message_content(&planned).trim().to_string();
        if plan_text.is_empty() {
            // A model that answered nothing gets no ack to answer — leaving the
            // prompt in would make its own silence the last thing it read.
            llm_messages.pop();
        } else {
            // Persisted as a plain assistant message, *without* the prompt that
            // asked for it: the desktop rebuilds a reopened session from this
            // log, and a "write me a plan" line in the transcript is scaffolding
            // the user never typed.
            let plan_msg = json!({ "role": "assistant", "content": plan_text });
            llm_messages.push(plan_msg.clone());
            out.new_history.push(plan_msg);
            llm_messages.push(json!({ "role": "user", "content": PLAN_ACK }));
            if !emit.emit("plan", json!({ "content": plan_text })) {
                return Err(TurnStop::ClientGone);
            }
        }
    }

    for _ in 0..max_iterations() {
        if calls.is_none() {
            let (message, usage) =
                call_llm_step_with_heartbeat(state, llm_messages, opts, true, emit).await?;
            step_num += 1;
            usage_steps
                .push(parse_llm_usage_dict(usage.as_ref(), Some(&format!("agent_step_{step_num}"))));

            let mut content = message_content(&message);
            let mut round = parse_tool_calls(&message);
            if round.is_empty() && !content.is_empty() {
                round = parse_leaked_tool_calls(&content);
                if !round.is_empty() {
                    content = strip_leaked_tool_syntax(&content);
                }
            }

            let mut assistant_msg = Map::new();
            assistant_msg.insert("role".into(), json!("assistant"));
            assistant_msg.insert("content".into(), json!(content));
            if !round.is_empty() {
                assistant_msg.insert(
                    "tool_calls".into(),
                    Value::Array(round.iter().map(|c| c.raw.clone()).collect()),
                );
            }
            if let Some(usage) = &usage {
                assistant_msg.insert("usage".into(), usage.clone());
            }
            let assistant_msg = Value::Object(assistant_msg);
            llm_messages.push(assistant_msg.clone());
            out.new_history.push(assistant_msg);

            // Reasoning the model wrote before deciding to call a tool. Surfaced
            // now so clients show "why" before the (possibly slow) tool runs,
            // instead of dropping it into history unseen until the turn ends.
            if !round.is_empty() && !content.trim().is_empty() {
                if !emit.emit("assistant", json!({ "content": content })) {
                    return Err(TurnStop::ClientGone);
                }
            }

            if round.is_empty() {
                let turn_usage = merge_llm_usages(usage_steps.clone());
                out.usage_steps = usage_steps;
                emit.emit("assistant", json!({ "content": content, "usage": turn_usage }));
                return Ok(());
            }
            calls = Some(round);
        }

        let batch = calls.take().unwrap_or_default();
        for (idx, call) in batch.iter().enumerate() {
            if APPROVAL_REQUIRED_TOOLS.contains(&call.name.as_str()) && !opts.auto_approve_commands {
                out.pending = Some(json!({
                    "call_id": call.id,
                    "name": call.name,
                    "arguments": call.arguments,
                    "remaining": batch[idx + 1..].iter().map(|c| c.raw.clone()).collect::<Vec<_>>(),
                }));
                // `usage_steps_out` is deliberately *not* extended here: Python
                // only extends it on the two final-assistant paths, so a turn
                // that pauses for approval reports zero usage in its `done`
                // payload. Copied, not corrected.
                emit.emit(
                    "approval_required",
                    json!({ "call_id": call.id, "name": call.name, "arguments": call.arguments }),
                );
                return Ok(());
            }

            if !emit.emit(
                "tool_call",
                json!({ "call_id": call.id, "name": call.name, "arguments": call.arguments }),
            ) {
                return Err(TurnStop::ClientGone);
            }
            let result = executor.execute(state, &call.name, &call.arguments, &call.id).await;
            let result = truncate_text_to_tokens(&result, tool_result_soft_cap_tokens());
            let tool_msg = json!({
                "role": "tool",
                "tool_call_id": call.id,
                "name": call.name,
                "content": result,
            });
            llm_messages.push(tool_msg.clone());
            out.new_history.push(tool_msg);
            if !emit.emit("tool_result", json!({ "name": call.name, "content": result })) {
                return Err(TurnStop::ClientGone);
            }
        }
    }

    let turn_usage = merge_llm_usages(usage_steps.clone());
    out.usage_steps = usage_steps;
    emit.emit(
        "assistant",
        json!({
            "content": format!("Stopped: reached the maximum of {} agent iterations.", max_iterations()),
            "usage": turn_usage,
        }),
    );
    Ok(())
}

/// `message.get("content") or ""`, then `str(content)` for anything that is not
/// already a string — so a model answering with a list gets Python's repr of
/// it in the transcript, not JSON.
fn message_content(message: &Value) -> String {
    match message.get("content") {
        Some(Value::String(s)) => s.clone(),
        // `str()` of a container **is** its repr, so `['a']` — not JSON, which
        // is what `todos::python_str` renders for one.
        Some(other) if crate::action_orchestrator::py_truthy(other) => {
            crate::todos::py_repr(other)
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_frames_carry_pythons_separators() {
        // `json.dumps(..., ensure_ascii=False)`: a space after `:` and `,`, and
        // no `\uXXXX` escaping. Both are bytes on the wire.
        assert_eq!(
            sse("tool_result", &json!({"name": "read_file", "content": "é"})),
            "event: tool_result\ndata: {\"name\": \"read_file\", \"content\": \"é\"}\n\n"
        );
    }

    #[test]
    fn tool_call_arguments_accept_a_string_or_an_object() {
        let calls = parse_tool_calls_raw(&[
            json!({"id": "a", "function": {"name": "read_file", "arguments": "{\"path\": \"x\"}"}}),
            json!({"id": "b", "function": {"name": "list_dir", "arguments": {"path": "."}}}),
            // Junk arguments, a non-object, and a missing id each degrade
            // rather than fail: `{}` and `call_{i}`.
            json!({"function": {"name": "repo_map", "arguments": "not json"}}),
            json!({"id": "d", "function": {"name": "search", "arguments": "[1,2]"}}),
        ]);
        assert_eq!(calls[0].arguments, json!({"path": "x"}));
        assert_eq!(calls[1].arguments, json!({"path": "."}));
        assert_eq!(calls[2].id, "call_2");
        assert_eq!(calls[2].arguments, json!({}));
        assert_eq!(calls[3].arguments, json!({}));
    }

    /// The recovery path for a model that writes its call as text. `search` and
    /// `repo_map` are in `KNOWN_TOOLS` — leaving them out silently dropped the
    /// call while stripping the markup, so the turn looked like it did nothing.
    #[test]
    fn leaked_calls_are_recovered_and_their_markup_stripped() {
        let text = "I will look.<function=search>{\"query\": \"send_message\"}</function> Done.";
        let calls = parse_leaked_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "search");
        assert_eq!(calls[0].arguments, json!({"query": "send_message"}));
        assert!(calls[0].id.starts_with("leaked_"));
        assert_eq!(calls[0].raw["function"]["arguments"], json!("{\"query\": \"send_message\"}"));
        assert_eq!(strip_leaked_tool_syntax(text), "I will look. Done.");
    }

    #[test]
    fn an_unclosed_block_ends_at_the_next_tag_or_the_end() {
        // No `</function>`: the body runs to the next tag, then to the end.
        let text = "<function=read_file>{\"path\": \"a\"}<function=list_dir>{}";
        let calls = parse_leaked_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].arguments, json!({"path": "a"}));
        assert_eq!(calls[1].name, "list_dir");
        assert_eq!(strip_leaked_tool_syntax(text), "");

        // One call per tool, first occurrence wins; an unknown name is dropped
        // from the calls but still stripped from the visible answer.
        let dupes = "<function=read_file>{}</function><function=read_file>{}</function>\
                     <function=made_up>{}</function>tail";
        assert_eq!(parse_leaked_tool_calls(dupes).len(), 1);
        assert_eq!(strip_leaked_tool_syntax(dupes), "tail");
    }

    #[test]
    fn text_without_the_marker_is_left_alone() {
        assert!(parse_leaked_tool_calls("just prose").is_empty());
        assert_eq!(strip_leaked_tool_syntax("  just prose  "), "just prose");
        assert_eq!(strip_leaked_tool_syntax(""), "");
    }

    #[test]
    fn content_falls_back_the_way_pythons_or_does() {
        assert_eq!(message_content(&json!({"content": "hi"})), "hi");
        assert_eq!(message_content(&json!({})), "");
        assert_eq!(message_content(&json!({"content": null})), "");
        // Falsy, so `or ""` wins before `str()` is ever reached.
        assert_eq!(message_content(&json!({"content": []})), "");
        assert_eq!(message_content(&json!({"content": ["a"]})), "['a']");
    }

    #[test]
    fn heartbeat_seconds_round_to_one_place() {
        assert_eq!(round1(24.000000000000004), json!(24.0));
        assert_eq!(round1(0.75), json!(0.8));
    }
}
