//! The Coder agent's thread CRUD, ported from `app/coder/routes.py` +
//! `service.py`.
//!
//! **Scope: routes 1-5 only** — `GET`/`POST /chat/threads`,
//! `GET /chat/context-usage`, `GET /chat/thread`, `DELETE /chat/thread/{id}`.
//! The five that run the agent loop (`send`, `stream`, `retry`, `approve`,
//! `tool-result`) stay proxied and **must move as one commit**: the delegated
//! tool call parks an in-process future in whichever server handled
//! `/chat/stream`, and the unpark has to land in that same process. See
//! `plan.md`'s coder scope note.
//!
//! **This split is deliberately short-lived.** While it holds, Python's
//! `_persist` writes the whole row back, so a `DELETE` served here mid-stream
//! leaves SQLAlchemy updating zero rows — a `StaleDataError` 500 rather than a
//! silent resurrection. Loud, but still a reason not to leave it sitting.
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
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::FromRow;

use crate::auth::Principal;
use crate::chat_usage::{estimate_context_usage, ContextInputs, ContextUsageOut};
use crate::error::ApiError;
use crate::wire::{iso_from_sql, sql_now};
use crate::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/coder/chat/threads", get(threads_list).post(threads_create))
        .route("/api/v1/coder/chat/context-usage", get(context_usage))
        .route("/api/v1/coder/chat/thread", get(thread_get))
        .route("/api/v1/coder/chat/thread/{thread_id}", delete(thread_delete))
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

fn tool_specs() -> Vec<Value> {
    serde_json::from_str(TOOL_SPECS_JSON).expect("TOOL_SPECS_JSON is valid JSON")
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

#[derive(FromRow, Clone)]
struct ThreadRow {
    id: i64,
    title: Option<String>,
    workspace_root: Option<String>,
    messages_json: Option<String>,
    model: Option<String>,
    created_at: String,
    updated_at: String,
}

const THREAD_COLUMNS: &str = "id, title, workspace_root, messages_json, model, \
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
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO coder_chat_threads (title, workspace_root, created_at, updated_at) \
         VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(title)
    .bind(workspace_root)
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.pool)
    .await?;
    Ok(ThreadRow {
        id,
        title: Some(title.to_string()),
        workspace_root: workspace_root.map(str::to_string),
        // `set_messages([])` stores NULL, not "[]" — an empty list is falsy.
        messages_json: None,
        model: None,
        created_at: now.clone(),
        updated_at: now,
    })
}

async fn get_thread_by_id(state: &AppState, thread_id: i64) -> Result<ThreadRow, ApiError> {
    sqlx::query_as(&format!("SELECT {THREAD_COLUMNS} FROM coder_chat_threads WHERE id = ?"))
        .bind(thread_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("Coder thread not found"))
}

/// `_resolve_thread`: a named thread, else the most recently updated one, else
/// **a freshly created row** — the insert-on-read the module docs call out.
async fn resolve_thread(state: &AppState, thread_id: Option<i64>) -> Result<ThreadRow, ApiError> {
    if let Some(thread_id) = thread_id {
        return get_thread_by_id(state, thread_id).await;
    }
    let row: Option<ThreadRow> = sqlx::query_as(&format!(
        "SELECT {THREAD_COLUMNS} FROM coder_chat_threads ORDER BY updated_at DESC LIMIT 1"
    ))
    .fetch_optional(&state.pool)
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
    let mut out = vec![json!({ "role": "system", "content": CODER_SYSTEM_PROMPT })];
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
    let rows: Vec<ThreadRow> = sqlx::query_as(&format!(
        "SELECT {THREAD_COLUMNS} FROM coder_chat_threads ORDER BY updated_at DESC"
    ))
    .fetch_all(&state.pool)
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
    body: Option<Json<ThreadCreateRequest>>,
) -> Result<Response, ApiError> {
    require_chat_write(&principal)?;
    let req = body.map(|Json(b)| b).unwrap_or_default();
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
    sqlx::query("DELETE FROM coder_chat_threads WHERE id = ?")
        .bind(thread_id)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({ "thread_id": thread_id, "deleted": true })).into_response())
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

    #[test]
    fn thread_id_query_rejects_zero_and_junk_but_allows_absent() {
        assert_eq!(parse_thread_id(None).unwrap(), None);
        assert_eq!(parse_thread_id(Some("3".into())).unwrap(), Some(3));
        assert!(parse_thread_id(Some("0".into())).is_err());
        assert!(parse_thread_id(Some("-1".into())).is_err());
        assert!(parse_thread_id(Some("abc".into())).is_err());
    }
}
