//! Processes: the whole orchestrator HTTP surface, ported from
//! `app/process_routes.py` (plus `projects_routes.py::list_project_processes`).
//!
//! Sub-steps 1–3 of plan.md's `processes / orchestrator — scope (step 3)` were
//! the routes that schedule nothing; sub-step 6 added the six that do. Those six
//! are **status writes plus a `tokio::spawn`**, which is exactly what
//! `BackgroundTasks` is on the Python side: the response goes out before the
//! work runs, so every handler here commits its own rows and *then* calls into
//! [`crate::executor`]. Nothing in this file awaits a planner or a DAG.
//!
//! `process`, `tasknode` and `eventlog` are all written from here. The `process`
//! statements assign only the attributes SQLAlchemy would have flushed — `status`
//! and `failure_reason` — so they still cannot revert the three writers plan.md
//! lists, each of which owns a different column.
//!
//! Python's oddities are ported deliberately, not fixed:
//!
//! - `assert_process_client_access` answers **404** for a mismatched
//!   `X-Agent-Platform-Client`, never 403 — the namespace hides rows rather than
//!   refusing them.
//! - `GET /projects/{id}/processes` checks project access and **not**
//!   `process:read`, unlike every other route in this file.
//! - `/sync` treats a whitespace-only `dag_json` as missing, `/retry` does not —
//!   one tests `.strip()`, the other only truthiness, so the same row 400s with
//!   two different messages depending on which route you call.
//!
//! ponytail: a handler's writes are separate autocommitted statements where
//! Python has one `session.commit()`. Nothing is scheduled until they have all
//! landed, so the executor never sees a half-written row; a concurrent *reader*
//! could. Wrap them in a `sqlx` transaction if that ever shows up.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::FromRow;

use crate::auth::Principal;
use crate::error::{ApiError, PathId};
use crate::teams::{parse_roster, resolved_team_color, with_default_accents, TeamRoster};
use crate::wire::{iso_from_sql, parse_body_or_default, sql_flag, sql_now, sql_time, sql_time_opt};
use crate::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/processes", get(list_processes).post(start_process))
        .route("/api/v1/processes/{process_id}", get(get_process).patch(patch_process))
        .route("/api/v1/processes/{process_id}/events", get(list_events))
        .route("/api/v1/processes/{process_id}/approve", post(approve_dag))
        .route("/api/v1/processes/{process_id}/cancel", post(cancel_process))
        .route("/api/v1/processes/{process_id}/sync", post(sync_process))
        .route("/api/v1/processes/{process_id}/retry", post(retry_process))
        .route(
            "/api/v1/processes/{process_id}/tasks/{task_id}/review",
            post(review_task),
        )
        .route(
            "/api/v1/processes/{process_id}/tasks/{task_id}/retry",
            post(retry_failed_task),
        )
        .route("/api/v1/processes/{process_id}/stream", get(stream_events))
        // Lives with the domain it reads, not with `projects.rs`.
        .route(
            "/api/v1/projects/{project_id}/processes",
            get(list_project_processes),
        )
}

const CLIENT_HEADER: &str = "x-agent-platform-client";

/// Terminal phases: nothing further will be appended to the log.
const TERMINAL: [&str; 3] = ["completed", "failed", "cancelled"];
/// Phases that block on a human. The stream stops; correctness moves to polling.
const HUMAN_GATE: [&str; 2] = ["approval_required", "task_review_required"];
const CANCELLABLE: [&str; 6] = [
    "pending",
    "planning",
    "approval_required",
    "approved",
    "running",
    "task_review_required",
];

// ---------------------------------------------------------------------------
// Wire shapes
// ---------------------------------------------------------------------------

/// Field order is `models.py`'s declaration order, which is what pydantic
/// serializes a `Process` in.
#[derive(Debug, FromRow, Serialize)]
struct ProcessOut {
    id: i64,
    goal: String,
    status: String,
    dag_json: Option<String>,
    failure_reason: Option<String>,
    total_tokens: i64,
    total_cost: f64,
    tool_invocations_used: i64,
    team_template_id: Option<i64>,
    team_snapshot_json: Option<String>,
    project_id: Option<i64>,
    client_id: Option<String>,
    token_id: Option<i64>,
    model_build_job_id: Option<i64>,
    /// `INTEGER` 0/1 on both backends, for the same reason `requires_review` is.
    #[serde(serialize_with = "sql_flag")]
    auto_approve: i64,
    #[serde(serialize_with = "sql_time")]
    created_at: String,
    #[serde(serialize_with = "sql_time")]
    updated_at: String,
}

pub const PROCESS_COLUMNS: &str = "CAST(id AS BIGINT) AS id, goal, status, dag_json, \
     failure_reason, CAST(total_tokens AS BIGINT) AS total_tokens, total_cost, \
     CAST(tool_invocations_used AS BIGINT) AS tool_invocations_used, \
     CAST(team_template_id AS BIGINT) AS team_template_id, team_snapshot_json, \
     CAST(project_id AS BIGINT) AS project_id, client_id, \
     CAST(token_id AS BIGINT) AS token_id, \
     CAST(model_build_job_id AS BIGINT) AS model_build_job_id, auto_approve, \
     CAST(created_at AS TEXT) AS created_at, CAST(updated_at AS TEXT) AS updated_at";

#[derive(Debug, FromRow, Serialize)]
struct TaskNodeOut {
    id: i64,
    process_id: i64,
    client_uuid: String,
    parent_client_uuid: Option<String>,
    role: String,
    system_prompt: String,
    instructions: String,
    llm_model: Option<String>,
    /// `text` | `image` | `video`, and the media job the last two started
    /// (ADR 0018). `media_job_id` is here so a reader can fetch the picture
    /// without parsing it back out of `output`.
    modality: String,
    media_job_id: Option<i64>,
    dependencies_json: String,
    status: String,
    /// `INTEGER` on both backends, so `i64` here — the `Any` driver will not
    /// hand a `BIGINT` to a Rust `bool`, and this 500'd every task-detail read
    /// once `processes` moved onto that pool. The wire keeps its boolean.
    #[serde(serialize_with = "sql_flag")]
    requires_review: i64,
    reviewer_client_uuid: Option<String>,
    review_feedback: Option<String>,
    revision_count: i64,
    draft_output: Option<String>,
    output: Option<String>,
    failure_debug_json: Option<String>,
    tokens_used: i64,
    #[serde(serialize_with = "sql_time_opt")]
    started_at: Option<String>,
    #[serde(serialize_with = "sql_time_opt")]
    completed_at: Option<String>,
}

pub const TASK_COLUMNS: &str = "CAST(id AS BIGINT) AS id, \
     CAST(process_id AS BIGINT) AS process_id, client_uuid, parent_client_uuid, role, \
     system_prompt, instructions, llm_model, modality, \
     CAST(media_job_id AS BIGINT) AS media_job_id, dependencies_json, status, requires_review, \
     reviewer_client_uuid, review_feedback, \
     CAST(revision_count AS BIGINT) AS revision_count, draft_output, output, \
     failure_debug_json, CAST(tokens_used AS BIGINT) AS tokens_used, \
     CAST(started_at AS TEXT) AS started_at, CAST(completed_at AS TEXT) AS completed_at";

#[derive(Debug, FromRow, Serialize)]
struct EventOut {
    id: i64,
    process_id: i64,
    task_id: Option<i64>,
    event_type: String,
    content: String,
    #[serde(serialize_with = "sql_time")]
    created_at: String,
}

pub const EVENT_COLUMNS: &str = "CAST(id AS BIGINT) AS id, \n     CAST(process_id AS BIGINT) AS process_id, CAST(task_id AS BIGINT) AS task_id, event_type, \n     content, CAST(created_at AS TEXT) AS created_at";

// ---------------------------------------------------------------------------
// Access
// ---------------------------------------------------------------------------

async fn load_process(state: &AppState, process_id: i64) -> Result<ProcessOut, ApiError> {
    sqlx::query_as(&crate::db::sql(&format!("SELECT {PROCESS_COLUMNS} FROM process WHERE id = ?"), state.backend))
        .bind(process_id)
        .fetch_optional(&state.any)
        .await?
        .ok_or_else(|| ApiError::not_found("Process not found"))
}

/// `client_scope.assert_process_client_access`.
///
/// A process with no `client_id` is visible to everyone; one with a `client_id`
/// is visible only to a caller sending exactly that header. The mismatch is a
/// **404, not a 403** — the namespace is meant to hide rows, and telling a caller
/// that a row exists but belongs to someone else would defeat that.
fn assert_client_access(proc: &ProcessOut, headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(scope) = proc.client_id.as_deref() else {
        return Ok(());
    };
    // The check trims again, which is a no-op after `client_header`. Missing or
    // blank reads as the empty string.
    if client_header(headers).unwrap_or_default() != scope {
        return Err(ApiError::not_found("Process not found"));
    }
    Ok(())
}

