//! The Coder agent — `app/coder/routes.py` + `service.py`, **the whole domain**.
//!
//! All ten routes: the five CRUD ones here, and the five that run the agent
//! loop (`send`, `stream`, `retry`, `approve`, `tool-result`), which had to move
//! **in one commit**. The delegated tool call parks an in-process future in
//! whichever server handled `/chat/stream`, and the unpark must land in that
//! same process; split them across two servers and `/chat/tool-result` 404s
//! while the turn waits out its full 300s and then feeds the model "timed out".
//! The park itself is [`crate::coder_tools`]; the turn is [`crate::coder_loop`].
//!
//! This domain has **two pause mechanisms and only one is portable**. The
//! *approval* pause is `pending_call_json` on the row — DB state, and it
//! survives anything, exactly like `/processes` cancel. The *delegation* pause
//! is process memory and does not. Both are here; only the first is durable.
//!
//! Two things about this domain that read like bugs and are not:
//!
//! - **There is no project scoping anywhere.** `coder_chat_threads` has no
//!   `project_id` and no handler calls `assert_token_project_access`, so a
//!   workspace token holding `chat:write` sees every coder thread on the box.
//!   Ported verbatim; narrowing it is a product decision, not a port one.
//! - **Two of the GETs write.** `_resolve_thread(None)` falls through to
//!   `_create_thread_row`, which commits — so `GET /chat/thread` and
//!   `GET /chat/context-usage` INSERT a `"New session"` row on an empty
//!   database and return it. Answering them as pure reads would diverge on the
//!   first call against a fresh DB, and nothing tests it.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::FromRow;
use tokio::sync::mpsc::UnboundedSender;

use crate::auth::Principal;
use crate::chat_thread_title::{
    await_smart_title, fallback_title_from_message, is_placeholder_title, start_smart_title_task,
};
use crate::chat_usage::{
    estimate_context_usage, merge_llm_usages, ContextInputs, ContextUsageOut, LlmStepUsageOut,
};
use crate::coder_loop::{
    parse_tool_calls_raw, run_agent_turn, sse, Emitter, ToolCall, TurnOptions, TurnOutcome, TurnStop,
};
use crate::coder_tools::{make_executor, Executor};
use crate::context_budget::{tool_result_soft_cap_tokens, truncate_text_to_tokens};
use crate::dag_schema::python_json;
use crate::error::ApiError;
use crate::wire::{iso_from_sql, parse_body_or_default, parse_body_typed, sql_now};
use crate::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/coder/chat/threads", get(threads_list).post(threads_create))
        .route("/api/v1/coder/chat/context-usage", get(context_usage))
        .route("/api/v1/coder/chat/thread", get(thread_get))
        .route("/api/v1/coder/chat/thread/{thread_id}", delete(thread_delete))
        .route("/api/v1/coder/chat/send", post(chat_send))
        .route("/api/v1/coder/chat/stream", post(chat_stream))
        .route("/api/v1/coder/chat/retry", post(chat_retry))
        .route("/api/v1/coder/chat/approve", post(chat_approve))
        .route("/api/v1/coder/chat/tool-result", post(chat_tool_result))
}

/// `require_scope(principal, "chat:write")` — every route in this domain
/// checks the same one, including the reads.
fn require_chat_write(principal: &Principal) -> Result<(), ApiError> {
    if principal.has_scope("chat:write") {
        return Ok(());
    }
    Err(ApiError {
        status: StatusCode::FORBIDDEN,
        code: "INSUFFICIENT_SCOPE",
        message: "Token lacks required scope 'chat:write'.".to_string(),
        extra: None,
    })
}

const DEFAULT_TITLE: &str = "New session";

/// `CODER_SYSTEM_PROMPT`, byte-identical — it is tokenized into every
/// `context_usage` body, so a drifted character is a changed number.
const CODER_SYSTEM_PROMPT: &str = "You are a coding assistant working directly in the user's workspace via tools.\n\
Rules:\n\
- All paths are relative to the workspace root.\n\
- Explore before you change: use list_dir and read_file to understand code before write_file.\n\
- write_file replaces the whole file; always read a file first and write back its full updated content.\n\
- Prefer small, targeted changes. Do not rewrite files you were not asked to touch.\n\
- When done, summarize what you changed and why in a short final answer.";

/// `executor.TOOL_SPECS`, embedded rather than rebuilt from a Rust structure.
///
/// It is counted as `estimate_tokens(json.dumps(tools, ensure_ascii=False))`,
/// so both the key **order** and the exact strings change the `tools` figure in
/// the response body. `serde_json`'s crate-wide `preserve_order` is what keeps
/// the order; parsing one constant keeps the strings.
const TOOL_SPECS_JSON: &str = r#"[
  {"type": "function", "function": {"name": "read_file", "description": "Read a text file from the workspace. Path is relative to the workspace root.", "parameters": {"type": "object", "properties": {"path": {"type": "string", "description": "Relative file path, e.g. 'src/app.py'"}}, "required": ["path"]}}},
  {"type": "function", "function": {"name": "write_file", "description": "Create or overwrite a text file in the workspace. Parent directories are created automatically.", "parameters": {"type": "object", "properties": {"path": {"type": "string", "description": "Relative file path"}, "content": {"type": "string", "description": "Full new file content"}}, "required": ["path", "content"]}}},
  {"type": "function", "function": {"name": "list_dir", "description": "List entries in a workspace directory. Directories end with '/'.", "parameters": {"type": "object", "properties": {"path": {"type": "string", "description": "Relative directory path; omit or '.' for the root"}}, "required": []}}},
  {"type": "function", "function": {"name": "search", "description": "Find which files contain a literal string, case-insensitively. Use this to locate code instead of reading files one at a time.", "parameters": {"type": "object", "properties": {"query": {"type": "string", "description": "Literal text to find, e.g. 'def send_message'"}}, "required": ["query"]}}},
  {"type": "function", "function": {"name": "repo_map", "description": "List the top-level definitions of every source file in the workspace (Python, Rust, JavaScript/TypeScript). Use this to see what exists and where a name lives before reading anything.", "parameters": {"type": "object", "properties": {}, "required": []}}},
  {"type": "function", "function": {"name": "run_command", "description": "Run a shell command in the workspace root and return stdout/stderr. Only available when command execution is enabled for the session.", "parameters": {"type": "object", "properties": {"command": {"type": "string", "description": "Shell command, e.g. 'pytest -q'"}}, "required": ["command"]}}}
]"#;

pub(crate) fn tool_specs() -> Vec<Value> {
    serde_json::from_str(TOOL_SPECS_JSON).expect("TOOL_SPECS_JSON is valid JSON")
}

/// `is_placeholder_title(..., placeholders=frozenset({"New session"}))` — this
/// domain overrides the shared default, so `"New chat"` is a real title here.
const CODER_PLACEHOLDERS: [&str; 1] = [DEFAULT_TITLE];

/// `_LEGACY_MODE_LABELS`.
const LEGACY_MODE_LABELS: [&str; 5] = ["plan", "debug", "multitask", "ask", "auto"];

/// Caps on a caller-supplied `tools` list. Not ported from anything — Python
/// had no such field. Both are far above any honest client (this crate sends
/// six specs in ~2 KB) and far below a list that would crowd out the
/// conversation or the provider's own body limit.
const MAX_REQUEST_TOOLS: usize = 64;
const MAX_REQUEST_TOOLS_BYTES: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

#[derive(FromRow, Clone)]
struct ThreadRow {
    id: i64,
    title: Option<String>,
    workspace_root: Option<String>,
    messages_json: Option<String>,
    pending_call_json: Option<String>,
    model: Option<String>,
    created_at: String,
    updated_at: String,
}

