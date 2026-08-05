//! User-authored workflows, ported from `app/workflows/routes.py`.
//!
//! Split the same way todos is: Rust serves the CRUD and the run history,
//! Python still serves `POST /workflows/{id}/run` (the engine) and
//! `POST /workflows/assist` (the LLM). Rust owns `workflows`; it only ever
//! *reads* `workflow_runs`, which the engine writes — except on delete, where
//! the runs go with their workflow.
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
use crate::wire::iso_from_sql;
use crate::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/workflows", get(list_workflows).post(create_workflow))
        // Explicitly proxied, and it has to be declared: `assist` would
        // otherwise match `{workflow_id}` and answer 405 instead of falling
        // through to Python.
        .route("/api/v1/workflows/assist", post(crate::proxy::forward))
        .route(
            "/api/v1/workflows/{workflow_id}",
            get(get_workflow).put(update_workflow).delete(delete_workflow),
        )
        .route("/api/v1/workflows/{workflow_id}/runs", get(list_runs))
        .route("/api/v1/workflows/{workflow_id}/runs/{run_id}", get(get_run))
    // `{workflow_id}/run` is not declared, so it reaches Python's engine.
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
    enabled: bool,
    interval_seconds: Option<i64>,
    next_run_at: Option<String>,
    created_at: String,
    updated_at: String,
}

const WORKFLOW_COLUMNS: &str = "id, client_id, name, description, steps_json, enabled, \
     interval_seconds, next_run_at, created_at, updated_at";

fn workflow_out(row: &WorkflowRow) -> Value {
    json!({
        "id": row.id,
        "name": row.name,
        "description": row.description,
        "steps": json_array(&row.steps_json).iter().map(step_out).collect::<Vec<_>>(),
        "enabled": row.enabled,
        "interval_seconds": row.interval_seconds,
        "next_run_at": row.next_run_at.as_deref().map(iso_from_sql),
        "created_at": iso_from_sql(&row.created_at),
        "updated_at": iso_from_sql(&row.updated_at),
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

const RUN_COLUMNS: &str = "id, workflow_id, trigger, status, input_json, steps_json, error, \
     started_at, finished_at";

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
    scope: Option<&str>,
    workflow_id: i64,
) -> Result<WorkflowRow, ApiError> {
    let row: Option<WorkflowRow> =
        sqlx::query_as(&format!("SELECT {WORKFLOW_COLUMNS} FROM workflows WHERE id = ?"))
            .bind(workflow_id)
            .fetch_optional(&state.pool)
            .await?;
    let row = row.ok_or_else(|| ApiError::not_found("Workflow not found"))?;
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
    let rows: Vec<WorkflowRow> = sqlx::query_as(&format!(
        "SELECT {WORKFLOW_COLUMNS} FROM workflows ORDER BY id DESC LIMIT ?"
    ))
    .bind(q.limit)
    .fetch_all(&state.pool)
    .await?;

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
    body: Option<Json<WorkflowCreate>>,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    let Json(req) = body.ok_or_else(|| {
        ApiError::validation(vec![
            ApiError::field_error("name", "missing", "Field required"),
            ApiError::field_error("steps", "missing", "Field required"),
        ])
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
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO workflows (client_id, name, description, steps_json, enabled, \
         interval_seconds, next_run_at, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(scope.as_deref())
    .bind(req.name.unwrap_or_default())
    .bind(req.description)
    .bind(Value::Array(steps).to_string())
    .bind(req.enabled)
    .bind(req.interval_seconds)
    .bind(next_run_at)
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.pool)
    .await?;

    let row = accessible_workflow(&state, scope.as_deref(), id).await?;
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
    let row = accessible_workflow(&state, scope.as_deref(), workflow_id).await?;
    Ok(Json(workflow_out(&row)).into_response())
}

async fn update_workflow(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(workflow_id): PathId<i64>,
    body: Option<Json<Value>>,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    accessible_workflow(&state, scope.as_deref(), workflow_id).await?;

    let patch: Map<String, Value> = match body {
        Some(Json(Value::Object(map))) => map,
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

    let row = accessible_workflow(&state, scope.as_deref(), workflow_id).await?;
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
    T: for<'q> sqlx::Encode<'q, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite> + Send,
{
    let sql = format!("UPDATE workflows SET {column} = ? WHERE id = ?");
    sqlx::query(&sql).bind(value).bind(workflow_id).execute(&state.pool).await?;
    Ok(())
}

async fn delete_workflow(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(workflow_id): PathId<i64>,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    accessible_workflow(&state, scope.as_deref(), workflow_id).await?;

    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM workflow_runs WHERE workflow_id = ?")
        .bind(workflow_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM workflows WHERE id = ?")
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
    accessible_workflow(&state, scope.as_deref(), workflow_id).await?;

    let rows: Vec<RunRow> = sqlx::query_as(&format!(
        "SELECT {RUN_COLUMNS} FROM workflow_runs WHERE workflow_id = ? ORDER BY id DESC LIMIT ?"
    ))
    .bind(workflow_id)
    .bind(q.limit)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(json!({ "runs": rows.iter().map(run_out).collect::<Vec<_>>() })).into_response())
}

async fn get_run(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId((workflow_id, run_id)): PathId<(i64, i64)>,
) -> Result<Response, ApiError> {
    let scope = client_scope(&principal, &headers);
    accessible_workflow(&state, scope.as_deref(), workflow_id).await?;

    let row: Option<RunRow> =
        sqlx::query_as(&format!("SELECT {RUN_COLUMNS} FROM workflow_runs WHERE id = ?"))
            .bind(run_id)
            .fetch_optional(&state.pool)
            .await?;
    match row {
        Some(row) if row.workflow_id == workflow_id => Ok(Json(run_out(&row)).into_response()),
        _ => Err(ApiError::not_found("Run not found")),
    }
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
}