/// `api_auth.agent_platform_client_header`: trimmed, blank is absent, 256 chars.
pub(crate) fn client_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get(CLIENT_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.chars().take(256).collect())
}

/// `assert_token_project_access(principal, proc.project_id)` — the sessionless
/// call the per-process routes make. An unassigned process is master-key-only.
async fn assert_project_access(
    state: &AppState,
    principal: &Principal,
    project_id: Option<i64>,
) -> Result<(), ApiError> {
    match project_id {
        None if principal.workspace_id.is_some() => Err(ApiError::not_found("Not found")),
        None => Ok(()),
        Some(project_id) => crate::projects::assert_access(state, principal, project_id).await,
    }
}

/// The three checks every per-process route runs, in Python's order.
async fn accessible_process(
    state: &AppState,
    principal: &Principal,
    headers: &HeaderMap,
    process_id: i64,
) -> Result<ProcessOut, ApiError> {
    let proc = load_process(state, process_id).await?;
    assert_client_access(&proc, headers)?;
    assert_project_access(state, principal, proc.project_id).await?;
    Ok(proc)
}

// ---------------------------------------------------------------------------
// GET /processes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default = "default_process_limit")]
    limit: i64,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    project_id: Option<i64>,
    #[serde(default)]
    unassigned_only: bool,
}

fn default_process_limit() -> i64 {
    50
}

#[derive(Debug, PartialEq)]
struct ListFilters {
    client_id: Option<String>,
    project_id: Option<i64>,
    unassigned_only: bool,
}

/// The route's two 400s and the normalisation between them.
///
/// Split out from the handler because the workspace branch has a database call
/// in the middle of it: Python checks for `project_id`, *then* resolves project
/// access, *then* applies the general filter rule. Resolving access after both
/// checks is equivalent — when the caller is workspace-scoped the general rule
/// is already satisfied by the `project_id` the first check demanded.
fn list_filters(workspace_scoped: bool, q: &ListQuery) -> Result<ListFilters, ApiError> {
    let mut unassigned_only = q.unassigned_only;
    if workspace_scoped {
        // A workspace token must name a project inside its own workspace, and may
        // never ask for the workspace-less rows.
        if q.project_id.is_none() {
            return Err(ApiError::bad_request(
                "project_id is required for a workspace-scoped token.",
            ));
        }
        unassigned_only = false;
    }

    // Python's truthiness: `?client_id=` is falsy and does not count as a filter.
    let named_client = q.client_id.as_deref().is_some_and(|c| !c.is_empty());
    if !(named_client || q.project_id.is_some() || unassigned_only) {
        return Err(ApiError::bad_request(
            "Must specify one of: project_id, client_id, or unassigned_only=true",
        ));
    }

    Ok(ListFilters {
        // `?client_id=%20` passes the check above and then filters on nothing:
        // the guard tests truthiness, the WHERE clause tests the trimmed value.
        client_id: q
            .client_id
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(|c| c.chars().take(256).collect()),
        project_id: q.project_id,
        unassigned_only,
    })
}

async fn list_processes(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    principal.require_scope("process:read")?;
    let filters = list_filters(principal.workspace_id.is_some(), &q)?;
    if let Some(project_id) = filters.project_id {
        crate::projects::assert_access(&state, &principal, project_id).await?;
    }

    let mut sql = format!("SELECT {PROCESS_COLUMNS} FROM process");
    let mut wheres: Vec<&str> = Vec::new();
    if filters.client_id.is_some() {
        wheres.push("client_id = ?");
    }
    if filters.unassigned_only {
        wheres.push("project_id IS NULL");
    } else if filters.project_id.is_some() {
        wheres.push("project_id = ?");
    }
    if !wheres.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&wheres.join(" AND "));
    }
    sql.push_str(" ORDER BY id DESC LIMIT ?");

    let sql = crate::db::sql(&sql, state.backend).into_owned();
    let mut query = sqlx::query_as::<_, ProcessOut>(&sql);
    if let Some(client_id) = &filters.client_id {
        query = query.bind(client_id.as_str());
    }
    if !filters.unassigned_only {
        if let Some(project_id) = filters.project_id {
            query = query.bind(project_id);
        }
    }
    // A negative `limit` reaches SQLite as `LIMIT -n`, which means no limit —
    // exactly what `min(limit, 200)` lets through on the Python side.
    let rows = query.bind(q.limit.min(200)).fetch_all(&state.any).await?;

    Ok(Json(json!({ "processes": rows })).into_response())
}

// ---------------------------------------------------------------------------
// GET /projects/{id}/processes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct LimitQuery {
    #[serde(default = "default_process_limit")]
    limit: i64,
}

/// The odd one out: project access is checked, `process:read` is **not**.
/// Adding the scope check would refuse a token the Python route serves.
///
/// `require_one(session, Project, …)` is not repeated — `assert_access` already
/// 404s a project that does not exist, and does so first.
async fn list_project_processes(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(project_id): PathId<i64>,
    Query(q): Query<LimitQuery>,
) -> Result<Response, ApiError> {
    crate::projects::assert_access(&state, &principal, project_id).await?;

    let rows: Vec<ProcessOut> = sqlx::query_as(&crate::db::sql(&format!(
        "SELECT {PROCESS_COLUMNS} FROM process WHERE project_id = ? ORDER BY id DESC LIMIT ?"
    ), state.backend))
    .bind(project_id)
    .bind(q.limit.min(200))
    .fetch_all(&state.any)
    .await?;

    Ok(Json(json!({ "processes": rows })).into_response())
}

// ---------------------------------------------------------------------------
// GET /processes/{id} and /events
// ---------------------------------------------------------------------------

async fn get_process(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(process_id): PathId<i64>,
) -> Result<Response, ApiError> {
    principal.require_scope("process:read")?;
    let proc = accessible_process(&state, &principal, &headers, process_id).await?;

    // No ORDER BY, like Python: the rows come back in rowid order, which is
    // insertion order, and the DAG's own edges are what the UI sorts on.
    let tasks: Vec<TaskNodeOut> =
        sqlx::query_as(&crate::db::sql(&format!("SELECT {TASK_COLUMNS} FROM tasknode WHERE process_id = ?"), state.backend))
            .bind(process_id)
            .fetch_all(&state.any)
            .await?;

    Ok(Json(json!({ "process": proc, "tasks": tasks })).into_response())
}

#[derive(Debug, Deserialize)]
struct EventQuery {
    #[serde(default)]
    event_type: Option<String>,
    #[serde(default = "default_event_limit")]
    limit: i64,
    #[serde(default)]
    after_id: i64,
}

fn default_event_limit() -> i64 {
    500
}

async fn list_events(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(process_id): PathId<i64>,
    Query(q): Query<EventQuery>,
) -> Result<Response, ApiError> {
    principal.require_scope("process:read")?;
    accessible_process(&state, &principal, &headers, process_id).await?;

    let event_type = q.event_type.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let mut sql = format!(
        "SELECT {EVENT_COLUMNS} FROM eventlog WHERE process_id = ? AND id > ?"
    );
    if event_type.is_some() {
        sql.push_str(" AND event_type = ?");
    }
    sql.push_str(" ORDER BY id ASC LIMIT ?");

    let sql = crate::db::sql(&sql, state.backend).into_owned();
    let mut query = sqlx::query_as::<_, EventOut>(&sql)
        .bind(process_id)
        .bind(q.after_id.max(0));
    if let Some(event_type) = event_type {
        query = query.bind(event_type.to_string());
    }
    let rows = query.bind(q.limit.max(1).min(2000)).fetch_all(&state.any).await?;

    Ok(Json(json!({ "events": rows })).into_response())
}

// ---------------------------------------------------------------------------
// POST /processes/{id}/cancel
// ---------------------------------------------------------------------------

async fn cancel_process(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(process_id): PathId<i64>,
) -> Result<Response, ApiError> {
    principal.require_scope("process:write")?;
    let proc = accessible_process(&state, &principal, &headers, process_id).await?;

    if TERMINAL.contains(&proc.status.as_str()) {
        return Ok(Json(json!({ "status": proc.status, "idempotent": true })).into_response());
    }
    if !CANCELLABLE.contains(&proc.status.as_str()) {
        return Err(ApiError::bad_request(format!(
            "Cannot cancel from status {}",
            proc.status
        )));
    }

    // One column. The other writers of this table each assign a different
    // attribute, so none of them can be reverted by this statement — and Python
    // does not touch `updated_at` here either.
    sqlx::query(&crate::db::sql("UPDATE process SET status = 'cancelled' WHERE id = ?", state.backend))
        .bind(process_id)
        .execute(&state.any)
        .await?;

    Ok(Json(json!({ "status": "cancelled" })).into_response())
}

