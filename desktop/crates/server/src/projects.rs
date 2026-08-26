//! Projects: the first CRUD domain moved off Python (ADR 0007), ported from
//! `app/projects_routes.py`.
//!
//! Rust owns the `project` table from here. Two edges are deliberate:
//!
//! - `DELETE` nullifies `process.project_id`, a write to a table the processes
//!   domain still owns. It has to happen with the delete or the FK dangles.
//! - `GET /{id}/processes` has moved with the processes domain and is registered
//!   in [`crate::processes`], not here — it reads the process table and checks
//!   project access without `process:read`, which makes it that domain's rule to
//!   keep. `/{id}/workspace/*` is not here either: it was a different router in
//!   Python (`workspace_files_router`) that happened to share the path, and it
//!   lives in [`crate::workspace_files`] now.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::FromRow;

use crate::auth::Principal;
use crate::db;
use crate::error::{ApiError, PathId};
use crate::wire::{iso_from_sql, parse_body, sql_now, sql_time};
use crate::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        // Both spellings: FastAPI declares the collection with a trailing slash
        // and 307s the other, and a redirect through a proxy is a worse contract
        // than simply answering.
        .route("/api/v1/projects", get(list_projects).post(create_project))
        .route("/api/v1/projects/", get(list_projects).post(create_project))
        .route(
            "/api/v1/projects/{project_id}",
            get(get_project).patch(update_project).delete(delete_project),
        )
        .route(
            "/api/v1/projects/{project_id}/workspace-state",
            get(get_workspace_state).put(put_workspace_state),
        )
        .route(
            "/api/v1/projects/{project_id}/planning-context",
            get(get_planning_context).patch(patch_planning_context),
        )
}

// ---------------------------------------------------------------------------
// Wire shapes
// ---------------------------------------------------------------------------

#[derive(Debug, FromRow, Serialize)]
pub struct ProjectOut {
    pub id: i64,
    pub workspace_id: Option<i64>,
    pub name: String,
    pub description: Option<String>,
    pub color: Option<String>,
    #[serde(serialize_with = "sql_time")]
    pub created_at: String,
    #[serde(serialize_with = "sql_time")]
    pub updated_at: String,
}

const PROJECT_COLUMNS: &str = "CAST(id AS BIGINT) AS id, \n     CAST(workspace_id AS BIGINT) AS workspace_id, name, description, color, \n     CAST(created_at AS TEXT) AS created_at, CAST(updated_at AS TEXT) AS updated_at";