pub const THREAD_COLUMNS: &str = "CAST(id AS BIGINT) AS id, title, workspace_root, messages_json, pending_call_json, model, \
     CAST(created_at AS TEXT) AS created_at, CAST(updated_at AS TEXT) AS updated_at";

impl ThreadRow {
    /// `get_messages`: best-effort, an undecodable blob reads as empty rather
    /// than 500ing — same discipline as every other JSON column in this crate.
    fn messages(&self) -> Vec<Value> {
        self.messages_json
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .and_then(|v| match v {
                Value::Array(a) => Some(a),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// `get_pending_call`: an object or nothing, never a failure.
    fn pending_call(&self) -> Option<Value> {
        self.pending_call_json
            .as_deref()
            .filter(|raw| !raw.is_empty())
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .filter(Value::is_object)
    }

    fn display_title(&self) -> String {
        self.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| DEFAULT_TITLE.to_string())
    }
}

/// `CODER_WORKSPACE_ROOT`, read per call the way Python reads it.
fn default_workspace_root() -> Option<String> {
    std::env::var("CODER_WORKSPACE_ROOT").ok().map(|r| r.trim().to_string()).filter(|r| !r.is_empty())
}

async fn create_thread_row(
    state: &AppState,
    title: Option<&str>,
    workspace_root: Option<&str>,
) -> Result<ThreadRow, ApiError> {
    let now = sql_now();
    let title = title.unwrap_or(DEFAULT_TITLE);
    let id: i64 = sqlx::query_scalar(&crate::db::sql(
        "INSERT INTO coder_chat_threads (title, workspace_root, created_at, updated_at) \
         VALUES (?, ?, ?, ?) RETURNING CAST(id AS BIGINT)", state.backend)
    )
    .bind(title)
    .bind(workspace_root)
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.any)
    .await?;
    Ok(ThreadRow {
        id,
        title: Some(title.to_string()),
        workspace_root: workspace_root.map(str::to_string),
        // `set_messages([])` stores NULL, not "[]" — an empty list is falsy.
        messages_json: None,
        pending_call_json: None,
        model: None,
        created_at: now.clone(),
        updated_at: now,
    })
}

async fn get_thread_by_id(state: &AppState, thread_id: i64) -> Result<ThreadRow, ApiError> {
    sqlx::query_as(&crate::db::sql(&format!("SELECT {THREAD_COLUMNS} FROM coder_chat_threads WHERE id = ?"), state.backend))
        .bind(thread_id)
        .fetch_optional(&state.any)
        .await?
        .ok_or_else(|| ApiError::not_found("Coder thread not found"))
}

/// `_resolve_thread`: a named thread, else the most recently updated one, else
/// **a freshly created row** — the insert-on-read the module docs call out.
async fn resolve_thread(state: &AppState, thread_id: Option<i64>) -> Result<ThreadRow, ApiError> {
    if let Some(thread_id) = thread_id {
        return get_thread_by_id(state, thread_id).await;
    }
    let row: Option<ThreadRow> = sqlx::query_as(&crate::db::sql(&format!(
        "SELECT {THREAD_COLUMNS} FROM coder_chat_threads ORDER BY updated_at DESC LIMIT 1"
    ), state.backend))
    .fetch_optional(&state.any)
    .await?;
    match row {
        Some(row) => Ok(row),
        None => create_thread_row(state, None, default_workspace_root().as_deref()).await,
    }
}

// ---------------------------------------------------------------------------
// Context usage — `_coder_context_usage`
// ---------------------------------------------------------------------------

/// `_llm_messages_from_history`, minus the trailing user turn.
///
/// Every history entry is rebuilt field by field rather than passed through:
/// Python keeps `role`, `content` (defaulting to `""`), `tool_calls` when
/// truthy, and `tool_call_id` only on a `tool` row — a stored `usage` blob, in
/// particular, is dropped here and so is not tokenized.
fn llm_messages_from_history(history: &[Value]) -> Vec<Value> {
    llm_messages_from_history_with_mode(history, None)
}

/// `_compose_system_prompt`: the constant, plus the client's per-turn mode
/// guidance as a second paragraph. Merged into the *system* prompt rather than
/// stored as user content, which is why it never appears in the transcript.
fn compose_system_prompt(mode_instruction: Option<&str>) -> String {
    match mode_instruction.map(str::trim).filter(|m| !m.is_empty()) {
        None => CODER_SYSTEM_PROMPT.to_string(),
        Some(extra) => format!("{CODER_SYSTEM_PROMPT}\n\n{extra}"),
    }
}

fn llm_messages_from_history_with_mode(history: &[Value], mode: Option<&str>) -> Vec<Value> {
    let mut out = vec![json!({ "role": "system", "content": compose_system_prompt(mode) })];
    for h in history {
        let Some(obj) = h.as_object() else { continue };
        let role = obj.get("role").and_then(Value::as_str).unwrap_or("user");
        let content = obj.get("content").filter(|c| !c.is_null()).cloned().unwrap_or(json!(""));
        let mut m = Map::new();
        m.insert("role".into(), json!(role));
        m.insert("content".into(), content);
        if let Some(calls) = obj.get("tool_calls").filter(|c| crate::action_orchestrator::py_truthy(c)) {
            m.insert("tool_calls".into(), calls.clone());
        }
        if role == "tool" {
            if let Some(id) = obj.get("tool_call_id").filter(|v| crate::action_orchestrator::py_truthy(v)) {
                m.insert("tool_call_id".into(), id.clone());
            }
        }
        out.push(Value::Object(m));
    }
    out
}

fn coder_context_usage(llm_messages: &[Value]) -> ContextUsageOut {
    // `_system_prompt_from_messages`: the first system row, else the constant.
    let system = llm_messages
        .iter()
        .find(|m| m.get("role").and_then(Value::as_str) == Some("system"))
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or(CODER_SYSTEM_PROMPT)
        .to_string();
    let conversation: Vec<Value> = llm_messages
        .iter()
        .filter(|m| m.get("role").and_then(Value::as_str) != Some("system"))
        .cloned()
        .collect();
    let tools = tool_specs();
    estimate_context_usage(&ContextInputs {
        system_prompt: Some(&system),
        tools: Some(&tools),
        conversation_messages: Some(&conversation),
        ..Default::default()
    })
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

async fn threads_list(
    State(state): State<Arc<AppState>>,
    principal: Principal,
) -> Result<Response, ApiError> {
    require_chat_write(&principal)?;
    let rows: Vec<ThreadRow> = sqlx::query_as(&crate::db::sql(&format!(
        "SELECT {THREAD_COLUMNS} FROM coder_chat_threads ORDER BY updated_at DESC"
    ), state.backend))
    .fetch_all(&state.any)
    .await?;

    let threads: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            let messages = row.messages();
            // `str(m["content"])[:120]` over the last truthy user row. Python
            // slices by character, not byte.
            let preview = messages
                .iter()
                .rev()
                .find_map(|m| {
                    let obj = m.as_object()?;
                    if obj.get("role").and_then(Value::as_str) != Some("user") {
                        return None;
                    }
                    let content = obj.get("content").filter(|c| crate::action_orchestrator::py_truthy(c))?;
                    let text = match content {
                        Value::String(s) => s.clone(),
                        other => crate::todos::python_str(other).as_str().unwrap_or_default().to_string(),
                    };
                    Some(text.chars().take(120).collect::<String>())
                })
                .unwrap_or_default();
            json!({
                "id": row.id,
                "title": row.display_title(),
                "workspace_root": row.workspace_root,
                "message_count": messages.len(),
                "preview": preview,
                "created_at": iso_from_sql(&row.created_at),
                "updated_at": iso_from_sql(&row.updated_at),
            })
        })
        .collect();

    Ok(Json(json!({ "threads": threads })).into_response())
}

#[derive(Deserialize, Default)]
struct ThreadCreateRequest {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    workspace_root: Option<String>,
}