// ---------------------------------------------------------------------------
// Shared write helpers
// ---------------------------------------------------------------------------

/// `services.process_mutation_service.append_process_event`.
///
/// Every event these routes append is a `status_change`; `trace`, `tool_call`
/// and `error` rows come from the executor.
async fn append_event(
    state: &AppState,
    process_id: i64,
    task_id: Option<i64>,
    content: &str,
) -> Result<(), ApiError> {
    sqlx::query(&crate::db::sql(
        "INSERT INTO eventlog (process_id, task_id, event_type, content, created_at) \
         VALUES (?, ?, 'status_change', ?, ?)", state.backend)
    )
    .bind(process_id)
    .bind(task_id)
    .bind(content)
    .bind(sql_now())
    .execute(&state.any)
    .await?;
    Ok(())
}

/// `services.process_sync_service.task_status_counts`.
///
/// Python inserts in first-seen order and `json.dumps` keeps it; `serde_json::Map`
/// is a `BTreeMap` here, so this one sorts. Both cross-render comparisons parse
/// the body, so the order is not observable.
fn task_status_counts(statuses: &[String]) -> Map<String, Value> {
    let mut counts = Map::new();
    for status in statuses {
        let seen = counts.get(status.as_str()).and_then(Value::as_i64).unwrap_or(0);
        counts.insert(status.clone(), Value::from(seen + 1));
    }
    counts
}

/// The columns the tasknode routes read. `TaskNodeOut` is the wire shape and
/// carries far more than any of them needs.
#[derive(Debug, FromRow)]
struct TaskRow {
    process_id: i64,
    client_uuid: String,
    status: String,
    revision_count: i64,
}

async fn load_task(state: &AppState, process_id: i64, task_id: i64) -> Result<TaskRow, ApiError> {
    let task: TaskRow = sqlx::query_as(&crate::db::sql(
        "SELECT process_id, client_uuid, status, revision_count FROM tasknode WHERE id = ?", state.backend)
    )
    .bind(task_id)
    .fetch_optional(&state.any)
    .await?
    .ok_or_else(|| ApiError::not_found("Task not found"))?;
    // A task of another process is hidden, not misattributed.
    if task.process_id != process_id {
        return Err(ApiError::not_found("Task not found"));
    }
    Ok(task)
}