#[derive(Debug, Deserialize)]
pub struct ProjectCreate {
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    color: Option<String>,
    /// Required for master-key callers; ignored for workspace-scoped tokens.
    #[serde(default)]
    workspace_id: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ProjectUpdate {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    color: Option<String>,
}

// ---------------------------------------------------------------------------
// Access
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct ProjectOwner {
    workspace_id: Option<i64>,
    workspace_archived: Option<String>,
    owner_user_id: Option<i64>,
}

/// `assert_token_project_access` + `require_one` in one query.
///
/// 404 — never 401 — is the tenancy contract: a workspace token or a signed-in
/// user asking about another tenant's project must not learn that it exists.
pub(crate) async fn assert_access(
    state: &AppState,
    principal: &Principal,
    project_id: i64,
) -> Result<(), ApiError> {
    let row: Option<ProjectOwner> = sqlx::query_as(&db::sql(
        "SELECT CAST(p.workspace_id AS BIGINT) AS workspace_id, CAST(w.archived_at AS TEXT) AS workspace_archived, \
         CAST(w.user_id AS BIGINT) AS owner_user_id \
         FROM project p LEFT JOIN workspace w ON w.id = p.workspace_id \
         WHERE p.id = ?"
    , state.backend))
    .bind(project_id)
    .fetch_optional(&state.any)
    .await?;

    let Some(row) = row else {
        return Err(ApiError::not_found("Not found"));
    };
    // A project with no workspace, or one whose workspace is archived, is hidden
    // from everyone including the master key.
    if row.workspace_id.is_none() || row.workspace_archived.is_some() {
        return Err(ApiError::not_found("Not found"));
    }
    if let Some(ws) = principal.workspace_id {
        if Some(ws) != row.workspace_id {
            return Err(ApiError::not_found("Not found"));
        }
        return Ok(());
    }
    if let Some(uid) = principal.scoped_user_id() {
        if row.owner_user_id != Some(uid) {
            return Err(ApiError::not_found("Not found"));
        }
    }
    Ok(())
}

async fn load_project(state: &AppState, project_id: i64) -> Result<ProjectOut, ApiError> {
    sqlx::query_as(&db::sql(&format!("SELECT {PROJECT_COLUMNS} FROM project WHERE id = ?"), state.backend))
        .bind(project_id)
        .fetch_optional(&state.any)
        .await?
        .ok_or_else(|| ApiError::not_found("Project not found"))
}

// ---------------------------------------------------------------------------
// Validation (mirrors app/schema_fields.py)
// ---------------------------------------------------------------------------

fn validate(name: Option<&str>, description: Option<&str>, color: Option<&str>, name_required: bool) -> Result<(), ApiError> {
    let mut errors = Vec::new();
    match name {
        None if name_required => errors.push(ApiError::field_error("name", "missing", "Field required")),
        Some(v) if v.is_empty() => errors.push(ApiError::field_error(
            "name",
            "string_too_short",
            "String should have at least 1 character",
        )),
        Some(v) if v.chars().count() > 256 => errors.push(ApiError::field_error(
            "name",
            "string_too_long",
            "String should have at most 256 characters",
        )),
        _ => {}
    }
    if description.is_some_and(|v| v.chars().count() > 4096) {
        errors.push(ApiError::field_error(
            "description",
            "string_too_long",
            "String should have at most 4096 characters",
        ));
    }
    if color.is_some_and(|v| v.chars().count() > 32) {
        errors.push(ApiError::field_error(
            "color",
            "string_too_long",
            "String should have at most 32 characters",
        ));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ApiError::validation(errors))
    }
}