async fn threads_create(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    // Raw bytes, not `Option<Json<ThreadCreateRequest>>` — see
    // `require_body`'s comment.
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    require_chat_write(&principal)?;
    let req: ThreadCreateRequest = parse_body_or_default(&body)?;
    let mut errors = Vec::new();
    check_len(&mut errors, "title", req.title.as_deref(), 128);
    check_len(&mut errors, "workspace_root", req.workspace_root.as_deref(), 1024);
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    // `create_thread` defaults an empty title to "New session"; the workspace
    // root is *not* defaulted here, unlike the resolve-on-read path.
    let title = req.title.filter(|t| !t.is_empty());
    let row = create_thread_row(&state, title.as_deref(), req.workspace_root.as_deref()).await?;
    Ok(Json(json!({
        "thread_id": row.id,
        "title": row.display_title(),
        "workspace_root": row.workspace_root,
    }))
    .into_response())
}

fn check_len(errors: &mut Vec<Value>, field: &str, value: Option<&str>, max: usize) {
    let Some(value) = value else { return };
    if value.chars().count() > max {
        errors.push(ApiError::field_error_at(
            &[field],
            "string_too_long",
            &format!("String should have at most {max} characters"),
        ));
    }
}

#[derive(Deserialize)]
struct ThreadIdQuery {
    #[serde(default)]
    thread_id: Option<String>,
}

/// FastAPI's `Query(default=None, ge=1)`: absent is fine, present must parse
/// as an integer and be >= 1. The `input`/`ctx` fields of the pydantic entry
/// are the documented gap.
fn parse_thread_id(raw: Option<String>) -> Result<Option<i64>, ApiError> {
    let Some(raw) = raw else { return Ok(None) };
    // Built inline rather than through `field_error_at`, which prepends
    // `"body"` to the location — this one is a query parameter.
    let parsed: i64 = raw.trim().parse().map_err(|_| {
        ApiError::validation(vec![json!({
            "type": "int_parsing",
            "loc": ["query", "thread_id"],
            "msg": "Input should be a valid integer, unable to parse string as an integer",
        })])
    })?;
    if parsed < 1 {
        return Err(ApiError::validation(vec![json!({
            "type": "greater_than_equal",
            "loc": ["query", "thread_id"],
            "msg": "Input should be greater than or equal to 1",
        })]));
    }
    Ok(Some(parsed))
}

async fn context_usage(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<ThreadIdQuery>,
) -> Result<Response, ApiError> {
    require_chat_write(&principal)?;
    let thread_id = parse_thread_id(q.thread_id)?;
    let thread = resolve_thread(&state, thread_id).await?;
    let llm_messages = llm_messages_from_history(&thread.messages());
    Ok(Json(coder_context_usage(&llm_messages)).into_response())
}

async fn thread_get(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<ThreadIdQuery>,
) -> Result<Response, ApiError> {
    require_chat_write(&principal)?;
    let thread_id = parse_thread_id(q.thread_id)?;
    let thread = resolve_thread(&state, thread_id).await?;
    let messages = thread.messages();
    let llm_messages = llm_messages_from_history(&messages);
    let usage = coder_context_usage(&llm_messages);
    Ok(Json(json!({
        "thread_id": thread.id,
        "title": thread.display_title(),
        "workspace_root": thread.workspace_root,
        "model": thread.model,
        "messages": messages,
        "context_window": usage.context_window,
        "context_usage": usage,
    }))
    .into_response())
}

async fn thread_delete(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Path(thread_id): Path<i64>,
) -> Result<Response, ApiError> {
    require_chat_write(&principal)?;
    // 404s a missing row before deleting, the way `_get_thread_by_id` does.
    get_thread_by_id(&state, thread_id).await?;
    sqlx::query(&crate::db::sql("DELETE FROM coder_chat_threads WHERE id = ?", state.backend))
        .bind(thread_id)
        .execute(&state.any)
        .await?;
    Ok(Json(json!({ "thread_id": thread_id, "deleted": true })).into_response())
}

// ---------------------------------------------------------------------------
// The agent loop's shared pieces
// ---------------------------------------------------------------------------

/// `_unwrap_legacy_wrapped_user_message` + `_extract_legacy_mode_instruction`,
/// as one pass — `_resolve_user_turn`.
///
/// An explicit `mode_instruction` wins outright. Failing that, a message that
/// still arrives in the old `[plan mode]\n<instruction>\n\n<text>` wrapper is
/// unwrapped, so what is persisted is what the user typed and the instruction
/// goes to the system prompt where the new field would have put it.
fn resolve_user_turn(message: &str, mode_instruction: Option<&str>) -> (String, Option<String>) {
    if let Some(explicit) = mode_instruction.map(str::trim).filter(|m| !m.is_empty()) {
        return (message.to_string(), Some(explicit.to_string()));
    }
    for label in LEGACY_MODE_LABELS {
        let prefix = format!("[{label} mode]\n");
        let Some(rest) = message.strip_prefix(&prefix) else { continue };
        let Some((instruction, text)) = rest.split_once("\n\n") else { continue };
        let instruction = instruction.trim();
        return (
            text.to_string(),
            (!instruction.is_empty()).then(|| instruction.to_string()),
        );
    }
    (message.to_string(), None)
}

/// `_resolve_workspace`. The 400 text names all three places a root can come
/// from, because a client that hits it has to pick one.
fn resolve_workspace(thread: &ThreadRow, requested: Option<&str>) -> Result<String, ApiError> {
    let root = requested
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_string)
        .or_else(|| thread.workspace_root.clone().filter(|r| !r.is_empty()))
        .or_else(default_workspace_root);
    root.ok_or_else(|| {
        ApiError::bad_request(
            "No workspace_root configured. Pass workspace_root in the request, \
             set it on the thread, or set CODER_WORKSPACE_ROOT.",
        )
    })
}

/// `_require_no_pending`: a thread paused on an approval takes no new message
/// until that call is resolved.
fn require_no_pending(thread: &ThreadRow) -> Result<(), ApiError> {
    if thread.pending_call().is_some() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "Thread has a command awaiting approval. \
             Resolve it via /coder/chat/approve before sending a new message.",
        ));
    }
    Ok(())
}

/// `_truncate_history_for_retry`: keep through the last non-empty user turn and
/// drop the partial assistant/tool tail.
fn truncate_history_for_retry(history: &[Value]) -> Result<Vec<Value>, ApiError> {
    let last_user = history.iter().rposition(|m| {
        m.get("role").and_then(Value::as_str) == Some("user")
            && !message_text(m).trim().is_empty()
    });
    match last_user {
        Some(i) => Ok(history[..=i].to_vec()),
        None => Err(ApiError::bad_request("No user message to retry")),
    }
}

/// `str(m.get("content") or "")` — a container stringifies as its repr.
fn message_text(message: &Value) -> String {
    match message.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(other) if crate::action_orchestrator::py_truthy(other) => crate::todos::py_repr(other),
        _ => String::new(),
    }
}

/// `_is_command_override`: is `edited` an intentional full replacement?
///
/// The desktop's "Accept & remember" used to send the *rule pattern* (a bare
/// `powershell`, a bare `dir`) as `edited_command`, and that must not replace
/// the full command the model asked to run.
fn is_command_override(original: &str, edited: &str) -> bool {
    if edited.is_empty() || edited == original {
        return false;
    }
    if original.starts_with(&format!("{edited} ")) {
        return false;
    }
    let first = original.split_whitespace().next().unwrap_or("");
    edited != first
}

