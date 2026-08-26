//! User-authored workflows, ported from `app/workflows/routes.py`.
//!
//! Rust serves the whole domain: the CRUD, the run history, the engine, the
//! interval scheduler and `POST /workflows/assist`. Rust owns both `workflows`
//! and `workflow_runs` now — the engine that writes runs moved with the
//! scheduler, because two pollers on one table would each fire every due
//! workflow.
//!
//! Tenancy here is the third kind in the platform: `client_id`, a namespace
//! rather than a security boundary. A workspace token gets `ws:{id}` derived
//! from the token itself; only the master key may name a namespace by header,
//! because a caller-supplied one could otherwise point at someone else's.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::FromRow;

use crate::auth::Principal;
use crate::error::{ApiError, PathId};
use crate::wire::{iso_from_sql, parse_body};
use crate::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/workflows", get(list_workflows).post(create_workflow))
        // Declared before `{workflow_id}` for the reader's sake; the router
        // prefers the literal segment either way.
        .route("/api/v1/workflows/assist", post(assist))
        .route(
            "/api/v1/workflows/{workflow_id}",
            get(get_workflow).put(update_workflow).delete(delete_workflow),
        )
        .route("/api/v1/workflows/{workflow_id}/run", post(run_workflow))
        .route("/api/v1/workflows/{workflow_id}/runs", get(list_runs))
        .route("/api/v1/workflows/{workflow_id}/runs/{run_id}", get(get_run))
}

const CLIENT_HEADER: &str = "x-agent-platform-client";

/// `action_client_scope`: the namespace this request may see.
fn client_scope(principal: &Principal, headers: &HeaderMap) -> Option<String> {
    if let Some(workspace_id) = principal.workspace_id {
        return Some(format!("ws:{workspace_id}"));
    }
    headers
        .get(CLIENT_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.chars().take(256).collect())
}

/// `_check_client_access`: a row with no `client_id` is public.
fn may_access(row_client_id: Option<&str>, scope: Option<&str>) -> bool {
    match row_client_id {
        None | Some("") => true,
        Some(owner) => scope.is_some_and(|s| s == owner),
    }
}

// ---------------------------------------------------------------------------
// Steps
// ---------------------------------------------------------------------------

const HTTP_METHODS: [&str; 6] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"];

/// `_STEP_ID_RE`: lowercase letter, then `[a-z0-9_-]`, max 64.
fn valid_step_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && id.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Per-step validation is a pydantic field/model validator, so it fails the
/// request at parse time with a 422 — unlike the cross-step checks below, which
/// the route raises as a 400.
fn validate_step(index: usize, step: &Value, errors: &mut Vec<Value>) {
    let idx = index.to_string();

    let id = step.get("id").and_then(Value::as_str);
    match id {
        None => errors.push(ApiError::field_error_at(&["steps", &idx, "id"], "missing", "Field required")),
        Some(id) if !valid_step_id(id) => errors.push(ApiError::field_error_at(
            &["steps", &idx, "id"],
            "value_error",
            "Value error, step id must be a slug: lowercase letter first, then [a-z0-9_-], max 64 chars",
        )),
        Some(_) => {}
    }

    let step_type = step.get("type").and_then(Value::as_str);
    match step_type {
        None => errors.push(ApiError::field_error_at(
            &["steps", &idx, "type"],
            "missing",
            "Field required",
        )),
        Some(t) if t != "http" && t != "action" => errors.push(ApiError::field_error_at(
            &["steps", &idx, "type"],
            "literal_error",
            "Input should be 'http' or 'action'",
        )),
        Some(_) => {}
    }

    let empty = Map::new();
    let params = step.get("params").and_then(Value::as_object).unwrap_or(&empty);
    let id = id.unwrap_or("");
    match step_type {
        Some("http") => {
            if !params.get("url").is_some_and(Value::is_string) {
                errors.push(ApiError::field_error_at(
                    &["steps", &idx],
                    "value_error",
                    &format!("Value error, step '{id}': http step requires params.url"),
                ));
            } else {
                let method = params
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or("GET")
                    .to_uppercase();
                if !HTTP_METHODS.contains(&method.as_str()) {
                    errors.push(ApiError::field_error_at(
                        &["steps", &idx],
                        "value_error",
                        &format!("Value error, step '{id}': unsupported method {method}"),
                    ));
                }
            }
        }
        Some("action") => {
            // Python tests these with `not params.get(...)`, so 0 and "" count
            // as missing just like a null does.
            let missing = |key: &str| {
                params.get(key).is_none_or(|v| v.is_null() || v == &json!("") || v == &json!(0))
            };
            if missing("action_set_id") || missing("action_id") {
                errors.push(ApiError::field_error_at(
                    &["steps", &idx],
                    "value_error",
                    &format!(
                        "Value error, step '{id}': action step requires params.action_set_id and params.action_id"
                    ),
                ));
            }
        }
        _ => {}
    }
}