/// `process.status` + `process.failure_reason` are the only attributes these
/// routes dirty, so the UPDATE names exactly what SQLAlchemy would have flushed
/// and still cannot revert the other writers of this table.
async fn set_process_status(
    state: &AppState,
    process_id: i64,
    status: &str,
    failure_reason: Option<&str>,
) -> Result<(), ApiError> {
    sqlx::query(&crate::db::sql("UPDATE process SET status = ?, failure_reason = ? WHERE id = ?", state.backend))
        .bind(status)
        .bind(failure_reason)
        .bind(process_id)
        .execute(&state.any)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Planner team context (`team_schema.py`, the process-facing half)
// ---------------------------------------------------------------------------

/// `team_schema.role_depth` — ancestor edges to a root, stopping on a cycle.
fn role_depth(role_id: &str, parent_by_id: &HashMap<&str, Option<&str>>) -> usize {
    let mut depth = 0;
    let mut seen: HashSet<&str> = HashSet::new();
    let mut cur = Some(role_id);
    while let Some(id) = cur {
        if !seen.insert(id) {
            return depth;
        }
        // A missing key and a null parent both end the walk, exactly as Python's
        // `parent_by_id.get(cur)` does.
        match parent_by_id.get(id).copied().flatten() {
            None => break,
            Some(parent) => {
                depth += 1;
                cur = Some(parent);
            }
        }
    }
    depth
}

/// `team_schema.render_team_context_for_planner`.
///
/// This text is a prompt fragment the planner reads, so the two-space indent per
/// level, the `(id=…)` suffix and the depth-then-lowercase-name ordering are all
/// contract.
fn render_team_context_for_planner(
    name: &str,
    description: Option<&str>,
    color: Option<&str>,
    roster: &TeamRoster,
) -> String {
    let mut lines = vec![format!("Team template: {name}")];
    if let Some(description) = description.map(str::trim).filter(|d| !d.is_empty()) {
        lines.push(format!("Team description: {description}"));
    }
    if let Some(color) = color.map(str::trim).filter(|c| !c.is_empty()) {
        lines.push(format!("Team color (UI hint): {color}"));
    }
    lines.push(
        "Preferred team roster (map subagent `role` names and responsibilities to these \
         where sensible):"
            .to_string(),
    );

    // A duplicate id keeps the last parent, like Python's dict comprehension.
    let parent_by_id: HashMap<&str, Option<&str>> = roster
        .roles
        .iter()
        .map(|role| (role.id.as_str(), role.parent_id.as_deref()))
        .collect();
    let mut ordered: Vec<_> = roster.roles.iter().collect();
    // `sorted` and `sort_by_key` are both stable, so ties keep roster order.
    ordered.sort_by_key(|role| (role_depth(&role.id, &parent_by_id), role.name.to_lowercase()));

    for role in ordered {
        let indent = "  ".repeat(role_depth(&role.id, &parent_by_id));
        let mut line = format!("{indent}- {} (id={})", role.name, role.id);
        if !role.description.trim().is_empty() {
            line.push_str(&format!(": {}", role.description.trim()));
        }
        if role.modality != "text" {
            line.push_str(&format!(" [modality: {}]", role.modality));
        }
        lines.push(line);
    }
    lines.join("\n")
}

/// `team_schema.team_context_from_snapshot_json` — the planner context for a
/// process that has a snapshot but no template in hand (`sync` and `retry`).
/// Every failure is `None`, never an error: a process with an unreadable
/// snapshot still re-plans, just without the roster hint.
fn team_context_from_snapshot(snapshot_json: Option<&str>) -> Option<String> {
    let raw = snapshot_json.map(str::trim).filter(|s| !s.is_empty())?;
    let data = serde_json::from_str::<Value>(raw).ok()?;
    let data = data.as_object()?;
    let name = match data.get("name").and_then(Value::as_str).unwrap_or("").trim() {
        "" => "Team",
        name => name,
    };
    let roster = data.get("roster").filter(|roster| roster.is_object())?;
    // ponytail: Python parses this through pydantic, so it also returns `None`
    // for a roster with a bad parent graph. This only deserializes — the same
    // gap `teams::parse_roster` already carries on the read path.
    let roster: TeamRoster = serde_json::from_value(roster.clone()).ok()?;
    Some(render_team_context_for_planner(
        name,
        data.get("description").and_then(Value::as_str),
        data.get("color").and_then(Value::as_str),
        &roster,
    ))
}

// ---------------------------------------------------------------------------
// POST /processes
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct StartProcessRequest {
    #[serde(default)]
    goal: Option<String>,
    #[serde(default)]
    auto_approve: bool,
    #[serde(default)]
    team_template_id: Option<i64>,
    #[serde(default)]
    project_id: Option<i64>,
    #[serde(default)]
    client_id: Option<String>,
}

#[derive(Debug, FromRow)]
struct TemplateRow {
    id: i64,
    workspace_id: Option<i64>,
    name: String,
    description: Option<String>,
    color: Option<String>,
    roster_json: String,
}

/// `client_scope.require_client_id_enabled`.
pub(crate) fn require_client_id_enabled() -> bool {
    matches!(
        std::env::var("AGENT_PLATFORM_REQUIRE_CLIENT_ID")
            .unwrap_or_default()
            .trim()
            .to_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// `client_scope.merged_client_id` — the first non-blank of header, then body.
pub(crate) fn merged_client_id(header: Option<&str>, body: Option<&str>) -> Option<String> {
    header
        .into_iter()
        .chain(body)
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(|value| value.chars().take(256).collect())
}

/// Create a process and hand the planner to the executor.
///
/// The row is committed at `pending` and the response goes out before
/// `spawn_plan` does anything, which is what `BackgroundTasks` gives Python. The
/// `auto_approve` flag rides along rather than being resolved here — the
/// executor ORs it with `AGENT_PLATFORM_AUTO_APPROVE`, and a plan that
/// auto-approves falls straight into execution without a second request.
async fn start_process(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    // Raw bytes, not `Option<Json<StartProcessRequest>>`: axum's `Json`
    // extractor only yields `None` for a body-less request with no
    // `Content-Type` at all — an empty body sent *with* `application/json`
    // (an argument-less POST from most clients) fails to parse and axum
    // answers its own plain-text 400 before this handler runs.
    body: Bytes,
) -> Result<Response, ApiError> {
    principal.require_scope("process:write")?;
    let req: StartProcessRequest = parse_body_or_default(&body)?;

    let mut errors = Vec::new();
    if req.goal.is_none() {
        errors.push(ApiError::field_error("goal", "missing", "Field required"));
    }
    if req.team_template_id.is_none() {
        errors.push(ApiError::field_error("team_template_id", "missing", "Field required"));
    }
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }
    let goal = req.goal.unwrap_or_default();
    let team_template_id = req.team_template_id.unwrap_or_default();

    if principal.workspace_id.is_some() {
        // A workspace token must name a project inside its own workspace.
        let project_id = req.project_id.ok_or_else(|| {
            ApiError::bad_request("project_id is required for a workspace-scoped token.")
        })?;
        crate::projects::assert_access(&state, &principal, project_id).await?;
    }

    let client_id = merged_client_id(client_header(&headers).as_deref(), req.client_id.as_deref());
    if require_client_id_enabled() && client_id.is_none() {
        return Err(ApiError::bad_request(
            "client_id is required (JSON body or X-Agent-Platform-Client header)",
        ));
    }

    let template: TemplateRow = sqlx::query_as(&crate::db::sql(
        "SELECT id, workspace_id, name, description, color, roster_json \
         FROM teamtemplate WHERE id = ?", state.backend)
    )
    .bind(team_template_id)
    .fetch_optional(&state.any)
    .await?
    .ok_or_else(|| ApiError::not_found("Team template not found"))?;
    // A workspace token may plan with a global (NULL) template or its own, never
    // another tenant's — and the miss is a 404, like every other tenancy check.
    if let (Some(caller), Some(owner)) = (principal.workspace_id, template.workspace_id) {
        if caller != owner {
            return Err(ApiError::not_found("Team template not found"));
        }
    }
    if let Some(project_id) = req.project_id {
        let exists: Option<i64> = sqlx::query_scalar(&crate::db::sql("SELECT id FROM project WHERE id = ?", state.backend))
            .bind(project_id)
            .fetch_optional(&state.any)
            .await?;
        if exists.is_none() {
            return Err(ApiError::not_found("Project not found"));
        }
    }

    // Same read-path colour resolution as `todos::snapshot_template`, keyed by
    // the template id; the snapshot is stored and never recomputed.
    let key = template.id.to_string();
    let color = resolved_team_color(template.color.as_deref(), Some(&key));
    let roster = with_default_accents(&parse_roster(&template.roster_json)?, Some(&color), &key);
    let team_context = render_team_context_for_planner(
        &template.name,
        template.description.as_deref(),
        Some(&color),
        &roster,
    );
    let team_snapshot_json = crate::todos::build_process_team_snapshot(
        template.id,
        &template.name,
        template.description.as_deref(),
        &color,
        &roster,
    );

    // Column for column with SQLModel's `Process(...)` defaults; `dag_json`,
    // `failure_reason` and `model_build_job_id` are left NULL. `token_id` *is*
    // set here — this is the path plan.md flags for the token-counter hazard.
    let now = sql_now();
    let process_id: i64 = sqlx::query_scalar(&crate::db::sql(
        "INSERT INTO process \
         (goal, status, total_tokens, total_cost, tool_invocations_used, team_template_id, \
          team_snapshot_json, project_id, client_id, token_id, created_at, updated_at) \
         VALUES (?, 'pending', 0, 0.0, 0, ?, ?, ?, ?, ?, ?, ?) RETURNING CAST(id AS BIGINT)", state.backend)
    )
    .bind(&goal)
    .bind(team_template_id)
    .bind(&team_snapshot_json)
    .bind(req.project_id)
    .bind(client_id.as_deref())
    .bind(principal.token_id)
    .bind(i64::from(req.auto_approve))
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.any)
    .await?;

    crate::executor::spawn_plan(state.clone(), process_id, goal, Some(team_context));

    Ok(Json(json!({ "process_id": process_id, "status": "pending" })).into_response())
}

// ---------------------------------------------------------------------------
// PATCH /processes/{id}
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct PatchProcessRequest {
    #[serde(default)]
    auto_approve: Option<bool>,
}

/// Flip `auto_approve` on a process already in flight.
///
/// Only that one field: everything else on the row is either the planner's
/// output or a status the executor owns. Turning it on does **not** clear the
/// gate the process is sitting at right now — the executor reads the flag when
/// it next reaches one — so the caller still posts `/approve` or a task review
/// to release the current stop.
async fn patch_process(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(process_id): PathId<i64>,
    body: Bytes,
) -> Result<Response, ApiError> {
    principal.require_scope("process:write")?;
    accessible_process(&state, &principal, &headers, process_id).await?;
    let req: PatchProcessRequest = parse_body_or_default(&body)?;

    if let Some(auto_approve) = req.auto_approve {
        sqlx::query(&crate::db::sql("UPDATE process SET auto_approve = ?, updated_at = ? WHERE id = ?", state.backend))
            .bind(i64::from(auto_approve))
            .bind(sql_now())
            .bind(process_id)
            .execute(&state.any)
            .await?;
        append_event(
            &state,
            process_id,
            None,
            if auto_approve { "Auto-approve turned on" } else { "Auto-approve turned off" },
        )
        .await?;
    }

    Ok(Json(load_process(&state, process_id).await?).into_response())
}

// ---------------------------------------------------------------------------
// POST /processes/{id}/approve
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct ApproveDagRequest {
    #[serde(default)]
    dag_json: Option<String>,
}

/// `services.process_approval_service.is_idempotent_approval_status` — a second
/// POST after the commit but before the status moves is a success, not a 400.
const IDEMPOTENT_APPROVAL: [&str; 3] = ["running", "completed", "approved"];

async fn approve_dag(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(process_id): PathId<i64>,
    // Raw bytes, not `Option<Json<ApproveDagRequest>>` — see `start_process`'s
    // comment.
    body: Bytes,
) -> Result<Response, ApiError> {
    principal.require_scope("process:write")?;
    let proc = accessible_process(&state, &principal, &headers, process_id).await?;

    let req: ApproveDagRequest = parse_body_or_default(&body)?;
    let dag_json = req.dag_json.ok_or_else(|| {
        ApiError::validation(vec![ApiError::field_error("dag_json", "missing", "Field required")])
    })?;

    if IDEMPOTENT_APPROVAL.contains(&proc.status.as_str()) {
        return Ok(Json(json!({
            "status": proc.status,
            "idempotent": true,
            "message": "DAG already approved or process already finished",
        }))
        .into_response());
    }
    if proc.status != "approval_required" {
        return Err(ApiError::bad_request(format!(
            "Process is not awaiting approval (status={})",
            proc.status
        )));
    }

    let raw: Value = serde_json::from_str(&dag_json)
        .map_err(|e| ApiError::bad_request(format!("Invalid JSON for approved DAG: {e}")))?;
    let validated = crate::dag_schema::validate_planner_dag(&raw).map_err(ApiError::bad_request)?;
    // Replaces the task rows and stores the canonical DAG, both before the
    // status flips — an executor that starts early must never find `approved`
    // next to the old tasks.
    crate::executor::apply_validated_planner_to_process(&state, process_id, &validated).await?;
    set_process_status(&state, process_id, "approved", None).await?;

    crate::executor::spawn_execute_dag(state.clone(), process_id);
    Ok(Json(json!({ "status": "approved", "message": "Execution scheduled" })).into_response())
}

// ---------------------------------------------------------------------------
// POST /processes/{id}/tasks/{task_id}/review
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct ReviewTaskRequest {
    #[serde(default)]
    decision: Option<String>,
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    feedback: Option<String>,
    #[serde(default)]
    instructions: Option<String>,
}

/// `shared_enums.ReviewDecision`.
const REVIEW_DECISIONS: [&str; 3] = ["approve", "reject", "request_changes"];

/// `json.dumps` of `reject_task_and_fail_process`'s debug payload — default
/// separators, so the spaces are part of the stored value.
const REVIEW_REJECT_DEBUG_JSON: &str = "{\"source\": \"review_reject\", \"message\": \
     \"Human reviewer rejected this task at the review gate.\"}";

/// The human gate: approve, reject, or send a task back for revision.
///
/// Approve and request_changes both schedule; **reject does not** — it fails the
/// process, and there is nothing left to run.
async fn review_task(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId((process_id, task_id)): PathId<(i64, i64)>,
    // Raw bytes, not `Option<Json<ReviewTaskRequest>>` — see `start_process`'s
    // comment.
    body: Bytes,
) -> Result<Response, ApiError> {
    principal.require_scope("process:write")?;
    let proc = accessible_process(&state, &principal, &headers, process_id).await?;

    let req: ReviewTaskRequest = parse_body_or_default(&body)?;
    let decision = match req.decision.as_deref() {
        None => {
            return Err(ApiError::validation(vec![ApiError::field_error(
                "decision",
                "missing",
                "Field required",
            )]))
        }
        Some(decision) if !REVIEW_DECISIONS.contains(&decision) => {
            return Err(ApiError::validation(vec![ApiError::field_error(
                "decision",
                "enum",
                "Input should be 'approve', 'reject' or 'request_changes'",
            )]))
        }
        Some(decision) => decision,
    };

    let task = load_task(&state, process_id, task_id).await?;

    // Re-approving a task that already went through the gate is a success, so a
    // retried request does not 400 after the first one won.
    if task.status == "completed" && decision == "approve" {
        return Ok(Json(json!({ "status": "completed", "idempotent": true })).into_response());
    }
    if task.status != "awaiting_review" {
        return Err(ApiError::bad_request(format!(
            "Task is not awaiting review (status={})",
            task.status
        )));
    }
    if TERMINAL.contains(&proc.status.as_str()) {
        return Err(ApiError::bad_request(format!(
            "Process has finished (status={}); cannot review tasks",
            proc.status
        )));
    }

    if decision == "request_changes" {
        let feedback = req.feedback.as_deref().unwrap_or("").trim().to_string();
        if feedback.is_empty() {
            return Err(ApiError::bad_request("feedback is required for request_changes"));
        }
        let revision_count = task.revision_count + 1;
        // `draft_output = output` reads the pre-update row, which is what
        // `task.draft_output = task.output` does before the flush.
        sqlx::query(&crate::db::sql(
            "UPDATE tasknode SET draft_output = output, output = NULL, review_feedback = ?, \
             reviewer_client_uuid = NULL, revision_count = revision_count + 1, \
             status = 'pending', failure_debug_json = NULL, started_at = NULL, \
             completed_at = NULL, tokens_used = 0 WHERE id = ?", state.backend)
        )
        .bind(&feedback)
        .bind(task_id)
        .execute(&state.any)
        .await?;
        // Only when the reviewer sent one: `instructions` is not nullable, and
        // omitting it must leave the planner's text alone.
        if let Some(instructions) = req.instructions.as_deref() {
            sqlx::query(&crate::db::sql("UPDATE tasknode SET instructions = ? WHERE id = ?", state.backend))
                .bind(instructions)
                .bind(task_id)
                .execute(&state.any)
                .await?;
        }
        set_process_status(&state, process_id, "running", None).await?;
        append_event(
            &state,
            process_id,
            Some(task_id),
            &format!(
                "Task {} requeued for revision (revision {revision_count})",
                task.client_uuid
            ),
        )
        .await?;

        crate::executor::spawn_execute_dag(state.clone(), process_id);
        return Ok(
            Json(json!({ "status": "requeued", "revision_count": revision_count })).into_response()
        );
    }

    if decision == "reject" {
        sqlx::query(&crate::db::sql(
            "UPDATE tasknode SET reviewer_client_uuid = NULL, status = 'failed', \
             failure_debug_json = ? WHERE id = ?", state.backend)
        )
        .bind(REVIEW_REJECT_DEBUG_JSON)
        .bind(task_id)
        .execute(&state.any)
        .await?;
        let reason = format!("Task {} rejected at review", task.client_uuid);
        set_process_status(&state, process_id, "failed", Some(&reason)).await?;
        append_event(&state, process_id, Some(task_id), &reason).await?;
        // Nothing is scheduled — the process is terminal.
        return Ok(Json(json!({ "status": "rejected" })).into_response());
    }

    // Approve. An omitted `output` keeps whatever the agent produced.
    if let Some(output) = req.output.as_deref() {
        sqlx::query(&crate::db::sql("UPDATE tasknode SET output = ? WHERE id = ?", state.backend))
            .bind(output)
            .bind(task_id)
            .execute(&state.any)
            .await?;
    }
    sqlx::query(&crate::db::sql(
        "UPDATE tasknode SET reviewer_client_uuid = NULL, status = 'completed', \
         completed_at = ?, draft_output = NULL, review_feedback = NULL WHERE id = ?", state.backend)
    )
    .bind(sql_now())
    .bind(task_id)
    .execute(&state.any)
    .await?;
    set_process_status(&state, process_id, "running", None).await?;
    append_event(
        &state,
        process_id,
        Some(task_id),
        &format!("Task {} approved", task.client_uuid),
    )
    .await?;

    // Not `execute_dag`: an approved task may still expand into a sub-DAG, and
    // that merge has to happen before the next wave is computed.
    crate::executor::spawn_expand_after_review(state.clone(), process_id, task_id);
    Ok(Json(json!({ "status": "approved", "message": "Execution scheduled" })).into_response())
}

// ---------------------------------------------------------------------------
// POST /processes/{id}/sync
// ---------------------------------------------------------------------------

/// Everything `sync` branches on. Split out from the handler so the decision
/// table can be tested without a database — the ten HTTP tests in this domain
/// assert on a mocked executor and prove nothing about it.
#[derive(Debug, PartialEq)]
struct SyncState {
    status: String,
    awaiting_review: i64,
    /// `dag_json` present **and non-blank**. `retry` tests only truthiness on the
    /// same column, which is why a whitespace-only DAG behaves differently there.
    has_dag: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum SyncBranch {
    /// Terminal: nothing to recover.
    Terminal,
    /// Blocked on the DAG approval gate.
    ApprovalGate,
    /// Blocked on the task review gate.
    ReviewGate,
    /// `running` with tasks awaiting review — the executor should have moved the
    /// process itself and did not.
    AlignStatus,
    RequeuePlan,
    RequeueApprovedExecution,
    /// `approved` with no DAG to run: the one branch that raises.
    ApprovedWithoutDag,
    ResetRunningAndRequeue,
    Unexpected,
}

fn sync_branch(sync: &SyncState) -> SyncBranch {
    let status = sync.status.as_str();
    if TERMINAL.contains(&status) {
        return SyncBranch::Terminal;
    }
    match status {
        "approval_required" => SyncBranch::ApprovalGate,
        "task_review_required" => SyncBranch::ReviewGate,
        // Order matters: this is checked before the plain `running` branch, so a
        // process with an open review gate is aligned, never reset.
        "running" if sync.awaiting_review > 0 => SyncBranch::AlignStatus,
        "pending" | "planning" => SyncBranch::RequeuePlan,
        "approved" if sync.has_dag => SyncBranch::RequeueApprovedExecution,
        "approved" => SyncBranch::ApprovedWithoutDag,
        "running" => SyncBranch::ResetRunningAndRequeue,
        _ => SyncBranch::Unexpected,
    }
}

fn sync_terminal_detail(status: &str) -> &'static str {
    if status == "failed" {
        "Process failed; use POST /retry to re-plan or re-run execution, \
         or retry individual failed tasks — sync does not apply."
    } else {
        "Process is already finished; sync does nothing."
    }
}

fn sync_review_gate_detail(awaiting_review: i64) -> String {
    format!(
        "Waiting for human task review ({awaiting_review} task(s) in awaiting_review). \
         Use the review actions on each task."
    )
}

fn sync_response(
    process_id: i64,
    status: &str,
    action: &str,
    detail: impl Into<String>,
    counts: Map<String, Value>,
    reset_running_tasks: Option<i64>,
) -> Response {
    let mut body = json!({
        "process_id": process_id,
        "process_status": status,
        "action": action,
        "detail": detail.into(),
        "task_counts": counts,
    });
    if let Some(reset) = reset_running_tasks {
        body["reset_running_tasks"] = Value::from(reset);
    }
    Json(body).into_response()
}

/// Recover a stuck process, or explain what is blocking it.
///
/// Eight outcomes, four of which write. This is the decision table
/// `startup_recovery` applies once at boot, so the two are read together.
async fn sync_process(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(process_id): PathId<i64>,
) -> Result<Response, ApiError> {
    principal.require_scope("process:write")?;
    let proc = accessible_process(&state, &principal, &headers, process_id).await?;

    let statuses: Vec<String> =
        sqlx::query_scalar(&crate::db::sql("SELECT status FROM tasknode WHERE process_id = ?", state.backend))
            .bind(process_id)
            .fetch_all(&state.any)
            .await?;
    let counts = task_status_counts(&statuses);
    let sync = SyncState {
        status: proc.status.clone(),
        awaiting_review: statuses.iter().filter(|s| *s == "awaiting_review").count() as i64,
        has_dag: proc.dag_json.as_deref().map(str::trim).is_some_and(|dag| !dag.is_empty()),
    };

    match sync_branch(&sync) {
        SyncBranch::Terminal => Ok(sync_response(
            process_id,
            &proc.status,
            "none",
            sync_terminal_detail(&proc.status),
            counts,
            None,
        )),
        SyncBranch::ApprovalGate => Ok(sync_response(
            process_id,
            &proc.status,
            "blocked",
            "Waiting for human approval of the planner DAG. \
             Approve in the UI or cancel the process.",
            counts,
            None,
        )),
        SyncBranch::ReviewGate => Ok(sync_response(
            process_id,
            &proc.status,
            "blocked",
            sync_review_gate_detail(sync.awaiting_review),
            counts,
            None,
        )),
        SyncBranch::AlignStatus => {
            set_process_status(&state, process_id, "task_review_required", None).await?;
            append_event(
                &state,
                process_id,
                None,
                "Sync: aligned process status to task_review_required (review gate open)",
            )
            .await?;
            // Python reads `proc.status` *after* the mutation, so the response
            // reports the new phase rather than the one it was called on.
            Ok(sync_response(
                process_id,
                "task_review_required",
                "aligned_status",
                "Process status was running while tasks awaited review; \
                 updated to task_review_required.",
                counts,
                None,
            ))
        }
        SyncBranch::RequeuePlan => {
            // No status write in this branch — only the event. The row is already
            // `pending` or `planning`, which is where the planner expects it.
            append_event(&state, process_id, None, "Sync: re-scheduled planning").await?;
            crate::executor::spawn_plan(
                state.clone(),
                process_id,
                proc.goal.clone(),
                team_context_from_snapshot(proc.team_snapshot_json.as_deref()),
            );
            Ok(sync_response(
                process_id,
                &proc.status,
                "requeued_plan",
                "Planning was scheduled again. If planning was already active, \
                 you may see duplicate work until one completes.",
                counts,
                None,
            ))
        }
        SyncBranch::ApprovedWithoutDag => Err(ApiError::bad_request(
            "Process is approved but has no DAG JSON; cannot re-queue execution.",
        )),
        SyncBranch::RequeueApprovedExecution => {
            append_event(&state, process_id, None, "Sync: re-scheduled DAG execution").await?;
            crate::executor::spawn_execute_dag(state.clone(), process_id);
            Ok(sync_response(
                process_id,
                &proc.status,
                "requeued_execution",
                "DAG execution was scheduled again.",
                counts,
                None,
            ))
        }
        SyncBranch::ResetRunningAndRequeue => {
            // One statement over the matched set is what N per-task flushes come
            // to. Any in-flight work the server no longer tracks is abandoned —
            // that is the documented cost of this branch.
            let reset = sqlx::query(&crate::db::sql(
                "UPDATE tasknode SET status = 'pending', output = NULL, draft_output = NULL, \
                 review_feedback = NULL, reviewer_client_uuid = NULL, failure_debug_json = NULL, \
                 started_at = NULL, completed_at = NULL, tokens_used = 0 \
                 WHERE process_id = ? AND status = 'running'", state.backend)
            )
            .bind(process_id)
            .execute(&state.any)
            .await?
            .rows_affected() as i64;
            // `revision_count` is deliberately not reset here, unlike task retry.
            sqlx::query(&crate::db::sql("UPDATE process SET failure_reason = NULL WHERE id = ?", state.backend))
                .bind(process_id)
                .execute(&state.any)
                .await?;
            append_event(
                &state,
                process_id,
                None,
                &format!(
                    "Sync: reset {reset} stuck running task(s) to pending; \
                     re-scheduled DAG execution"
                ),
            )
            .await?;

            crate::executor::spawn_execute_dag(state.clone(), process_id);

            // Counts are recomputed after the reset, so they report `pending`.
            let statuses: Vec<String> = statuses
                .into_iter()
                .map(|status| if status == "running" { "pending".to_string() } else { status })
                .collect();
            let mut detail = "DAG execution was scheduled again.".to_string();
            if reset != 0 {
                detail
                    .push_str(&format!(" Reset {reset} task(s) that were still marked running."));
            }
            Ok(sync_response(
                process_id,
                &proc.status,
                "requeued_execution",
                detail,
                task_status_counts(&statuses),
                Some(reset),
            ))
        }
        SyncBranch::Unexpected => Ok(sync_response(
            process_id,
            &proc.status,
            "none",
            // ponytail: `{status!r}`. A status is our own column and always plain
            // ASCII, so quoting it is all `repr` would have done.
            format!("Unexpected process status '{}'; no automatic recovery.", proc.status),
            counts,
            None,
        )),
    }
}

// ---------------------------------------------------------------------------
// POST /processes/{id}/retry and /tasks/{task_id}/retry
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum RetryPlan {
    NotFailed,
    /// No tasks were ever persisted, so there is nothing to re-run: plan again.
    Replan,
    /// Tasks but no DAG — the row cannot be reconstructed.
    MissingDag,
    Reexecute,
}

fn retry_plan(status: &str, has_tasks: bool, dag_json: Option<&str>) -> RetryPlan {
    if status != "failed" {
        return RetryPlan::NotFailed;
    }
    if !has_tasks {
        return RetryPlan::Replan;
    }
    // Truthiness only, unlike `sync`: a whitespace-only DAG gets past this and
    // fails as invalid JSON with a different message.
    match dag_json {
        Some(dag) if !dag.is_empty() => RetryPlan::Reexecute,
        _ => RetryPlan::MissingDag,
    }
}

async fn retry_process(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(process_id): PathId<i64>,
) -> Result<Response, ApiError> {
    principal.require_scope("process:write")?;
    let proc = accessible_process(&state, &principal, &headers, process_id).await?;

    let has_tasks: Option<i64> =
        sqlx::query_scalar(&crate::db::sql("SELECT 1 FROM tasknode WHERE process_id = ? LIMIT 1", state.backend))
            .bind(process_id)
            .fetch_optional(&state.any)
            .await?;

    match retry_plan(&proc.status, has_tasks.is_some(), proc.dag_json.as_deref()) {
        RetryPlan::NotFailed => Err(ApiError::bad_request(format!(
            "Process is not in failed state (status={})",
            proc.status
        ))),
        RetryPlan::Replan => {
            set_process_status(&state, process_id, "planning", None).await?;
            append_event(&state, process_id, None, "Retry: re-planning scheduled").await?;
            crate::executor::spawn_plan(
                state.clone(),
                process_id,
                proc.goal.clone(),
                team_context_from_snapshot(proc.team_snapshot_json.as_deref()),
            );
            Ok(Json(json!({
                "process_id": process_id,
                "status": "planning",
                "retry": "planning",
            }))
            .into_response())
        }
        RetryPlan::MissingDag => Err(ApiError::bad_request(
            "Process has tasks but no stored DAG JSON; cannot retry execution",
        )),
        RetryPlan::Reexecute => {
            let dag_json = proc.dag_json.as_deref().unwrap_or_default();
            let raw: Value = serde_json::from_str(dag_json)
                .map_err(|e| ApiError::bad_request(format!("Invalid stored DAG JSON: {e}")))?;
            let validated =
                crate::dag_schema::validate_planner_dag(&raw).map_err(ApiError::bad_request)?;
            // Re-materialises the task rows from the stored DAG: a retry starts
            // from the planner's graph, not from whatever the last run left.
            crate::executor::apply_validated_planner_to_process(&state, process_id, &validated)
                .await?;
            set_process_status(&state, process_id, "approved", None).await?;
            append_event(&state, process_id, None, "Retry: execution re-scheduled").await?;

            crate::executor::spawn_execute_dag(state.clone(), process_id);
            Ok(Json(json!({
                "process_id": process_id,
                "status": "approved",
                "retry": "execution",
            }))
            .into_response())
        }
    }
}

/// Re-queue one failed task and resume the DAG. The process must be failed too —
/// a task cannot be retried out from under a run that is still going.
async fn retry_failed_task(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId((process_id, task_id)): PathId<(i64, i64)>,
) -> Result<Response, ApiError> {
    principal.require_scope("process:write")?;
    let proc = accessible_process(&state, &principal, &headers, process_id).await?;
    let task = load_task(&state, process_id, task_id).await?;

    if proc.status != "failed" {
        return Err(ApiError::bad_request(format!(
            "Process must be failed to retry a task (status={})",
            proc.status
        )));
    }
    if task.status != "failed" {
        return Err(ApiError::bad_request(format!(
            "Task is not failed (status={})",
            task.status
        )));
    }
    // Truthiness, like `retry` and unlike `sync`.
    if proc.dag_json.as_deref().unwrap_or_default().is_empty() {
        return Err(ApiError::bad_request(
            "Process has no stored DAG JSON; use full process retry instead",
        ));
    }

    // Unlike `sync`'s reset, this one clears `revision_count` as well: the task
    // is starting over, not resuming.
    sqlx::query(&crate::db::sql(
        "UPDATE tasknode SET status = 'pending', output = NULL, draft_output = NULL, \
         review_feedback = NULL, reviewer_client_uuid = NULL, failure_debug_json = NULL, \
         revision_count = 0, started_at = NULL, completed_at = NULL, tokens_used = 0 \
         WHERE id = ?", state.backend)
    )
    .bind(task_id)
    .execute(&state.any)
    .await?;
    set_process_status(&state, process_id, "approved", None).await?;
    append_event(
        &state,
        process_id,
        Some(task_id),
        &format!(
            "Retry: task {} reset to pending; execution re-scheduled",
            task.client_uuid
        ),
    )
    .await?;

    crate::executor::spawn_execute_dag(state.clone(), process_id);
    Ok(Json(json!({
        "process_id": process_id,
        "task_id": task_id,
        "status": "approved",
        "retry": "task",
    }))
    .into_response())
}

// ---------------------------------------------------------------------------
// GET /processes/{id}/stream (SSE)
// ---------------------------------------------------------------------------

/// `sse_starlette` writes CRLF-separated frames on Windows, which is where this
/// runs; `client/src/sse.rs` normalises either separator before splitting.
const SEP: &str = "\r\n";
/// The keep-alive. `sse_starlette` appends a timestamp to the comment; nothing
/// reads it, and a `:` line is dropped by every consumer before it is parsed.
const PING_FRAME: &str = ": ping\r\n\r\n";
const POLL: Duration = Duration::from_millis(800);
const PING: Duration = Duration::from_secs(15);

/// One event-log row. **Both `task_id` and `timestamp` are always present**, even
/// when `type` is `"error"` — `sse.rs::is_sentinel` tells a log row from a
/// sentinel by exactly that, and dropping either would end the client's stream
/// on the first logged failure.
#[derive(Serialize)]
struct LogFrame<'a> {
    task_id: Option<i64>,
    #[serde(rename = "type")]
    event_type: &'a str,
    content: &'a str,
    timestamp: String,
}