/// Everything a turn writes back to the row. Python assigns these onto the ORM
/// object and lets the unit of work flush them; there is one writer of this
/// table in either language, so a whole-row write is not the hazard here that
/// it was in todos.
///
/// Every field is written on every commit, so the routes whose `_persist` does
/// *not* assign a column (`approve` leaves `workspace_root` and `model` alone,
/// and `retry`'s pre-run truncation leaves both) pass the row's own value back
/// — including a `NULL`, which is why these are `Option` rather than defaulted
/// to an empty string.
struct Persisted {
    thread_id: i64,
    title: Option<String>,
    workspace_root: Option<String>,
    model: Option<String>,
    messages: Vec<Value>,
    pending: Option<Value>,
}

impl Persisted {
    fn display_title(&self) -> String {
        self.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| DEFAULT_TITLE.to_string())
    }

    async fn write(&self, state: &AppState) -> Result<(), sqlx::Error> {
        // `set_messages`/`set_pending_call`: falsy stores NULL, not "[]"/"null".
        let messages_json =
            (!self.messages.is_empty()).then(|| python_json(&self.messages, false));
        let pending_json = self.pending.as_ref().map(|p| python_json(p, false));
        sqlx::query(&crate::db::sql(
            "UPDATE coder_chat_threads \
             SET title = ?, workspace_root = ?, messages_json = ?, pending_call_json = ?, \
                 model = ?, updated_at = ? WHERE id = ?", state.backend)
        )
        .bind(&self.title)
        .bind(&self.workspace_root)
        .bind(&messages_json)
        .bind(&pending_json)
        .bind(&self.model)
        .bind(sql_now())
        .bind(self.thread_id)
        .execute(&state.any)
        .await
        .map(|_| ())
    }
}

/// `_done_payload`. Key order is the dict's, which is what a client parsing it
/// positionally would see and what a cross-render diffs.
fn done_payload(p: &Persisted, context_usage: &ContextUsageOut, usage: &Value) -> Value {
    json!({
        "thread_id": p.thread_id,
        "title": p.display_title(),
        "workspace_root": p.workspace_root,
        "context_window": context_usage.context_window,
        "messages": p.messages,
        "pending_call": p.pending,
        "context_usage": context_usage,
        "usage": usage,
    })
}

/// The tail every streaming route shares: persist, then say what happened.
///
/// Python's ordering, exactly — success is `_persist()` then `done`; an
/// `HTTPException` is `_persist()`, `error`, `done`; a client disconnect is
/// `_persist()` and silence, because there is nobody left to tell.
async fn finish_stream(
    state: &Arc<AppState>,
    tx: &UnboundedSender<String>,
    token_id: Option<i64>,
    p: &Persisted,
    context_usage: &ContextUsageOut,
    usage_steps: Vec<LlmStepUsageOut>,
    stop: Option<TurnStop>,
) {
    if let Err(e) = p.write(state).await {
        logd!("coder thread {} persist failed: {e}", p.thread_id);
    }
    if matches!(stop, Some(TurnStop::ClientGone)) {
        return;
    }
    if let Some(TurnStop::Failed(err)) = &stop {
        let _ = tx.send(sse("error", &json!({ "detail": err.message })));
    }
    let usage = serde_json::to_value(merge_llm_usages(usage_steps)).unwrap_or(Value::Null);
    // `_usage_tracking_stream` reads the `done` frame's own `usage` block, so
    // the counters move exactly when that frame does — not on a disconnect.
    crate::executor::record_api_token_usage(
        state,
        token_id,
        usage.get("total_tokens").and_then(Value::as_i64).unwrap_or(0),
        usage.get("cost_usd").and_then(Value::as_f64).unwrap_or(0.0),
        false,
    )
    .await;
    let _ = tx.send(sse("done", &done_payload(p, context_usage, &usage)));
}