/// Python trims *after* validating, so a name of three spaces is valid and
/// stored empty. Keeping that order matters more than it looks: the other way
/// round turns an accepted request into a 422.
fn trimmed(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn list_projects(
    State(state): State<Arc<AppState>>,
    principal: Principal,
) -> Result<Response, ApiError> {
    let rows: Vec<ProjectOut> = match (principal.workspace_id, principal.scoped_user_id()) {
        (Some(ws), _) => sqlx::query_as(&db::sql(&format!(
            "SELECT {PROJECT_COLUMNS} FROM project WHERE workspace_id = ? ORDER BY id ASC"
        ), state.backend))
        .bind(ws)
        .fetch_all(&state.any)
        .await?,
        (None, Some(uid)) => sqlx::query_as(&db::sql(&format!(
            "SELECT {PROJECT_COLUMNS} FROM project \
             WHERE workspace_id IN (SELECT id FROM workspace WHERE user_id = ? AND archived_at IS NULL) \
             ORDER BY id ASC"
        ), state.backend))
        .bind(uid)
        .fetch_all(&state.any)
        .await?,
        // Master key sees every live tenant. Projects with no workspace are
        // excluded here exactly as the Python `IN (...)` excludes NULL.
        (None, None) => sqlx::query_as(&db::sql(&format!(
            "SELECT {PROJECT_COLUMNS} FROM project \
             WHERE workspace_id IN (SELECT id FROM workspace WHERE archived_at IS NULL) \
             ORDER BY id ASC"
        ), state.backend))
        .fetch_all(&state.any)
        .await?,
    };
    Ok(Json(json!({ "projects": rows })).into_response())
}

async fn create_project(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    // Raw bytes, not `Option<Json<ProjectCreate>>`: axum's `Json` extractor
    // only yields `None` for a body-less request with no `Content-Type` at
    // all — an empty body sent *with* `application/json` (an argument-less
    // POST from most clients) fails to parse and axum answers its own
    // plain-text 400 before this handler runs.
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    if body.is_empty() {
        return Err(ApiError::validation(vec![ApiError::field_error(
            "name", "missing", "Field required",
        )]));
    }
    let req: ProjectCreate = serde_json::from_value(parse_body(&body)?).map_err(|e| {
        ApiError::validation(vec![ApiError::field_error_at(
            &["body"],
            "model_attributes_type",
            &e.to_string(),
        )])
    })?;
    validate(req.name.as_deref(), req.description.as_deref(), req.color.as_deref(), true)?;

    let workspace_id = match (principal.workspace_id, req.workspace_id) {
        (Some(ws), _) => ws,
        (None, Some(ws)) => {
            crate::identity::assert_workspace_visible(&state, &principal, ws).await?;
            ws
        }
        (None, None) => {
            if let Some(uid) = principal.user_id {
                let username = principal
                    .email
                    .as_deref()
                    .and_then(|e| e.split('@').next())
                    .unwrap_or("user");
                let kind = if principal.mode == crate::auth::AuthMode::OpenLocal {
                    "local"
                } else {
                    "cloud"
                };
                crate::identity::ensure_user_workspace(&state, uid, username, kind).await?
            } else {
                sqlx::query_scalar::<_, i64>(&db::sql(
                    "SELECT CAST(id AS BIGINT) FROM workspace WHERE slug = 'default'",
                    state.backend,
                ))
                .fetch_optional(&state.any)
                .await?
                .ok_or_else(|| {
                    ApiError::bad_request("workspace_id is required (no Default workspace exists).")
                })?
            }
        }
    };

    let archived: Option<Option<String>> =
        sqlx::query_scalar(&db::sql("SELECT CAST(archived_at AS TEXT) FROM workspace WHERE id = ?", state.backend))
            .bind(workspace_id)
            .fetch_optional(&state.any)
            .await?;
    if !matches!(archived, Some(None)) {
        return Err(ApiError::not_found("Workspace not found"));
    }

    let now = sql_now();
    let id: i64 = sqlx::query_scalar(&db::sql(
        "INSERT INTO project (workspace_id, name, description, color, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?) RETURNING CAST(id AS BIGINT)"
    , state.backend))
    .bind(workspace_id)
    .bind(req.name.unwrap_or_default().trim())
    .bind(trimmed(req.description))
    .bind(trimmed(req.color))
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.any)
    .await?;

    Ok((StatusCode::CREATED, Json(load_project(&state, id).await?)).into_response())
}

async fn get_project(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(project_id): PathId<i64>,
) -> Result<Response, ApiError> {
    assert_access(&state, &principal, project_id).await?;
    Ok(Json(load_project(&state, project_id).await?).into_response())
}

async fn update_project(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(project_id): PathId<i64>,
    // Raw bytes — see `create_project`'s comment. An empty body still means
    // "no changes", same as the old `unwrap_or_default()`.
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let req: ProjectUpdate = if body.is_empty() {
        ProjectUpdate::default()
    } else {
        serde_json::from_value(parse_body(&body)?).map_err(|e| {
            ApiError::validation(vec![ApiError::field_error_at(
                &["body"],
                "model_attributes_type",
                &e.to_string(),
            )])
        })?
    };
    assert_access(&state, &principal, project_id).await?;
    load_project(&state, project_id).await?;

    // `null` and "absent" mean the same thing to the Python model (it checks
    // `is not None`), so a single Option matches without a nested one.
    if req.name.is_some() {
        validate(req.name.as_deref(), None, None, false)?;
    }
    validate(None, req.description.as_deref(), req.color.as_deref(), false)?;

    if let Some(name) = req.name {
        sqlx::query(&db::sql("UPDATE project SET name = ? WHERE id = ?", state.backend))
            .bind(name.trim())
            .bind(project_id)
            .execute(&state.any)
            .await?;
    }
    if req.description.is_some() {
        sqlx::query(&db::sql("UPDATE project SET description = ? WHERE id = ?", state.backend))
            .bind(trimmed(req.description))
            .bind(project_id)
            .execute(&state.any)
            .await?;
    }
    if req.color.is_some() {
        sqlx::query(&db::sql("UPDATE project SET color = ? WHERE id = ?", state.backend))
            .bind(trimmed(req.color))
            .bind(project_id)
            .execute(&state.any)
            .await?;
    }
    touch(&state, project_id).await?;

    Ok(Json(load_project(&state, project_id).await?).into_response())
}