/// A stream-control frame. Carries neither `task_id` nor `timestamp`.
#[derive(Serialize)]
struct Sentinel<'a> {
    #[serde(rename = "type")]
    event_type: &'a str,
    content: &'a str,
}

/// ponytail: `serde_json` writes `{"a":1}` where `json.dumps` writes `{"a": 1}`.
/// Key *order* is Python's (that is what a derived `Serialize` gives); the
/// spacing is not, and no consumer of this stream sees it — every one parses the
/// payload. Reach for `workflow_engine::PythonJson` if that ever stops being true.
fn frame<T: Serialize>(payload: &T) -> String {
    let body = serde_json::to_string(payload).unwrap_or_else(|_| "{}".into());
    format!("data: {body}{SEP}{SEP}")
}

struct Tail {
    state: Arc<AppState>,
    process_id: i64,
    last_log_id: i64,
    last_ping: Instant,
    /// Python sleeps at the end of every pass that did not break. Yielding a
    /// chunk returns from the loop, so the sleep it owes moves to the next entry.
    wait: bool,
}

async fn stream_events(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    PathId(process_id): PathId<i64>,
) -> Result<Response, ApiError> {
    principal.require_scope("process:read")?;
    // Authorisation happens before the response, so a caller who may not see this
    // process gets a 404 body rather than an empty 200 stream.
    accessible_process(&state, &principal, &headers, process_id).await?;

    let tail = Tail {
        state,
        process_id,
        last_log_id: 0,
        last_ping: Instant::now(),
        wait: false,
    };

    let stream = futures::stream::unfold(Some(tail), |slot| async move {
        let mut tail = slot?;
        loop {
            if tail.wait {
                tokio::time::sleep(POLL).await;
                tail.wait = false;
                if tail.last_ping.elapsed() >= PING {
                    tail.last_ping = Instant::now();
                    return Some((chunk(PING_FRAME.to_string()), Some(tail)));
                }
            }

            let status: Option<String> =
                match sqlx::query_scalar(&crate::db::sql(
                    "SELECT status FROM process WHERE id = ?",
                    tail.state.backend,
                ))
                    .bind(tail.process_id)
                    .fetch_optional(&tail.state.any)
                    .await
                {
                    Ok(status) => status,
                    // Python's generator would raise and drop the connection; the
                    // client reconnects either way.
                    Err(e) => {
                        logd!("process stream: {e}");
                        return None;
                    }
                };

            let Some(status) = status else {
                let sentinel = Sentinel { event_type: "error", content: "process not found" };
                return Some((chunk(frame(&sentinel)), None));
            };

            // No limit, exactly as in Python: a backlog is replayed in one pass.
            let rows: Vec<EventOut> = match sqlx::query_as(&crate::db::sql(&format!(
                "SELECT {EVENT_COLUMNS} FROM eventlog \
                 WHERE process_id = ? AND id > ? ORDER BY id ASC"
            ), tail.state.backend))
            .bind(tail.process_id)
            .bind(tail.last_log_id)
            .fetch_all(&tail.state.any)
            .await
            {
                Ok(rows) => rows,
                Err(e) => {
                    logd!("process stream: {e}");
                    return None;
                }
            };

            let mut out = String::new();
            for row in &rows {
                tail.last_log_id = row.id;
                out.push_str(&frame(&LogFrame {
                    task_id: row.task_id,
                    event_type: &row.event_type,
                    content: &row.content,
                    // Naive column, so no `Z` — the trap the todos port hit.
                    timestamp: iso_from_sql(&row.created_at),
                }));
            }

            if TERMINAL.contains(&status.as_str()) {
                // **Ported bug, not ported intent.** The sentinel is emitted only
                // when this pass drained nothing, so an already-terminal process
                // with a backlog replays it and closes with no sentinel at all.
                // `client/src/sse.rs:355` is written around this: consumers gate
                // on polled status, not on the stream ending politely.
                if rows.is_empty() {
                    out.push_str(&frame(&Sentinel { event_type: "terminal", content: &status }));
                }
                return Some((chunk(out), None));
            }
            if HUMAN_GATE.contains(&status.as_str()) {
                // A human gate always gets its sentinel, backlog or not.
                out.push_str(&frame(&Sentinel { event_type: "terminal", content: &status }));
                return Some((chunk(out), None));
            }

            tail.wait = true;
            if !out.is_empty() {
                return Some((chunk(out), Some(tail)));
            }
        }
    });

    Ok((
        [
            (header::CONTENT_TYPE, "text/event-stream; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        axum::body::Body::from_stream(stream),
    )
        .into_response())
}

fn chunk(text: String) -> Result<Bytes, std::convert::Infallible> {
    Ok(Bytes::from(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn query(client_id: Option<&str>, project_id: Option<i64>, unassigned_only: bool) -> ListQuery {
        ListQuery {
            limit: 50,
            client_id: client_id.map(str::to_string),
            project_id,
            unassigned_only,
        }
    }

    #[test]
    fn listing_needs_an_explicit_filter() {
        // No filter at all is the second 400, and its text is the contract.
        let err = list_filters(false, &query(None, None, false)).unwrap_err();
        assert_eq!(err.status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(
            err.message,
            "Must specify one of: project_id, client_id, or unassigned_only=true"
        );
        // `?client_id=` is falsy in Python, so it is not a filter either.
        assert!(list_filters(false, &query(Some(""), None, false)).is_err());

        // Any one of the three is enough.
        assert!(list_filters(false, &query(Some("app"), None, false)).is_ok());
        assert!(list_filters(false, &query(None, Some(7), false)).is_ok());
        assert!(list_filters(false, &query(None, None, true)).is_ok());
    }

    #[test]
    fn a_workspace_token_must_name_a_project_and_never_sees_unassigned() {
        let err = list_filters(true, &query(None, None, true)).unwrap_err();
        assert_eq!(err.status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(err.message, "project_id is required for a workspace-scoped token.");
        // Even with a client_id: the workspace check runs first.
        assert!(list_filters(true, &query(Some("app"), None, false)).is_err());

        // `unassigned_only` is forced off rather than refused.
        let filters = list_filters(true, &query(None, Some(7), true)).unwrap();
        assert_eq!(
            filters,
            ListFilters { client_id: None, project_id: Some(7), unassigned_only: false }
        );
    }

    #[test]
    fn a_blank_client_id_passes_the_guard_and_filters_on_nothing() {
        // Whitespace is truthy to Python but strips to empty, so it satisfies the
        // "must specify one" rule and then adds no WHERE clause.
        let filters = list_filters(false, &query(Some("  "), None, false)).unwrap();
        assert_eq!(filters.client_id, None);
        assert_eq!(filters.project_id, None);
        assert!(!filters.unassigned_only);

        let filters = list_filters(false, &query(Some(" app "), None, false)).unwrap();
        assert_eq!(filters.client_id.as_deref(), Some("app"));
    }

    fn sync_state(status: &str, awaiting_review: i64, has_dag: bool) -> SyncState {
        SyncState { status: status.into(), awaiting_review, has_dag }
    }

    /// The whole decision table, because a cross-render diffs `action` and
    /// `detail` and the ten HTTP tests in this domain assert on a mocked
    /// executor instead.
    #[test]
    fn sync_picks_one_branch_per_phase() {
        for status in TERMINAL {
            assert_eq!(sync_branch(&sync_state(status, 0, true)), SyncBranch::Terminal);
        }
        assert_eq!(
            sync_branch(&sync_state("approval_required", 0, true)),
            SyncBranch::ApprovalGate
        );
        assert_eq!(
            sync_branch(&sync_state("task_review_required", 3, true)),
            SyncBranch::ReviewGate
        );
        for status in ["pending", "planning"] {
            assert_eq!(sync_branch(&sync_state(status, 0, false)), SyncBranch::RequeuePlan);
        }
        assert_eq!(
            sync_branch(&sync_state("approved", 0, true)),
            SyncBranch::RequeueApprovedExecution
        );
        // Blank counts as absent here — `/retry` disagrees, see below.
        assert_eq!(
            sync_branch(&sync_state("approved", 0, false)),
            SyncBranch::ApprovedWithoutDag
        );
        assert_eq!(
            sync_branch(&sync_state("running", 0, true)),
            SyncBranch::ResetRunningAndRequeue
        );
        assert_eq!(sync_branch(&sync_state("who knows", 0, true)), SyncBranch::Unexpected);
    }

    #[test]
    fn a_running_process_with_an_open_review_gate_is_aligned_not_reset() {
        // Both branches match `running`; the alignment check runs first, so a
        // reviewer's task is never reset out from under them.
        assert_eq!(sync_branch(&sync_state("running", 1, true)), SyncBranch::AlignStatus);
        assert_eq!(
            sync_branch(&sync_state("running", 0, true)),
            SyncBranch::ResetRunningAndRequeue
        );
        // A gate open on a terminal or blocked process changes nothing.
        assert_eq!(sync_branch(&sync_state("failed", 2, true)), SyncBranch::Terminal);
        assert_eq!(
            sync_branch(&sync_state("task_review_required", 2, true)),
            SyncBranch::ReviewGate
        );
    }

    /// Verbatim from `services/process_sync_service.py` — these land in the body.
    #[test]
    fn sync_details_are_pythons_strings() {
        assert_eq!(
            sync_terminal_detail("failed"),
            "Process failed; use POST /retry to re-plan or re-run execution, or retry \
             individual failed tasks — sync does not apply."
        );
        assert_eq!(
            sync_terminal_detail("completed"),
            "Process is already finished; sync does nothing."
        );
        assert_eq!(sync_terminal_detail("cancelled"), sync_terminal_detail("completed"));
        assert_eq!(
            sync_review_gate_detail(2),
            "Waiting for human task review (2 task(s) in awaiting_review). \
             Use the review actions on each task."
        );
        // `count(…)` is not used on purpose: Python writes "1 task(s)" too.
        assert!(sync_review_gate_detail(1).contains("(1 task(s) in awaiting_review)"));
    }

    #[test]
    fn counting_task_statuses() {
        let statuses: Vec<String> =
            ["pending", "running", "pending", "awaiting_review"].iter().map(|s| s.to_string()).collect();
        let counts = task_status_counts(&statuses);
        assert_eq!(counts["pending"], 2);
        assert_eq!(counts["running"], 1);
        assert_eq!(counts["awaiting_review"], 1);
        // Absent, not zero — the UI reads `counts.get(status, 0)`.
        assert!(counts.get("failed").is_none());
        assert!(task_status_counts(&[]).is_empty());
    }

    #[test]
    fn retry_replans_only_when_no_task_ever_landed() {
        assert_eq!(retry_plan("running", true, Some("{}")), RetryPlan::NotFailed);
        assert_eq!(retry_plan("completed", false, None), RetryPlan::NotFailed);
        // No tasks: the planner never got far enough, so re-plan rather than
        // re-run — and the stored DAG is not even looked at.
        assert_eq!(retry_plan("failed", false, None), RetryPlan::Replan);
        assert_eq!(retry_plan("failed", false, Some("{}")), RetryPlan::Replan);
        assert_eq!(retry_plan("failed", true, Some("{}")), RetryPlan::Reexecute);
        assert_eq!(retry_plan("failed", true, None), RetryPlan::MissingDag);
        assert_eq!(retry_plan("failed", true, Some("")), RetryPlan::MissingDag);
        // Whitespace is *not* missing here, unlike `sync`: it gets through and
        // fails later as "Invalid stored DAG JSON".
        assert_eq!(retry_plan("failed", true, Some("  ")), RetryPlan::Reexecute);
    }

    #[test]
    fn the_client_header_beats_the_body_and_blanks_are_absent() {
        assert_eq!(merged_client_id(Some(" hdr "), Some("body")).as_deref(), Some("hdr"));
        assert_eq!(merged_client_id(None, Some(" body ")).as_deref(), Some("body"));
        assert_eq!(merged_client_id(Some("   "), Some("body")).as_deref(), Some("body"));
        assert_eq!(merged_client_id(Some("  "), Some("  ")), None);
        assert_eq!(merged_client_id(None, None), None);
        assert_eq!(merged_client_id(None, Some(&"x".repeat(300))).unwrap().len(), 256);
    }

    /// Pasted from `python -c "print(repr(render_team_context_for_planner(…)))"`.
    /// The planner reads this, and the ordering rule (depth, then lowercased
    /// name) is the part a port silently gets wrong.
    #[test]
    fn team_context_matches_pythons_render() {
        let roster: TeamRoster = serde_json::from_str(
            r#"{"roles":[
                {"id":"b","name":"Beta","parent_id":"a"},
                {"id":"a","name":"Alpha","description":" leads "},
                {"id":"c","name":"alpha2","parent_id":"a"}
            ]}"#,
        )
        .expect("roster");
        assert_eq!(
            render_team_context_for_planner("Pod", Some("  "), Some("#fff"), &roster),
            "Team template: Pod\n\
             Team color (UI hint): #fff\n\
             Preferred team roster (map subagent `role` names and responsibilities to these where sensible):\n\
             - Alpha (id=a): leads\n\
             \x20 - alpha2 (id=c)\n\
             \x20 - Beta (id=b)"
        );
    }

    #[test]
    fn an_unreadable_snapshot_plans_without_a_roster_hint() {
        assert_eq!(team_context_from_snapshot(None), None);
        assert_eq!(team_context_from_snapshot(Some("   ")), None);
        assert_eq!(team_context_from_snapshot(Some("not json")), None);
        assert_eq!(team_context_from_snapshot(Some("[1,2]")), None);
        // A snapshot with no roster object is `None`, not an empty roster.
        assert_eq!(team_context_from_snapshot(Some(r#"{"name":"Pod"}"#)), None);

        let context = team_context_from_snapshot(Some(
            r##"{"name":"","description":null,"color":"#fff","roster":{"roles":[{"id":"a","name":"Alpha"}]}}"##,
        ))
        .expect("context");
        // A blank name falls back to "Team", and a null description is dropped.
        assert!(context.starts_with("Team template: Team\nTeam color (UI hint): #fff\n"));
        assert!(context.ends_with("- Alpha (id=a)"));
    }

    /// `client/src/sse.rs::is_sentinel`, verbatim.
    fn is_sentinel(frame: &Value) -> bool {
        frame.get("task_id").map_or(true, Value::is_null)
            && frame.get("timestamp").map_or(true, Value::is_null)
    }

    fn parse(frame: &str) -> Value {
        let body = frame
            .strip_prefix("data: ")
            .and_then(|f| f.strip_suffix("\r\n\r\n"))
            .expect("data frame");
        serde_json::from_str(body).expect("json payload")
    }

    #[test]
    fn an_error_log_row_is_not_a_sentinel() {
        // The whole reason both fields are on `LogFrame`: a planner failure is
        // stored with event_type "error", and a client that read it as the
        // stream's error sentinel would stop tailing a process still running.
        let row = parse(&frame(&LogFrame {
            task_id: Some(4),
            event_type: "error",
            content: "planner failed",
            timestamp: "2026-08-06T09:00:00".into(),
        }));
        assert!(!is_sentinel(&row));
        assert_eq!(row["type"], "error");
        // Naive column: no trailing Z.
        assert_eq!(row["timestamp"], "2026-08-06T09:00:00");

        // A row with no task_id still carries a timestamp, so it is still a row.
        let unattached = parse(&frame(&LogFrame {
            task_id: None,
            event_type: "status_change",
            content: "running",
            timestamp: "2026-08-06T09:00:00".into(),
        }));
        assert!(!is_sentinel(&unattached));

        for sentinel in [
            Sentinel { event_type: "terminal", content: "completed" },
            Sentinel { event_type: "error", content: "process not found" },
        ] {
            let parsed = parse(&frame(&sentinel));
            assert!(is_sentinel(&parsed));
            assert!(parsed.get("task_id").is_none());
            assert!(parsed.get("timestamp").is_none());
        }
    }
}