/// `validate_steps`: the cross-step rules, raised by the route as a 400.
fn validate_steps(steps: &[Value]) -> Result<(), ApiError> {
    if steps.is_empty() {
        return Err(ApiError::bad_request("workflow requires at least one step"));
    }
    let mut seen = std::collections::HashSet::new();
    for step in steps {
        let id = step.get("id").and_then(Value::as_str).unwrap_or("");
        if !seen.insert(id) {
            return Err(ApiError::bad_request(format!("duplicate step id '{id}'")));
        }
    }
    Ok(())
}

/// Pydantic re-serializes each step through `WorkflowStep`, so the response
/// carries exactly `id`, `type`, `params` — anything else the caller stored is
/// dropped on the way out.
fn step_out(step: &Value) -> Value {
    json!({
        "id": step.get("id").cloned().unwrap_or(Value::Null),
        "type": step.get("type").cloned().unwrap_or(Value::Null),
        "params": step.get("params").cloned().unwrap_or_else(|| json!({})),
    })
}

fn step_result_out(result: &Value) -> Value {
    json!({
        "id": result.get("id").cloned().unwrap_or(Value::Null),
        "status": result.get("status").cloned().unwrap_or(Value::Null),
        "output": result.get("output").cloned().unwrap_or(Value::Null),
        "error": result.get("error").cloned().unwrap_or(Value::Null),
        "duration_ms": result.get("duration_ms").cloned().unwrap_or(Value::Null),
    })
}

fn json_array(raw: &str) -> Vec<Value> {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Array(a)) => a,
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct WorkflowRow {
    id: i64,
    client_id: Option<String>,
    name: String,
    description: Option<String>,
    steps_json: String,
    /// 0/1, not `bool` — see [`crate::db`]: the `Any` driver will not decode a
    /// SQLite boolean, and `BOOL!` selects it as an integer.
    enabled: i64,
    interval_seconds: Option<i64>,
    next_run_at: Option<String>,
    created_at: String,
    updated_at: String,
    user_id: Option<i64>,
}

/// Every id is `CAST(… AS BIGINT)` and every timestamp `CAST(… AS TEXT)`: the
/// `Any` driver refuses a timestamp column on either backend, and a Postgres
/// `integer` is int4 where these fields are `i64`. See [`crate::db`].
pub const WORKFLOW_COLUMNS: &str = concat!(
    "CAST(id AS BIGINT) AS id, client_id, name, description, steps_json, ",
    // Plain `enabled` here 500'd every list and detail read the moment this
    // module moved onto the `Any` pool: SQLite hands back a Bool the driver
    // will not decode.
    crate::BOOL!("enabled"),
    ", CAST(interval_seconds AS BIGINT) AS interval_seconds, \
     CAST(next_run_at AS TEXT) AS next_run_at, CAST(created_at AS TEXT) AS created_at, \
     CAST(updated_at AS TEXT) AS updated_at, CAST(user_id AS BIGINT) AS user_id"
);

fn workflow_out(row: &WorkflowRow) -> Value {
    json!({
        "id": row.id,
        "name": row.name,
        "description": row.description,
        "steps": json_array(&row.steps_json).iter().map(step_out).collect::<Vec<_>>(),
        "enabled": row.enabled != 0,
        "interval_seconds": row.interval_seconds,
        "next_run_at": row.next_run_at.as_deref().map(iso_from_sql),
        "created_at": iso_from_sql(&row.created_at),
        "updated_at": iso_from_sql(&row.updated_at),
        "user_id": row.user_id,
    })
}

#[derive(FromRow)]
struct RunRow {
    id: i64,
    workflow_id: i64,
    trigger: String,
    status: String,
    input_json: Option<String>,
    steps_json: String,
    error: Option<String>,
    started_at: String,
    finished_at: Option<String>,
}