async fn delete_project(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(project_id): PathId<i64>,
) -> Result<Response, ApiError> {
    assert_access(&state, &principal, project_id).await?;
    load_project(&state, project_id).await?;

    // One transaction: a deleted project that left processes pointing at it is
    // a dangling FK the UI renders as a ghost filter.
    let mut tx = state.any.begin().await?;
    sqlx::query(&db::sql("UPDATE process SET project_id = NULL WHERE project_id = ?", state.backend))
        .bind(project_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(&db::sql("DELETE FROM project WHERE id = ?", state.backend))
        .bind(project_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    delete_project_workspace(project_id);
    Ok(Json(json!({ "ok": true })).into_response())
}

/// Best-effort, like `workspace_service.delete_project_workspace`: losing the
/// directory is not worth failing a delete that already committed.
fn delete_project_workspace(project_id: i64) {
    let Some(root) = workspace_root() else { return };
    let dir = root.join(format!("project-{project_id}"));
    if dir.is_dir() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

fn workspace_root() -> Option<PathBuf> {
    if let Some(env) = crate::env_opt("AGENT_PLATFORM_WORKSPACE_ROOT") {
        return Some(PathBuf::from(env));
    }
    // Same fallback as `_default_workspace_root`: beside the database file.
    let db = crate::env_opt("AGENT_PLATFORM_DB_PATH").unwrap_or_else(|| "data/agent_platform.db".into());
    let parent = PathBuf::from(db).parent().map(PathBuf::from).filter(|p| !p.as_os_str().is_empty());
    Some(parent.unwrap_or_else(|| PathBuf::from("data")).join("workspaces"))
}

async fn touch(state: &AppState, project_id: i64) -> Result<(), ApiError> {
    sqlx::query(&db::sql("UPDATE project SET updated_at = ? WHERE id = ?", state.backend))
        .bind(sql_now())
        .bind(project_id)
        .execute(&state.any)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Workspace state (the Flow UI's per-project snapshot blob)
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct WorkspaceStateRow {
    workspace_payload_json: Option<String>,
    updated_at: String,
}

async fn get_workspace_state(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(project_id): PathId<i64>,
) -> Result<Response, ApiError> {
    assert_access(&state, &principal, project_id).await?;
    let row: WorkspaceStateRow =
        sqlx::query_as(&db::sql("SELECT workspace_payload_json, CAST(updated_at AS TEXT) AS updated_at FROM project WHERE id = ?", state.backend))
            .bind(project_id)
            .fetch_optional(&state.any)
            .await?
            .ok_or_else(|| ApiError::not_found("Project not found"))?;

    // Anything that is not a JSON object reads as no payload at all, including
    // stored garbage — the Python version swallows a decode error the same way.
    let payload = row
        .workspace_payload_json
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .filter(Value::is_object);

    Ok(Json(json!({ "payload": payload, "updated_at": iso_from_sql(&row.updated_at) })).into_response())
}

async fn put_workspace_state(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(project_id): PathId<i64>,
    // Raw bytes, not `Option<Json<Value>>` — see `create_project`'s comment.
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    assert_access(&state, &principal, project_id).await?;
    load_project(&state, project_id).await?;

    let payload = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|v| v.get("payload").cloned())
        .unwrap_or_else(|| json!({}));
    let now = sql_now();
    sqlx::query(&db::sql("UPDATE project SET workspace_payload_json = ?, updated_at = ? WHERE id = ?", state.backend))
        .bind(serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into()))
        .bind(&now)
        .bind(project_id)
        .execute(&state.any)
        .await?;

    Ok(Json(json!({ "payload": payload, "updated_at": iso_from_sql(&now) })).into_response())
}

// ---------------------------------------------------------------------------
// Planning context (project-scoped preferences; boards live in the todos domain)
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct PlanningRow {
    last_todo_board_id: Option<i64>,
    planning_prefs_json: Option<String>,
}

async fn planning_context(state: &AppState, project_id: i64) -> Result<Value, ApiError> {
    let row: PlanningRow =
        sqlx::query_as(&db::sql("SELECT CAST(last_todo_board_id AS BIGINT) AS last_todo_board_id, planning_prefs_json FROM project WHERE id = ?", state.backend))
            .bind(project_id)
            .fetch_optional(&state.any)
            .await?
            .ok_or_else(|| ApiError::not_found("Project not found"))?;

    // A board that was deleted or moved to another project reads as unset rather
    // than as a link the UI would follow into someone else's board.
    let mut last_board = row.last_todo_board_id;
    if let Some(board_id) = last_board {
        let owner: Option<Option<i64>> =
            sqlx::query_scalar(&db::sql("SELECT CAST(project_id AS BIGINT) FROM todo_boards WHERE id = ?", state.backend))
                .bind(board_id)
                .fetch_optional(&state.any)
                .await?;
        if owner != Some(Some(project_id)) {
            last_board = None;
        }
    }

    let dismissed = row
        .planning_prefs_json
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.get("onboarding_dismissed").cloned())
        .map(|v| v.as_bool().unwrap_or(!matches!(v, Value::Null | Value::Bool(false))))
        .unwrap_or(false);

    Ok(json!({
        "project_id": project_id,
        "last_todo_board_id": last_board,
        "onboarding_dismissed": dismissed,
    }))
}

async fn get_planning_context(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(project_id): PathId<i64>,
) -> Result<Response, ApiError> {
    assert_access(&state, &principal, project_id).await?;
    Ok(Json(planning_context(&state, project_id).await?).into_response())
}

async fn patch_planning_context(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(project_id): PathId<i64>,
    // Raw bytes, not `Option<Json<Value>>` — see `create_project`'s comment.
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    assert_access(&state, &principal, project_id).await?;
    planning_context(&state, project_id).await?; // 404s a missing project first

    let fields: Map<String, Value> = match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };

    // Presence, not value: an explicit `null` clears the board, an omitted key
    // leaves it. That is `model_dump(exclude_unset=True)` on the Python side.
    if let Some(raw) = fields.get("last_todo_board_id") {
        let board_id = raw.as_i64();
        if raw.is_null() {
            // nothing to validate
        } else if let Some(board_id) = board_id {
            let owner: Option<Option<i64>> =
                sqlx::query_scalar(&db::sql("SELECT CAST(project_id AS BIGINT) FROM todo_boards WHERE id = ?", state.backend))
                    .bind(board_id)
                    .fetch_optional(&state.any)
                    .await?;
            match owner {
                None => return Err(ApiError::not_found("Board not found")),
                Some(Some(other)) if other != project_id => {
                    return Err(ApiError::bad_request("Board belongs to another project"))
                }
                _ => {}
            }
        }
        sqlx::query(&db::sql("UPDATE project SET last_todo_board_id = ? WHERE id = ?", state.backend))
            .bind(board_id)
            .bind(project_id)
            .execute(&state.any)
            .await?;
    }

    if let Some(value) = fields.get("onboarding_dismissed").filter(|v| !v.is_null()) {
        let row: Option<String> =
            sqlx::query_scalar(&db::sql("SELECT planning_prefs_json FROM project WHERE id = ?", state.backend))
                .bind(project_id)
                .fetch_one(&state.any)
                .await?;
        let mut prefs = row
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|v| match v {
                Value::Object(map) => Some(map),
                _ => None,
            })
            .unwrap_or_default();
        prefs.insert("onboarding_dismissed".into(), value.clone());
        sqlx::query(&db::sql("UPDATE project SET planning_prefs_json = ? WHERE id = ?", state.backend))
            .bind(Value::Object(prefs).to_string())
            .bind(project_id)
            .execute(&state.any)
            .await?;
    }

    Ok(Json(planning_context(&state, project_id).await?).into_response())
}