/// The SSE response body: whatever the turn task and the title task send, until
/// both have finished.
///
/// This is `merge_title_sse_events` without its queue and its two workers — an
/// unbounded channel *is* the merge, and "keep waiting for the title after the
/// source closes" falls out of the stream ending when the last sender drops.
fn sse_response(rx: tokio::sync::mpsc::UnboundedReceiver<String>) -> Response {
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|frame| (Ok::<_, std::convert::Infallible>(bytes::Bytes::from(frame)), rx))
    });
    (
        [
            (header::CONTENT_TYPE, "text/event-stream; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
            (axum::http::HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        axum::body::Body::from_stream(stream),
    )
        .into_response()
}

/// The title half of `merge_title_sse_events`: resolve, persist if it changed,
/// emit one `title` frame.
fn spawn_title_worker(
    state: Arc<AppState>,
    tx: UnboundedSender<String>,
    task: Option<tokio::task::JoinHandle<Option<String>>>,
    thread_id: i64,
    fallback: String,
) {
    let Some(task) = task else { return };
    tokio::spawn(async move {
        let final_title = await_smart_title(Some(task), &fallback).await;
        // Python compares against its in-memory `thread.title`, which the turn
        // may already have written; reading the row is the same comparison
        // against whichever of the two got there first.
        let current: Option<String> =
            sqlx::query_scalar(&crate::db::sql("SELECT title FROM coder_chat_threads WHERE id = ?", state.backend))
                .bind(thread_id)
                .fetch_optional(&state.any)
                .await
                .ok()
                .flatten();
        if current.unwrap_or_default() != final_title {
            let _ = sqlx::query(&crate::db::sql("UPDATE coder_chat_threads SET title = ?, updated_at = ? WHERE id = ?", state.backend))
                .bind(&final_title)
                .bind(sql_now())
                .bind(thread_id)
                .execute(&state.any)
                .await;
        }
        let _ = tx.send(sse("title", &json!({ "thread_id": thread_id, "title": final_title })));
    });
}

// ---------------------------------------------------------------------------
// Request bodies
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct SendRequest {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    thread_id: Option<i64>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    workspace_root: Option<String>,
    #[serde(default)]
    allow_commands: bool,
    #[serde(default)]
    auto_approve_commands: bool,
    #[serde(default)]
    max_tokens: Option<i64>,
    #[serde(default)]
    delegate_tools: bool,
    /// A caller-supplied tool list, replacing [`tool_specs`] for this turn.
    ///
    /// Only meaningful to a delegating client: whatever the model calls comes
    /// straight back as a `tool_call` frame for that client to run. A
    /// non-delegating caller may still send one, and a name this crate's local
    /// executor does not know answers `"Error: unknown tool '…'."` as the tool
    /// result — the same thing a hallucinated call has always got.
    #[serde(default)]
    tools: Option<Vec<Value>>,
    #[serde(default)]
    mode_instruction: Option<String>,
    #[serde(default)]
    agent_mode: Option<String>,
    #[serde(default)]
    plan: bool,
}

impl SendRequest {
    /// `CoderChatSendRequest`'s constraints, in pydantic's own order: required
    /// first, then the length caps.
    fn validate(&self) -> Result<&str, ApiError> {
        let mut errors = Vec::new();
        check_len(&mut errors, "workspace_root", self.workspace_root.as_deref(), 1024);
        check_len(&mut errors, "mode_instruction", self.mode_instruction.as_deref(), 4096);
        check_len(&mut errors, "agent_mode", self.agent_mode.as_deref(), 32);
        match self.message.as_deref() {
            None => errors.insert(0, ApiError::field_error("message", "missing", "Field required")),
            Some(m) if m.is_empty() => errors.insert(
                0,
                ApiError::field_error(
                    "message",
                    "string_too_short",
                    "String should have at least 1 character",
                ),
            ),
            Some(_) => {}
        }
        self.validate_tools(&mut errors);
        if !errors.is_empty() {
            return Err(ApiError::validation(errors));
        }
        Ok(self.message.as_deref().unwrap_or_default())
    }

    /// `tools` goes into an upstream request body, so it is checked here rather
    /// than trusted: an entry that is not an object is a body the provider
    /// rejects with its own error, and the caps stop a list large enough to
    /// crowd out the conversation it is attached to.
    fn validate_tools(&self, errors: &mut Vec<Value>) {
        let Some(tools) = self.tools.as_deref() else { return };
        if tools.is_empty() {
            // An empty list means "no tools this turn", not "use the defaults":
            // `call_llm_step` reads `Some(&[])` as a tool-free step, which is
            // the same thing PLAN does.
            return;
        }
        if tools.len() > MAX_REQUEST_TOOLS {
            errors.push(ApiError::field_error(
                "tools",
                "too_long",
                &format!("List should have at most {MAX_REQUEST_TOOLS} items"),
            ));
            return;
        }
        if let Some(idx) = tools.iter().position(|t| !t.is_object()) {
            errors.push(ApiError::field_error(
                "tools",
                "model_type",
                &format!("Item {idx} is not a valid dictionary"),
            ));
            return;
        }
        let bytes: usize = tools.iter().map(|t| t.to_string().len()).sum();
        if bytes > MAX_REQUEST_TOOLS_BYTES {
            errors.push(ApiError::field_error(
                "tools",
                "too_long",
                &format!("Serialized tools should be at most {MAX_REQUEST_TOOLS_BYTES} bytes"),
            ));
        }
    }

    fn turn_options(&self) -> TurnOptions {
        TurnOptions {
            model: self.model.clone(),
            provider: self.provider.clone(),
            max_tokens: self.max_tokens,
            auto_approve_commands: self.auto_approve_commands,
            plan: self.plan,
            tools: self.tools.clone(),
        }
    }
}

#[derive(Deserialize, Default)]
struct RetryRequest {
    #[serde(default)]
    thread_id: Option<i64>,
    #[serde(flatten)]
    common: SendRequest,
}

#[derive(Deserialize, Default)]
struct ApprovalRequest {
    #[serde(default)]
    thread_id: Option<i64>,
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    approve: Option<bool>,
    #[serde(default)]
    edited_command: Option<String>,
    #[serde(flatten)]
    common: SendRequest,
}

/// `Field(ge=1)` on a required body integer.
fn require_thread_id(thread_id: Option<i64>) -> Result<i64, ApiError> {
    match thread_id {
        None => Err(ApiError::validation(vec![ApiError::field_error(
            "thread_id",
            "missing",
            "Field required",
        )])),
        Some(id) if id < 1 => Err(ApiError::validation(vec![ApiError::field_error(
            "thread_id",
            "greater_than_equal",
            "Input should be greater than or equal to 1",
        )])),
        Some(id) => Ok(id),
    }
}

/// A missing body is FastAPI's `{"loc": ["body"], "type": "missing"}` — the
/// shape `assistant.rs` already answers with. Takes raw bytes, not
/// `Option<Json<T>>`: axum's `Json` extractor only yields `None` for a
/// body-less request with no `Content-Type` at all — an empty body sent
/// *with* `application/json` (an argument-less POST from most clients) fails
/// to parse and axum answers its own plain-text 400 before the handler runs.
fn require_body<T: serde::de::DeserializeOwned>(body: &axum::body::Bytes) -> Result<T, ApiError> {
    if body.is_empty() {
        return Err(ApiError::validation(vec![json!({
            "type": "missing", "loc": ["body"], "msg": "Field required",
        })]));
    }
    parse_body_typed(body)
}

/// `api_auth.agent_platform_client_header` — what picks the delegated executor.
fn client_header(headers: &HeaderMap) -> Option<String> {
    crate::processes::client_header(headers)
}

/// `make_executor`, with its `ToolExecutionError` mapped to the 400 the routes
/// give it.
fn build_executor(
    root: &str,
    thread_id: i64,
    client_id: Option<&str>,
    allow_commands: bool,
    delegate_tools: bool,
) -> Result<Executor, ApiError> {
    make_executor(root, thread_id, client_id, allow_commands, delegate_tools)
        .map_err(|e| ApiError::bad_request(e.0))
}

/// Every streaming route refuses before it opens a stream, the way the routes
/// do rather than the way the service functions do.
fn require_master_key(state: &AppState) -> Result<(), ApiError> {
    if state.master_key.is_none() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "AGENT_PLATFORM_MASTER_KEY is not set.",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// POST /coder/chat/send — the non-streaming twin
// ---------------------------------------------------------------------------

async fn chat_send(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    // Raw bytes, not `Option<Json<SendRequest>>` — see `require_body`'s
    // comment.
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    require_chat_write(&principal)?;
    let body: SendRequest = require_body(&body)?;
    let message = body.validate()?.to_string();
    require_master_key(&state)?;

    let thread = resolve_thread(&state, body.thread_id).await?;
    let mut title = thread.title.clone();
    let mut fallback_title = thread.display_title();
    let mut title_task = None;
    if is_placeholder_title(title.as_deref(), &CODER_PLACEHOLDERS) {
        fallback_title = fallback_title_from_message(&message, DEFAULT_TITLE);
        title = Some(fallback_title.clone());
        title_task = start_smart_title_task(state.clone(), &message, body.model.as_deref());
    }
    require_no_pending(&thread)?;
    let root = resolve_workspace(&thread, body.workspace_root.as_deref())?;
    let executor = build_executor(
        &root,
        thread.id,
        client_header(&headers).as_deref(),
        body.allow_commands,
        body.delegate_tools,
    )?;

    let mut history = thread.messages();
    let (user_text, mode_addon) = resolve_user_turn(&message, body.mode_instruction.as_deref());
    // `agent_mode` is reserved for telemetry / future routing, and read by
    // nothing on either side.
    let mut llm_messages = llm_messages_from_history_with_mode(&history, mode_addon.as_deref());
    llm_messages.push(json!({ "role": "user", "content": user_text }));
    let context_usage = coder_context_usage(&llm_messages);

    let mut outcome = TurnOutcome::default();
    let stop = run_agent_turn(
        &state,
        &mut llm_messages,
        &executor,
        &body.turn_options(),
        None,
        &Emitter::Discard,
        &mut outcome,
    )
    .await;
    // `send_message` has no `except` around the turn: an HTTPException reaches
    // FastAPI and nothing is persisted. Only `stream_message` recovers.
    if let Err(TurnStop::Failed(err)) = stop {
        return Err(err);
    }

    history.push(json!({ "role": "user", "content": user_text }));
    history.extend(outcome.new_history);
    if is_placeholder_title(title.as_deref(), &CODER_PLACEHOLDERS) {
        title = Some(fallback_title.clone());
    }
    let persisted = Persisted {
        thread_id: thread.id,
        title,
        workspace_root: Some(executor.workspace_root_string()),
        model: body.model.clone().filter(|m| !m.is_empty()).or(thread.model.clone()),
        messages: history,
        pending: outcome.pending,
    };
    persisted.write(&state).await?;

    let final_title = await_smart_title(title_task, &fallback_title).await;
    if persisted.display_title() != final_title {
        sqlx::query(&crate::db::sql("UPDATE coder_chat_threads SET title = ?, updated_at = ? WHERE id = ?", state.backend))
            .bind(&final_title)
            .bind(sql_now())
            .bind(thread.id)
            .execute(&state.any)
            .await?;
    }

    let usage = serde_json::to_value(merge_llm_usages(outcome.usage_steps)).unwrap_or(Value::Null);
    crate::executor::record_api_token_usage(
        &state,
        principal.token_id,
        usage.get("total_tokens").and_then(Value::as_i64).unwrap_or(0),
        usage.get("cost_usd").and_then(Value::as_f64).unwrap_or(0.0),
        false,
    )
    .await;

    let mut payload = done_payload(&persisted, &context_usage, &usage);
    payload["title"] = json!(final_title);
    Ok(Json(payload).into_response())
}

// ---------------------------------------------------------------------------
// POST /coder/chat/stream
// ---------------------------------------------------------------------------

async fn chat_stream(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    // Raw bytes, not `Option<Json<SendRequest>>` — see `require_body`'s
    // comment.
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    require_chat_write(&principal)?;
    let body: SendRequest = require_body(&body)?;
    let message = body.validate()?.to_string();
    require_master_key(&state)?;

    // Resolved before the response starts, exactly as `stream_message` does:
    // the row (and, on an empty database, the row's creation) has to exist
    // before the title task can name it.
    let thread = resolve_thread(&state, body.thread_id).await?;
    let mut title = thread.title.clone();
    let mut fallback_title = thread.display_title();
    let mut title_task = None;
    if is_placeholder_title(title.as_deref(), &CODER_PLACEHOLDERS) {
        fallback_title = fallback_title_from_message(&message, DEFAULT_TITLE);
        title = Some(fallback_title.clone());
        title_task = start_smart_title_task(state.clone(), &message, body.model.as_deref());
    }

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    spawn_title_worker(state.clone(), tx.clone(), title_task, thread.id, fallback_title.clone());

    let client_id = client_header(&headers);
    let token_id = principal.token_id;
    tokio::spawn(async move {
        // Everything up to the executor is `_stream_body`'s own try/except: a
        // failure here is one `error` frame and **no `done`**, because nothing
        // ran and there is no state to report.
        let setup = require_no_pending(&thread)
            .and_then(|()| resolve_workspace(&thread, body.workspace_root.as_deref()))
            .and_then(|root| {
                build_executor(
                    &root,
                    thread.id,
                    client_id.as_deref(),
                    body.allow_commands,
                    body.delegate_tools,
                )
            });
        let executor = match setup {
            Ok(executor) => executor,
            Err(err) => {
                let _ = tx.send(sse("error", &json!({ "detail": err.message })));
                return;
            }
        };

        let mut history = thread.messages();
        let (user_text, mode_addon) = resolve_user_turn(&message, body.mode_instruction.as_deref());
        let mut llm_messages =
            llm_messages_from_history_with_mode(&history, mode_addon.as_deref());
        llm_messages.push(json!({ "role": "user", "content": user_text }));
        let context_usage = coder_context_usage(&llm_messages);

        let mut outcome = TurnOutcome::default();
        let stop = run_agent_turn(
            &state,
            &mut llm_messages,
            &executor,
            &body.turn_options(),
            None,
            &Emitter::Sse(&tx),
            &mut outcome,
        )
        .await;

        history.push(json!({ "role": "user", "content": user_text }));
        history.extend(std::mem::take(&mut outcome.new_history));
        if is_placeholder_title(title.as_deref(), &CODER_PLACEHOLDERS) {
            title = Some(fallback_title);
        }
        let persisted = Persisted {
            thread_id: thread.id,
            title,
            workspace_root: Some(executor.workspace_root_string()),
            model: body.model.clone().filter(|m| !m.is_empty()).or(thread.model.clone()),
            messages: history,
            pending: outcome.pending,
        };
        finish_stream(
            &state,
            &tx,
            token_id,
            &persisted,
            &context_usage,
            outcome.usage_steps,
            stop.err(),
        )
        .await;
    });

    Ok(sse_response(rx))
}

// ---------------------------------------------------------------------------
// POST /coder/chat/retry
// ---------------------------------------------------------------------------

async fn chat_retry(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    // Raw bytes, not `Option<Json<RetryRequest>>` — see `require_body`'s
    // comment.
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    require_chat_write(&principal)?;
    let body: RetryRequest = require_body(&body)?;
    let thread_id = require_thread_id(body.thread_id)?;
    let mut errors = Vec::new();
    check_len(&mut errors, "workspace_root", body.common.workspace_root.as_deref(), 1024);
    check_len(&mut errors, "mode_instruction", body.common.mode_instruction.as_deref(), 4096);
    check_len(&mut errors, "agent_mode", body.common.agent_mode.as_deref(), 32);
    // This route validates field by field rather than calling
    // `SendRequest::validate` — it has no `message` to require. `tools` still
    // reaches the turn through `body.common`, so its caps have to be repeated
    // here or the limit only exists on `/send` and `/stream`.
    body.common.validate_tools(&mut errors);
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }
    require_master_key(&state)?;

    // `stream_retry` resolves the thread by id — no insert-on-read here, and a
    // missing one is a 404 before the stream opens.
    let thread = get_thread_by_id(&state, thread_id).await?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let client_id = client_header(&headers);
    let token_id = principal.token_id;

    tokio::spawn(async move {
        let req = &body.common;
        let truncated = match truncate_history_for_retry(&thread.messages()) {
            Ok(truncated) => truncated,
            Err(err) => {
                let _ = tx.send(sse("error", &json!({ "detail": err.message })));
                return;
            }
        };
        // Python commits the truncation *before* the run, so a retry that dies
        // still leaves the thread ending on the user's message.
        let pre = Persisted {
            thread_id: thread.id,
            title: thread.title.clone(),
            workspace_root: thread.workspace_root.clone(),
            model: thread.model.clone(),
            messages: truncated.clone(),
            pending: None,
        };
        if let Err(e) = pre.write(&state).await {
            logd!("coder retry {} truncate failed: {e}", thread.id);
        }

        let setup = resolve_workspace(&thread, req.workspace_root.as_deref()).and_then(|root| {
            build_executor(
                &root,
                thread.id,
                client_id.as_deref(),
                req.allow_commands,
                req.delegate_tools,
            )
        });
        let executor = match setup {
            Ok(executor) => executor,
            Err(err) => {
                let _ = tx.send(sse("error", &json!({ "detail": err.message })));
                return;
            }
        };

        // `stream_retry` passes `mode_instruction` straight through rather than
        // running it past `_resolve_user_turn` — there is no new user message
        // for the legacy wrapper to be in.
        let mut llm_messages =
            llm_messages_from_history_with_mode(&truncated, req.mode_instruction.as_deref());
        let context_usage = coder_context_usage(&llm_messages);

        let mut outcome = TurnOutcome::default();
        let stop = run_agent_turn(
            &state,
            &mut llm_messages,
            &executor,
            &req.turn_options(),
            None,
            &Emitter::Sse(&tx),
            &mut outcome,
        )
        .await;

        let mut messages = truncated;
        messages.extend(std::mem::take(&mut outcome.new_history));
        let persisted = Persisted {
            thread_id: thread.id,
            // The retry path does not re-title: `_persist` there leaves
            // `thread.title` alone.
            title: thread.title.clone(),
            workspace_root: Some(executor.workspace_root_string()),
            model: req.model.clone().filter(|m| !m.is_empty()).or(thread.model.clone()),
            messages,
            pending: outcome.pending,
        };
        finish_stream(
            &state,
            &tx,
            token_id,
            &persisted,
            &context_usage,
            outcome.usage_steps,
            stop.err(),
        )
        .await;
    });

    Ok(sse_response(rx))
}

// ---------------------------------------------------------------------------
// POST /coder/chat/approve
// ---------------------------------------------------------------------------

async fn chat_approve(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    // Raw bytes, not `Option<Json<ApprovalRequest>>` — see `require_body`'s
    // comment.
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    require_chat_write(&principal)?;
    let body: ApprovalRequest = require_body(&body)?;
    let mut errors = Vec::new();
    if body.thread_id.is_none() {
        errors.push(ApiError::field_error("thread_id", "missing", "Field required"));
    }
    if body.call_id.is_none() {
        errors.push(ApiError::field_error("call_id", "missing", "Field required"));
    }
    if body.approve.is_none() {
        errors.push(ApiError::field_error("approve", "missing", "Field required"));
    }
    check_len(&mut errors, "mode_instruction", body.common.mode_instruction.as_deref(), 4096);
    check_len(&mut errors, "agent_mode", body.common.agent_mode.as_deref(), 32);
    // Same as `chat_retry`: the caps live on `validate_tools`, and this route
    // does not go through `SendRequest::validate`.
    body.common.validate_tools(&mut errors);
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }
    require_master_key(&state)?;

    let thread_id = body.thread_id.unwrap_or_default();
    let call_id = body.call_id.clone().unwrap_or_default();
    let approve = body.approve.unwrap_or_default();
    let thread = get_thread_by_id(&state, thread_id).await?;

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let client_id = client_header(&headers);
    let token_id = principal.token_id;

    tokio::spawn(async move {
        let req = &body.common;
        // Both of these are `error` frames with no `done`, and neither is an
        // HTTP status: a client that asks about a call the server has forgotten
        // gets a 200 stream saying so.
        let Some(pending) = thread.pending_call() else {
            let _ = tx.send(sse("error", &json!({ "detail": "No pending call on this thread." })));
            return;
        };
        let pending_call_id =
            pending.get("call_id").and_then(Value::as_str).unwrap_or_default().to_string();
        if pending_call_id != call_id {
            let detail = format!(
                "call_id mismatch: pending is {}",
                crate::todos::py_repr(pending.get("call_id").unwrap_or(&Value::Null))
            );
            let _ = tx.send(sse("error", &json!({ "detail": detail })));
            return;
        }

        // `resolve_pending_call` passes `None` for the requested root — the
        // thread's own workspace is the only one a resume may run in.
        let setup = resolve_workspace(&thread, None).and_then(|root| {
            build_executor(
                &root,
                thread.id,
                client_id.as_deref(),
                // `allow_commands` defaults to **True** on this route: the user
                // has just approved the command, so the session-level switch is
                // not consulted again.
                true,
                req.delegate_tools,
            )
        });
        let executor = match setup {
            Ok(executor) => executor,
            Err(err) => {
                let _ = tx.send(sse("error", &json!({ "detail": err.message })));
                return;
            }
        };

        let mut history = thread.messages();
        let mut llm_messages =
            llm_messages_from_history_with_mode(&history, req.mode_instruction.as_deref());
        let context_usage = coder_context_usage(&llm_messages);
        let mut outcome = TurnOutcome::default();

        let name = pending.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
        let mut args = pending.get("arguments").cloned().filter(Value::is_object).unwrap_or(json!({}));
        if name == "run_command" {
            if let Some(edited) = &body.edited_command {
                let original = args
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let edited = edited.trim();
                if !edited.is_empty() && edited != original && is_command_override(&original, edited)
                {
                    args["command"] = json!(edited);
                }
            }
        }

        let result = if approve {
            if !tx
                .send(sse(
                    "tool_call",
                    &json!({ "call_id": pending_call_id, "name": name, "arguments": args }),
                ))
                .is_ok()
            {
                return;
            }
            executor.execute(&state, &name, &args, &pending_call_id).await
        } else {
            "Error: command rejected by the user.".to_string()
        };
        let result = truncate_text_to_tokens(&result, tool_result_soft_cap_tokens());
        let tool_msg = json!({
            "role": "tool",
            "tool_call_id": pending_call_id,
            "name": name,
            "content": result,
        });
        llm_messages.push(tool_msg.clone());
        outcome.new_history.push(tool_msg);
        if tx.send(sse("tool_result", &json!({ "name": name, "content": result }))).is_err() {
            // A disconnect here still commits the tool result, the way the
            // generator's `finally` does.
            history.extend(std::mem::take(&mut outcome.new_history));
            let persisted = Persisted {
                thread_id: thread.id,
                title: thread.title.clone(),
                workspace_root: thread.workspace_root.clone(),
                model: thread.model.clone(),
                messages: history,
                pending: None,
            };
            let _ = persisted.write(&state).await;
            return;
        }

        let remaining = match pending.get("remaining") {
            Some(Value::Array(calls)) => parse_tool_calls_raw(calls),
            _ => Vec::<ToolCall>::new(),
        };
        // `model or thread.model` — this route falls back to whatever the
        // thread was last driven with, which the others do not.
        let mut options = req.turn_options();
        options.model = req.model.clone().filter(|m| !m.is_empty()).or(thread.model.clone());
        // `resolve_pending_call` never passes `plan`: the plan, if there was
        // one, is already in the history this resume picks up from.
        options.plan = false;

        let stop = run_agent_turn(
            &state,
            &mut llm_messages,
            &executor,
            &options,
            Some(remaining),
            &Emitter::Sse(&tx),
            &mut outcome,
        )
        .await;

        history.extend(std::mem::take(&mut outcome.new_history));
        let persisted = Persisted {
            thread_id: thread.id,
            title: thread.title.clone(),
            // `_persist` here does **not** write `workspace_root` or `model`,
            // unlike the other two — the resume keeps whatever the row had.
            workspace_root: thread.workspace_root.clone(),
            model: thread.model.clone(),
            messages: history,
            pending: outcome.pending,
        };
        finish_stream(
            &state,
            &tx,
            token_id,
            &persisted,
            &context_usage,
            outcome.usage_steps,
            stop.err(),
        )
        .await;
    });

    Ok(sse_response(rx))
}

// ---------------------------------------------------------------------------
// POST /coder/chat/tool-result — the unpark
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct ToolResultRequest {
    #[serde(default)]
    thread_id: Option<i64>,
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    result: Option<String>,
}

async fn chat_tool_result(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    // Raw bytes, not `Option<Json<ToolResultRequest>>` — see `require_body`'s
    // comment.
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    require_chat_write(&principal)?;
    let body: ToolResultRequest = require_body(&body)?;
    let thread_id = require_thread_id(body.thread_id)?;
    let call_id = match body.call_id.as_deref() {
        None => {
            return Err(ApiError::validation(vec![ApiError::field_error(
                "call_id",
                "missing",
                "Field required",
            )]))
        }
        Some("") => {
            return Err(ApiError::validation(vec![ApiError::field_error(
                "call_id",
                "string_too_short",
                "String should have at least 1 character",
            )]))
        }
        Some(id) => id,
    };
    // An unknown or already-resolved key is Python's `KeyError` → 404. The
    // detail is `str(e)`, and `str` of a one-argument exception is `repr` of
    // that argument — so the message arrives **wrapped in its own quotes**, and
    // in double quotes here because it contains an apostrophe. Found by
    // cross-rendering; nothing tests it.
    crate::coder_tools::resolve_desktop_tool_result(
        &state,
        thread_id,
        call_id,
        body.result.clone().unwrap_or_default(),
    )
    .map_err(|message| ApiError::not_found(crate::todos::py_repr(&Value::String(message))))?;
    Ok(Json(json!({ "ok": true })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The specs are tokenized into every `context_usage` body, so this
    /// guards both that the constant parses and that it still describes the
    /// six tools both executors implement.
    #[test]
    fn tool_specs_parse_and_cover_every_tool() {
        let specs = tool_specs();
        let names: Vec<&str> = specs
            .iter()
            .filter_map(|s| s.get("function")?.get("name")?.as_str())
            .collect();
        assert_eq!(
            names,
            ["read_file", "write_file", "list_dir", "search", "repo_map", "run_command"]
        );
    }

    /// A caller-supplied `tools` list reaches `TurnOptions` and is checked on
    /// the way. The three rejections are the ones that would otherwise reach a
    /// provider as a body it answers with its own error.
    #[test]
    fn caller_tools_are_validated_and_carried() {
        let send = |tools: Value| -> SendRequest {
            serde_json::from_value(json!({"message": "hi", "tools": tools})).expect("body")
        };
        let spec = json!({"type": "function", "function": {"name": "t"}});

        // Absent, empty and well-formed all pass; the first two are distinct
        // states, so `turn_options` must not collapse them.
        let bare: SendRequest = serde_json::from_value(json!({"message": "hi"})).expect("body");
        assert!(bare.validate().is_ok());
        assert!(bare.turn_options().tools.is_none());

        let empty = send(json!([]));
        assert!(empty.validate().is_ok());
        assert_eq!(empty.turn_options().tools.as_deref(), Some(&[][..]));

        let one = send(json!([spec]));
        assert!(one.validate().is_ok());
        assert_eq!(one.turn_options().tools.expect("tools").len(), 1);

        let too_many = send(Value::Array(vec![spec.clone(); MAX_REQUEST_TOOLS + 1]));
        assert!(too_many.validate().is_err());

        let not_objects = send(json!(["read_file"]));
        assert!(not_objects.validate().is_err());

        // Few enough entries to pass the count cap, large enough to fail bytes.
        let padding = "x".repeat(MAX_REQUEST_TOOLS_BYTES / 4);
        let fat = send(Value::Array(vec![json!({"pad": padding}); 8]));
        assert!(fat.validate().is_err());
    }

    /// `_llm_messages_from_history` drops everything it is not explicitly
    /// asked to keep — notably a persisted `usage` blob, which would otherwise
    /// be tokenized into the conversation figure.
    #[test]
    fn history_is_rebuilt_field_by_field_not_passed_through() {
        let history = vec![
            json!({"role": "user", "content": "hi", "usage": {"total_tokens": 99}}),
            json!({"role": "assistant", "content": "", "tool_calls": [{"id": "c1"}]}),
            json!({"role": "tool", "tool_call_id": "c1", "name": "read_file", "content": "x"}),
        ];
        let out = llm_messages_from_history(&history);
        assert_eq!(out[0]["role"], "system");
        assert_eq!(out[0]["content"], CODER_SYSTEM_PROMPT);

        assert_eq!(out[1], json!({"role": "user", "content": "hi"}));
        assert_eq!(out[2], json!({"role": "assistant", "content": "", "tool_calls": [{"id": "c1"}]}));
        // `name` is dropped on a tool row; `tool_call_id` is kept.
        assert_eq!(out[3], json!({"role": "tool", "content": "x", "tool_call_id": "c1"}));
    }

    /// An empty `tool_calls` list is falsy in Python, so it is not carried.
    #[test]
    fn falsy_tool_calls_and_missing_content_match_pythons_truthiness() {
        let history = vec![json!({"role": "assistant", "tool_calls": []})];
        let out = llm_messages_from_history(&history);
        assert_eq!(out[1], json!({"role": "assistant", "content": ""}));
    }

    /// The legacy `[plan mode]\n<instruction>\n\n<text>` wrapper: what is
    /// persisted is what the user typed, and the instruction goes where the
    /// explicit field would have put it.
    #[test]
    fn a_legacy_wrapped_message_is_unwrapped_and_its_instruction_hoisted() {
        assert_eq!(
            resolve_user_turn("[plan mode]\nBe careful\n\nfix the parser", None),
            ("fix the parser".to_string(), Some("Be careful".to_string()))
        );
        // An explicit instruction wins outright — no unwrapping at all.
        assert_eq!(
            resolve_user_turn("[plan mode]\nBe careful\n\nfix it", Some(" Focus ")),
            ("[plan mode]\nBe careful\n\nfix it".to_string(), Some("Focus".to_string()))
        );
        // A label that is not one of the five, and a wrapper with no blank
        // line, are both left alone.
        assert_eq!(resolve_user_turn("[wat mode]\nx\n\ny", None), ("[wat mode]\nx\n\ny".to_string(), None));
        assert_eq!(resolve_user_turn("[ask mode]\nnope", None), ("[ask mode]\nnope".to_string(), None));
        assert_eq!(resolve_user_turn("plain", Some("  ")), ("plain".to_string(), None));
    }

    #[test]
    fn the_mode_addendum_is_a_second_paragraph_of_the_system_prompt() {
        assert_eq!(compose_system_prompt(None), CODER_SYSTEM_PROMPT);
        assert_eq!(compose_system_prompt(Some("   ")), CODER_SYSTEM_PROMPT);
        assert_eq!(
            compose_system_prompt(Some(" Be terse ")),
            format!("{CODER_SYSTEM_PROMPT}\n\nBe terse")
        );
    }

    /// A retry re-runs from the last *non-empty* user turn, dropping whatever
    /// assistant/tool tail the failed attempt left behind.
    #[test]
    fn retry_truncates_to_the_last_real_user_turn() {
        let history = vec![
            json!({"role": "user", "content": "first"}),
            json!({"role": "assistant", "content": "answer"}),
            json!({"role": "user", "content": "second"}),
            json!({"role": "assistant", "content": "", "tool_calls": [{"id": "c1"}]}),
            json!({"role": "tool", "tool_call_id": "c1", "content": "out"}),
        ];
        let kept = truncate_history_for_retry(&history).unwrap();
        assert_eq!(kept.len(), 3);
        assert_eq!(kept[2]["content"], "second");

        // A blank user turn does not count, and a history with none at all is
        // the 400.
        let blank = vec![json!({"role": "user", "content": "  "})];
        assert!(truncate_history_for_retry(&blank).is_err());
        assert!(truncate_history_for_retry(&[]).is_err());
    }

    /// The desktop's "Accept & remember" used to send the *rule pattern* as
    /// `edited_command`; a shorthand must never replace the model's full
    /// command.
    #[test]
    fn only_a_real_rewrite_replaces_the_approved_command() {
        assert!(!is_command_override("pytest -q", "pytest"));
        assert!(!is_command_override("powershell -c ls", "powershell"));
        assert!(!is_command_override("pytest -q", "pytest -q"));
        assert!(!is_command_override("pytest -q", ""));
        assert!(is_command_override("pytest -q", "pytest -x"));
        assert!(is_command_override("rm -rf /", "echo no"));
    }

    /// `_resolve_workspace`'s precedence, and the 400 a caller has to act on.
    #[test]
    fn the_workspace_root_comes_from_the_request_then_the_thread() {
        let row = |root: Option<&str>| ThreadRow {
            id: 1,
            title: None,
            workspace_root: root.map(str::to_string),
            messages_json: None,
            pending_call_json: None,
            model: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert_eq!(resolve_workspace(&row(Some("/thread")), Some(" /req ")).unwrap(), "/req");
        assert_eq!(resolve_workspace(&row(Some("/thread")), Some("  ")).unwrap(), "/thread");
        let err = resolve_workspace(&row(None), None).unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err.message.starts_with("No workspace_root configured."), "{}", err.message);
    }

    /// A thread paused on an approval takes no new message until it is
    /// resolved — 409, not a silent second turn.
    #[test]
    fn a_pending_call_blocks_a_new_message() {
        let mut row = ThreadRow {
            id: 1,
            title: None,
            workspace_root: None,
            messages_json: None,
            pending_call_json: None,
            model: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert!(require_no_pending(&row).is_ok());
        row.pending_call_json = Some(r#"{"call_id": "c1"}"#.to_string());
        assert_eq!(require_no_pending(&row).unwrap_err().status, StatusCode::CONFLICT);
        // A blob that is not an object reads as no pending call, the way
        // `get_pending_call` does.
        row.pending_call_json = Some("[]".to_string());
        assert!(require_no_pending(&row).is_ok());
    }

    #[test]
    fn thread_id_query_rejects_zero_and_junk_but_allows_absent() {
        assert_eq!(parse_thread_id(None).unwrap(), None);
        assert_eq!(parse_thread_id(Some("3".into())).unwrap(), Some(3));
        assert!(parse_thread_id(Some("0".into())).is_err());
        assert!(parse_thread_id(Some("-1".into())).is_err());
        assert!(parse_thread_id(Some("abc".into())).is_err());
    }
}