pub const RUN_COLUMNS: &str = "CAST(id AS BIGINT) AS id, \
     CAST(workflow_id AS BIGINT) AS workflow_id, trigger, status, input_json, steps_json, \
     error, CAST(started_at AS TEXT) AS started_at, \
     CAST(finished_at AS TEXT) AS finished_at";

fn run_out(row: &RunRow) -> Value {
    let input = row
        .input_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|v| match v {
            Value::Object(o) => Some(o),
            _ => None,
        })
        .unwrap_or_default();
    json!({
        "id": row.id,
        "workflow_id": row.workflow_id,
        "trigger": row.trigger,
        "status": row.status,
        "input": input,
        "steps": json_array(&row.steps_json).iter().map(step_result_out).collect::<Vec<_>>(),
        "error": row.error,
        "started_at": iso_from_sql(&row.started_at),
        "finished_at": row.finished_at.as_deref().map(iso_from_sql),
    })
}

async fn accessible_workflow(
    state: &AppState,
    principal: &Principal,
    scope: Option<&str>,
    workflow_id: i64,
) -> Result<WorkflowRow, ApiError> {
    let row: Option<WorkflowRow> =
        sqlx::query_as(&crate::db::sql(
            &format!("SELECT {WORKFLOW_COLUMNS} FROM workflows WHERE id = ?"),
            state.backend,
        ))
            .bind(workflow_id)
            .fetch_optional(&state.any)
            .await?;
    let row = row.ok_or_else(|| ApiError::not_found("Workflow not found"))?;
    crate::identity::assert_user_row(principal, row.user_id)?;
    if !may_access(row.client_id.as_deref(), scope) {
        // 403, not 404: `client_id` is a namespace, not a tenancy boundary, and
        // the Python route says so too.
        return Err(ApiError::new(axum::http::StatusCode::FORBIDDEN, "Access denied"));
    }
    Ok(row)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct LimitQuery {
    #[serde(default = "default_workflow_limit")]
    limit: i64,
}

fn default_workflow_limit() -> i64 {
    50
}

async fn list_workflows(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    Query(q): Query<LimitQuery>,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    // The limit applies before the access filter, exactly as in Python: a page
    // can come back short rather than reaching further down the table.
    let rows: Vec<WorkflowRow> = if let Some(uid) = principal.scoped_user_id() {
        sqlx::query_as(&crate::db::sql(
            &format!("SELECT {WORKFLOW_COLUMNS} FROM workflows WHERE user_id = ? ORDER BY id DESC LIMIT ?"),
            state.backend,
        ))
        .bind(uid)
        .bind(q.limit)
        .fetch_all(&state.any)
        .await?
    } else {
        sqlx::query_as(&crate::db::sql(
            &format!("SELECT {WORKFLOW_COLUMNS} FROM workflows ORDER BY id DESC LIMIT ?"),
            state.backend,
        ))
        .bind(q.limit)
        .fetch_all(&state.any)
        .await?
    };

    let workflows: Vec<Value> = rows
        .iter()
        .filter(|row| may_access(row.client_id.as_deref(), scope.as_deref()))
        .map(workflow_out)
        .collect();
    Ok(Json(json!({ "workflows": workflows })).into_response())
}

#[derive(Deserialize)]
struct WorkflowCreate {
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    steps: Option<Vec<Value>>,
    #[serde(default = "enabled_default")]
    enabled: bool,
    #[serde(default)]
    interval_seconds: Option<i64>,
}

fn enabled_default() -> bool {
    true
}

/// `interval_seconds` is `ge=60`: a typo like `1` would hammer the scheduler.
fn check_interval(interval: Option<i64>, errors: &mut Vec<Value>) {
    if interval.is_some_and(|s| s < 60) {
        errors.push(ApiError::field_error(
            "interval_seconds",
            "greater_than_equal",
            "Input should be greater than or equal to 60",
        ));
    }
}

async fn create_workflow(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    // Raw bytes, not `Option<Json<WorkflowCreate>>`: axum's `Json` extractor
    // only yields `None` for a body-less request with no `Content-Type` at
    // all — an empty body sent *with* `application/json` (an argument-less
    // POST from most clients) fails to parse and axum answers its own
    // plain-text 400 before this handler runs.
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    let missing_body = || {
        ApiError::validation(vec![
            ApiError::field_error("name", "missing", "Field required"),
            ApiError::field_error("steps", "missing", "Field required"),
        ])
    };
    if body.is_empty() {
        return Err(missing_body());
    }
    let req: WorkflowCreate = serde_json::from_value(parse_body(&body)?).map_err(|e| {
        ApiError::validation(vec![ApiError::field_error_at(
            &["body"],
            "model_attributes_type",
            &e.to_string(),
        )])
    })?;

    let mut errors = Vec::new();
    if req.name.is_none() {
        errors.push(ApiError::field_error("name", "missing", "Field required"));
    }
    match &req.steps {
        None => errors.push(ApiError::field_error("steps", "missing", "Field required")),
        Some(steps) => {
            for (i, step) in steps.iter().enumerate() {
                validate_step(i, step, &mut errors);
            }
        }
    }
    check_interval(req.interval_seconds, &mut errors);
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    if crate::env_opt("AGENT_PLATFORM_REQUIRE_CLIENT_ID")
        .is_some_and(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        && scope.is_none()
    {
        return Err(ApiError::bad_request("client_id is required"));
    }

    let steps = req.steps.unwrap_or_default();
    validate_steps(&steps)?;

    let now = crate::wire::sql_now();
    let next_run_at = req.interval_seconds.map(|s| next_run_from(s));
    let id: i64 = sqlx::query_scalar(&crate::db::sql(
        "INSERT INTO workflows (client_id, name, description, steps_json, enabled, \
         interval_seconds, next_run_at, created_at, updated_at, user_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING CAST(id AS BIGINT)",
        state.backend,
    ))
    .bind(scope.as_deref())
    .bind(req.name.unwrap_or_default())
    .bind(req.description)
    .bind(Value::Array(steps).to_string())
    .bind(req.enabled)
    .bind(req.interval_seconds)
    .bind(next_run_at)
    .bind(&now)
    .bind(&now)
    .bind(crate::identity::stamp_user_id(&state, &principal))
    .fetch_one(&state.any)
    .await?;

    let row = accessible_workflow(&state, &principal, scope.as_deref(), id).await?;
    // 200, not 201: the Python route never set a status code for create.
    Ok(Json(workflow_out(&row)).into_response())
}

fn next_run_from(interval_seconds: i64) -> String {
    let at = chrono::Utc::now().naive_utc() + chrono::Duration::seconds(interval_seconds);
    at.format("%Y-%m-%d %H:%M:%S%.6f").to_string()
}

async fn get_workflow(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(workflow_id): PathId<i64>,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    let row = accessible_workflow(&state, &principal, scope.as_deref(), workflow_id).await?;
    Ok(Json(workflow_out(&row)).into_response())
}

async fn update_workflow(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(workflow_id): PathId<i64>,
    // Raw bytes, not `Option<Json<Value>>` — see `create_workflow`'s comment.
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    accessible_workflow(&state, &principal, scope.as_deref(), workflow_id).await?;

    let patch: Map<String, Value> = match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };

    let steps: Option<Vec<Value>> = match patch.get("steps") {
        Some(Value::Array(steps)) => Some(steps.clone()),
        _ => None,
    };
    let interval = patch.get("interval_seconds").and_then(Value::as_i64);

    let mut errors = Vec::new();
    if let Some(steps) = &steps {
        for (i, step) in steps.iter().enumerate() {
            validate_step(i, step, &mut errors);
        }
    }
    check_interval(interval, &mut errors);
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    if let Some(name) = patch.get("name").and_then(Value::as_str) {
        set_column(&state, workflow_id, "name", name.to_string()).await?;
    }
    if let Some(description) = patch.get("description").and_then(Value::as_str) {
        set_column(&state, workflow_id, "description", description.to_string()).await?;
    }
    if let Some(steps) = steps {
        validate_steps(&steps)?;
        set_column(&state, workflow_id, "steps_json", Value::Array(steps).to_string()).await?;
    }
    if let Some(enabled) = patch.get("enabled").and_then(Value::as_bool) {
        set_column(&state, workflow_id, "enabled", enabled).await?;
    }
    // `clear_interval` wins over `interval_seconds` when both are sent.
    if patch.get("clear_interval").and_then(Value::as_bool).unwrap_or(false) {
        set_column(&state, workflow_id, "interval_seconds", None::<i64>).await?;
        set_column(&state, workflow_id, "next_run_at", None::<String>).await?;
    } else if let Some(interval) = interval {
        set_column(&state, workflow_id, "interval_seconds", interval).await?;
        set_column(&state, workflow_id, "next_run_at", next_run_from(interval)).await?;
    }
    set_column(&state, workflow_id, "updated_at", crate::wire::sql_now()).await?;

    let row = accessible_workflow(&state, &principal, scope.as_deref(), workflow_id).await?;
    Ok(Json(workflow_out(&row)).into_response())
}

/// Column at a time, like todos: the engine writes this table from Python while
/// a run is in flight.
async fn set_column<T>(
    state: &AppState,
    workflow_id: i64,
    column: &str,
    value: T,
) -> Result<(), ApiError>
where
    T: for<'q> sqlx::Encode<'q, sqlx::Any> + sqlx::Type<sqlx::Any> + Send,
{
    let sql = format!("UPDATE workflows SET {column} = ? WHERE id = ?");
    sqlx::query(&crate::db::sql(&sql, state.backend))
        .bind(value)
        .bind(workflow_id)
        .execute(&state.any)
        .await?;
    Ok(())
}

async fn delete_workflow(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(workflow_id): PathId<i64>,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    accessible_workflow(&state, &principal, scope.as_deref(), workflow_id).await?;

    let mut tx = state.any.begin().await?;
    sqlx::query(&crate::db::sql("DELETE FROM workflow_runs WHERE workflow_id = ?", state.backend))
        .bind(workflow_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(&crate::db::sql("DELETE FROM workflows WHERE id = ?", state.backend))
        .bind(workflow_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(Json(json!({ "success": true })).into_response())
}

#[derive(Deserialize)]
struct RunLimitQuery {
    #[serde(default = "default_run_limit")]
    limit: i64,
}

fn default_run_limit() -> i64 {
    20
}

async fn list_runs(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(workflow_id): PathId<i64>,
    Query(q): Query<RunLimitQuery>,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    accessible_workflow(&state, &principal, scope.as_deref(), workflow_id).await?;

    let rows: Vec<RunRow> = sqlx::query_as(&crate::db::sql(
        &format!("SELECT {RUN_COLUMNS} FROM workflow_runs WHERE workflow_id = ? ORDER BY id DESC LIMIT ?"),
        state.backend,
    ))
    .bind(workflow_id)
    .bind(q.limit)
    .fetch_all(&state.any)
    .await?;

    Ok(Json(json!({ "runs": rows.iter().map(run_out).collect::<Vec<_>>() })).into_response())
}

/// Run now and return the finished run.
///
/// The JSON body — any shape — is exposed to steps as `{{trigger.body.*}}`.
/// This is the webhook-style trigger external apps call with a workspace token.
async fn run_workflow(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(workflow_id): PathId<i64>,
    raw: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    let workflow = accessible_workflow(&state, &principal, scope.as_deref(), workflow_id).await?;
    if workflow.enabled == 0 {
        return Err(ApiError::bad_request("Workflow is disabled"));
    }

    // An absent body is `{}`; a present one must be an object, the way FastAPI
    // reads `dict[str, Any] | None`.
    let input = if raw.is_empty() {
        Value::Object(Map::new())
    } else {
        match serde_json::from_slice::<Value>(&raw) {
            Ok(Value::Object(map)) => Value::Object(map),
            Ok(Value::Null) => Value::Object(Map::new()),
            _ => {
                return Err(ApiError::validation(vec![json!({
                    "type": "dict_type",
                    "loc": ["body"],
                    "msg": "Input should be a valid dictionary",
                })]))
            }
        }
    };

    let run_id = crate::workflow_engine::execute_workflow(
        &state,
        workflow_id,
        &workflow.steps_json,
        input,
        "api",
    )
    .await?;

    let row: RunRow = sqlx::query_as(&crate::db::sql(
        &format!("SELECT {RUN_COLUMNS} FROM workflow_runs WHERE id = ?"),
        state.backend,
    ))
        .bind(run_id)
        .fetch_one(&state.any)
        .await?;
    Ok(Json(run_out(&row)).into_response())
}

async fn get_run(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId((workflow_id, run_id)): PathId<(i64, i64)>,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    accessible_workflow(&state, &principal, scope.as_deref(), workflow_id).await?;

    let row: Option<RunRow> =
        sqlx::query_as(&crate::db::sql(
            &format!("SELECT {RUN_COLUMNS} FROM workflow_runs WHERE id = ?"),
            state.backend,
        ))
            .bind(run_id)
            .fetch_optional(&state.any)
            .await?;
    match row {
        Some(row) if row.workflow_id == workflow_id => Ok(Json(run_out(&row)).into_response()),
        _ => Err(ApiError::not_found("Run not found")),
    }
}

// ---------------------------------------------------------------------------
// POST /workflows/assist — ported from `app/workflows/assist.py`
// ---------------------------------------------------------------------------

/// `assist.py::SYSTEM_PROMPT`, verbatim.
const SYSTEM_PROMPT: &str = r#"You help users build and review workflow automations.

A workflow is a JSON array of steps, run strictly top to bottom. Each step:
  {"id": "<slug>", "type": "http" | "action", "params": {...}}

- "http" params: url (required), method (GET/POST/PUT/PATCH/DELETE/HEAD,
  default GET), headers (object), body (JSON), timeout_seconds.
- "action" params: action_set_id (int), action_id (string), arguments (object).
  Actions are pre-registered server-executed endpoints.
- Step ids are slugs: lowercase letter first, then [a-z0-9_-], unique.
- Templates pass data between steps: {{trigger.body.<path>}} is the JSON the
  caller sent when triggering the run; {{steps.<id>.output.body.<path>}} and
  {{steps.<id>.output.status}} read earlier http/action responses. Lists index
  numerically: {{steps.a.output.body.items.0.name}}. A string that is exactly
  one template keeps the referenced value's type.
- A failing step (non-2xx, timeout, missing template path) stops the run.

Respond with ONLY a JSON object, no markdown fences:
  {"reply": "<what you did or found, concise, plain text>",
   "steps": <full replacement steps array, or null>}

Set "steps" to null when the user asked a question or a review found nothing to
change. When you do return steps, return the COMPLETE array — it replaces the
draft wholesale. Never invent action_set_id/action_id values; use "http" steps
unless the user names a registered action.

Placeholder steps in the draft (e.g. a GET to https://example.com/api) are
editor boilerplate, not user intent: replace them, never keep them. Only
reference template paths a response will actually contain — if you do not know
a response's shape, do not build a step on an invented field; leave it out and
say so in the reply."#;

#[derive(Deserialize)]
struct AssistRequest {
    message: Option<String>,
    name: Option<String>,
    steps: Option<Vec<Value>>,
}

/// Every failure below the route is a 502 with this prefix: the Python handler
/// wraps its whole body in one `except Exception`.
fn unavailable(detail: impl std::fmt::Display) -> ApiError {
    ApiError::new(
        axum::http::StatusCode::BAD_GATEWAY,
        format!("Assistant unavailable: {detail}"),
    )
}

/// `llm_client._default_subagent_model`: `SUBAGENT_MODEL`, else `PLANNER_MODEL`,
/// else no `model` key at all so the proxy's own default answers.
///
/// Only the first variable *set* is consulted — Python sanitises `SUBAGENT_MODEL`
/// and returns that result, so a slug there does not fall through to the planner.
fn subagent_model() -> Option<String> {
    let raw = crate::env_opt("SUBAGENT_MODEL").or_else(|| crate::env_opt("PLANNER_MODEL"))?;
    crate::dag_schema::sanitize_llm_model_alias(&raw)
}

/// `_strip_fences`: a reasoning model's think-block preamble, then a markdown
/// json fence around the whole answer.
fn strip_fences(text: &str) -> String {
    // deepseek-r1, qwen3 and friends prefix the JSON with an inline
    // <think>…</think> block; it is deliberation, not answer.
    let body = match text.trim_start().strip_prefix("<think>") {
        Some(rest) => match rest.find("</think>") {
            Some(at) => &rest[at + "</think>".len()..],
            None => text,
        },
        None => text,
    };

    let body = body.trim();
    let Some(rest) = body.strip_prefix("```") else { return body.to_string() };
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    match rest.strip_suffix("```") {
        Some(inner) => inner.trim().to_string(),
        None => body.to_string(),
    }
}

/// `[WorkflowStep(**s) for s in raw_steps]` then `validate_steps`, returning the
/// text Python's `{e}` carries into the discarded-steps reply.
fn assist_steps(raw: &Value) -> Result<Vec<Value>, String> {
    let Some(items) = raw.as_array() else {
        // Python `**`-unpacks each element of whatever it was handed, so a
        // non-list raises TypeError before a single field is looked at.
        return Err("argument after ** must be a mapping".to_string());
    };
    for (index, step) in items.iter().enumerate() {
        let mut errors = Vec::new();
        validate_step(index, step, &mut errors);
        // ponytail: pydantic's `str(ValidationError)` wraps this same sentence in
        // a count header, a loc line, `[type=…, input_value=…]` and a docs URL.
        // The sentence a user reads matches; the envelope around it does not.
        if let Some(msg) = errors.first().and_then(|e| e.get("msg")).and_then(Value::as_str) {
            return Err(msg.to_string());
        }
    }
    // A plain ValueError in Python, so these two texts match exactly.
    validate_steps(items).map_err(|e| e.message)?;
    Ok(items.iter().map(step_out).collect())
}

/// `parse_assist_reply`: model output → response body.
///
/// A malformed or invalid answer becomes a plain reply rather than an error —
/// the user can just rephrase.
fn parse_assist_reply(content: &str) -> Value {
    let Ok(Value::Object(data)) = serde_json::from_str::<Value>(&strip_fences(content)) else {
        // Note the *raw* content, not the fence-stripped one.
        return json!({ "reply": content.trim(), "steps": Value::Null });
    };

    // ponytail: Python renders a non-string `reply` with `str()`, which for a
    // list or dict is a Python repr. Here anything but a string reads as absent.
    let reply = data.get("reply").and_then(Value::as_str).unwrap_or("").trim();
    let reply = if reply.is_empty() { "Done." } else { reply };

    match data.get("steps") {
        None | Some(Value::Null) => json!({ "reply": reply, "steps": Value::Null }),
        Some(raw) => match assist_steps(raw) {
            Ok(steps) => json!({ "reply": reply, "steps": steps }),
            Err(e) => json!({
                "reply": format!(
                    "{reply}\n\n(The suggested steps were invalid and were discarded: {e})"
                ),
                "steps": Value::Null,
            }),
        },
    }
}

/// `data["choices"][0]["message"]["content"]`, carrying the text Python's
/// `KeyError`/`IndexError` would have put in the 502.
fn completion_content(data: &Value) -> Result<&str, ApiError> {
    let choices = data.get("choices").ok_or_else(|| unavailable("'choices'"))?;
    let first = choices.get(0).ok_or_else(|| unavailable("list index out of range"))?;
    let message = first.get("message").ok_or_else(|| unavailable("'message'"))?;
    let content = message.get("content").ok_or_else(|| unavailable("'content'"))?;
    content.as_str().ok_or_else(|| {
        // A model that answered with tool calls only sends `"content": null`;
        // Python hands that straight to `re.sub`, which refuses it.
        unavailable("expected string or bytes-like object, got 'NoneType'")
    })
}

/// Chat-style help: generate, review or edit a steps array.
///
/// Stateless — the current draft travels in the request, the validated
/// replacement comes back. No workflow row is read or written, so the namespace
/// (`action_client_scope`) resolves to nothing this handler can consult; the
/// Python route takes the same dependency and ignores it too.
async fn assist(
    State(state): State<Arc<AppState>>,
    _principal: Principal,
    // Raw bytes, not `Option<Json<AssistRequest>>` — see `create_workflow`'s
    // comment.
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let missing_message =
        || ApiError::validation(vec![ApiError::field_error("message", "missing", "Field required")]);
    if body.is_empty() {
        return Err(missing_message());
    }
    let req: AssistRequest = serde_json::from_value(parse_body(&body)?).map_err(|e| {
        ApiError::validation(vec![ApiError::field_error_at(
            &["body"],
            "model_attributes_type",
            &e.to_string(),
        )])
    })?;
    let message = req.message.ok_or_else(missing_message)?;

    let mut user_parts = vec![message.trim().to_string()];
    if let Some(name) = req.name.as_deref().filter(|n| !n.is_empty()) {
        user_parts.push(format!("Workflow name: {name}"));
    }
    if let Some(steps) = &req.steps {
        // `json.dumps(steps, indent=2)`: same two-space layout, but Python's
        // `ensure_ascii=True` escapes non-ASCII where serde does not, and its
        // dicts keep the caller's key order where `serde_json::Map` sorts them.
        // Prompt text only — neither reaches the wire.
        let pretty = serde_json::to_string_pretty(steps).unwrap_or_default();
        user_parts.push(format!("Current steps:\n{pretty}"));
    }

    let (messages, _budget) = crate::context_budget::fit_chat_messages_for_request(vec![
        json!({ "role": "system", "content": SYSTEM_PROMPT }),
        json!({ "role": "user", "content": user_parts.join("\n\n") }),
    ]);

    let mut payload = Map::new();
    payload.insert("messages".into(), Value::Array(messages));
    payload.insert("temperature".into(), json!(0.2));
    if let Some(model) = subagent_model() {
        payload.insert("model".into(), json!(model));
    }
    payload.insert(
        "max_tokens".into(),
        json!(crate::context_budget::max_output_tokens_default()),
    );
    // `require_json=True`.
    payload.insert("response_format".into(), json!({ "type": "json_object" }));

    // ponytail: Python prefixes this with "LLM proxy request failed with HTTP
    // {status}." because it read an HTTP response off its own loopback. There is
    // no hop here, and `e.message` already says what went wrong.
    let data = crate::llm::complete_internal(&state, payload, crate::resources::Priority::Interactive)
        .await
        .map_err(|e| unavailable(e.message))?;

    Ok(Json(parse_assist_reply(completion_content(&data)?)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_ids_are_slugs() {
        assert!(valid_step_id("fetch"));
        assert!(valid_step_id("fetch-2_x"));
        assert!(!valid_step_id("Fetch"), "must start lowercase");
        assert!(!valid_step_id("2fetch"), "must start with a letter");
        assert!(!valid_step_id(""), "empty is not a slug");
        assert!(!valid_step_id(&"a".repeat(65)), "64 chars max");
    }

    #[test]
    fn public_rows_are_visible_without_a_namespace() {
        assert!(may_access(None, None));
        assert!(may_access(None, Some("ws:1")));
        assert!(may_access(Some("ws:1"), Some("ws:1")));
        assert!(!may_access(Some("ws:1"), Some("ws:2")));
        assert!(!may_access(Some("ws:1"), None));
    }

    #[test]
    fn step_output_drops_unknown_keys() {
        let step = json!({"id": "a", "type": "http", "params": {"url": "x"}, "extra": 1});
        assert_eq!(
            step_out(&step),
            json!({"id": "a", "type": "http", "params": {"url": "x"}})
        );
    }

    /// Nothing a model can answer with is an error here; the four shapes it
    /// actually produces all have to land as a reply.
    #[test]
    fn assist_reply_takes_whatever_the_model_answered() {
        // A fenced object parses, and its steps render like the CRUD's.
        let fenced = concat!(
            "```json\n",
            r#"{"reply": "added a fetch", "steps": [{"id": "a", "type": "http","#,
            r#" "params": {"url": "https://x"}, "extra": 1}]}"#,
            "\n```"
        );
        assert_eq!(
            parse_assist_reply(fenced),
            json!({
                "reply": "added a fetch",
                "steps": [{"id": "a", "type": "http", "params": {"url": "https://x"}}],
            })
        );

        // A think-block preamble is deliberation, not answer.
        assert_eq!(
            parse_assist_reply("<think>\nweigh it up\n</think>\n{\"reply\": \"hi\"}"),
            json!({ "reply": "hi", "steps": Value::Null })
        );

        // Prose is a reply, not a 502 — and it is the raw text that comes back.
        assert_eq!(
            parse_assist_reply("  I need the endpoint first.  "),
            json!({ "reply": "I need the endpoint first.", "steps": Value::Null })
        );

        // Invalid steps are discarded, with the reason appended to the reply.
        assert_eq!(
            parse_assist_reply(r#"{"reply": "here", "steps": [{"id": "a", "type": "http"}]}"#),
            json!({
                "reply": "here\n\n(The suggested steps were invalid and were discarded: \
                          Value error, step 'a': http step requires params.url)",
                "steps": Value::Null,
            })
        );
    }
}
