//! Workspace CRUD and `/me/workspace` — `app/workspaces_routes.py`, all six
//! routes.
//!
//! **Master-key only, except `/me/workspace`.** A workspace-scoped token must
//! never create, rename or archive tenants; the one endpoint it may call is the
//! resolver that tells it which workspace it belongs to.
//!
//! `DELETE` is an archive, not a delete, and it cascades: every non-revoked
//! token in the workspace is revoked, every team template it owns is removed
//! (with any process pointing at one detached first), and the row is stamped.
//! That is three tables Python was the only writer of, and the token half is
//! the one that matters — `auth.rs` reads `api_tokens` on every request.
//!
//! ponytail: `state.pool`, like every domain but `projects`.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};
use sqlx::FromRow;

use crate::auth::Principal;
use crate::error::{ApiError, PathId};
use crate::wire::{check_len, iso_from_sql, optional_str, parse_body, required_str, sql_now};
use crate::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        // Both spellings. FastAPI served `/workspaces/` and redirected the bare
        // form onto it, but `GET /api/v1/workspaces` is what every caller sends;
        // leaving it to the proxy was correct while there was a proxy, and a
        // 404 the moment there was not.
        .route("/api/v1/workspaces", get(list_workspaces).post(create_workspace))
        .route("/api/v1/workspaces/", get(list_workspaces).post(create_workspace))
        .route(
            "/api/v1/workspaces/{workspace_id}",
            get(get_workspace).patch(update_workspace).delete(delete_workspace),
        )
        .route("/api/v1/me/workspace", get(get_my_workspace))
}

/// `_require_master_key`.
fn require_master_key(principal: &Principal) -> Result<(), ApiError> {
    if principal.workspace_id.is_some() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Workspaces cannot be managed using an API token.",
        ));
    }
    Ok(())
}

#[derive(FromRow)]
struct WorkspaceRow {
    id: i64,
    name: String,
    slug: String,
    description: Option<String>,
    archived_at: Option<String>,
    created_at: String,
    updated_at: String,
}

const COLUMNS: &str = "id, name, slug, description, CAST(archived_at AS TEXT) AS archived_at, \
     CAST(created_at AS TEXT) AS created_at, CAST(updated_at AS TEXT) AS updated_at";

impl WorkspaceRow {
    fn to_out(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "slug": self.slug,
            "description": self.description,
            "archived_at": self.archived_at.as_deref().map(iso_from_sql),
            "created_at": iso_from_sql(&self.created_at),
            "updated_at": iso_from_sql(&self.updated_at),
        })
    }
}

/// `require_active_workspace`: an archived workspace is **404, not 403** — it is
/// hidden rather than refused.
async fn require_active(state: &AppState, workspace_id: i64) -> Result<WorkspaceRow, ApiError> {
    let row: Option<WorkspaceRow> =
        sqlx::query_as(&format!("SELECT {COLUMNS} FROM workspace WHERE id = ?"))
            .bind(workspace_id)
            .fetch_optional(&state.pool)
            .await?;
    match row {
        Some(row) if row.archived_at.is_none() => Ok(row),
        _ => Err(ApiError::not_found("Workspace not found")),
    }
}

/// `_slugify`: every run of non-`[a-z0-9]` becomes one `-`, ends trimmed, and an
/// empty result is the literal `workspace`.
fn slugify(value: &str) -> String {
    let lowered = value.trim().to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut pending_dash = false;
    for c in lowered.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c);
        } else {
            pending_dash = true;
        }
    }
    let slug = out.trim_matches('-');
    if slug.is_empty() {
        "workspace".to_string()
    } else {
        slug.to_string()
    }
}

async fn list_workspaces(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    require_master_key(&principal)?;
    let include_archived = bool_query(query.as_deref(), "include_archived", false)?;
    let sql = if include_archived {
        format!("SELECT {COLUMNS} FROM workspace ORDER BY id ASC")
    } else {
        format!("SELECT {COLUMNS} FROM workspace WHERE archived_at IS NULL ORDER BY id ASC")
    };
    let rows: Vec<WorkspaceRow> = sqlx::query_as(&sql).fetch_all(&state.pool).await?;
    let out: Vec<Value> = rows.iter().map(WorkspaceRow::to_out).collect();
    Ok(Json(json!({ "workspaces": out })).into_response())
}

/// A `bool` query parameter, the way FastAPI parses one.
fn bool_query(query: Option<&str>, name: &str, default: bool) -> Result<bool, ApiError> {
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        if key == name {
            return match value.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => Ok(true),
                "0" | "false" | "no" | "off" => Ok(false),
                _ => Err(ApiError::validation(vec![json!({
                    "type": "bool_parsing",
                    "loc": ["query", name],
                    "msg": "Input should be a valid boolean, unable to interpret input",
                })])),
            };
        }
    }
    Ok(default)
}

async fn create_workspace(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    raw: Bytes,
) -> Result<Response, ApiError> {
    require_master_key(&principal)?;
    let body = parse_body(&raw)?;

    let mut errors = Vec::new();
    let name = required_str(&mut errors, &body, "name");
    if body.get("name").is_some_and(Value::is_string) {
        check_len(&mut errors, &["name"], Some(name.as_str()), 1, 256);
    }
    let slug = optional_str(&mut errors, &body, "slug");
    if let Some(slug) = slug.as_deref().filter(|_| body.get("slug").is_some_and(Value::is_string)) {
        check_len(&mut errors, &["slug"], Some(slug), 1, 128);
    }
    let description = optional_str(&mut errors, &body, "description");
    if let Some(description) = description
        .as_deref()
        .filter(|_| body.get("description").is_some_and(Value::is_string))
    {
        check_len(&mut errors, &["description"], Some(description), 0, 4096);
    }
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    let slug = slugify(slug.as_deref().filter(|s| !s.is_empty()).unwrap_or(&name));
    let taken: Option<i64> = sqlx::query_scalar("SELECT id FROM workspace WHERE slug = ?")
        .bind(&slug)
        .fetch_optional(&state.pool)
        .await?;
    if taken.is_some() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("Workspace slug '{slug}' already exists"),
        ));
    }

    let now = sql_now();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO workspace (name, slug, description, archived_at, created_at, updated_at) \
         VALUES (?, ?, ?, NULL, ?, ?) RETURNING id",
    )
    .bind(name.trim())
    .bind(&slug)
    // `req.description.strip() if req.description else None` — a blank string
    // is stored as NULL, not as "".
    .bind(description.filter(|d| !d.is_empty()).map(|d| d.trim().to_string()))
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.pool)
    .await?;

    let row = require_active(&state, id).await?;
    Ok((StatusCode::CREATED, Json(row.to_out())).into_response())
}

async fn get_workspace(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(workspace_id): PathId<i64>,
) -> Result<Response, ApiError> {
    require_master_key(&principal)?;
    let row = require_active(&state, workspace_id).await?;
    Ok(Json(row.to_out()).into_response())
}

async fn update_workspace(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(workspace_id): PathId<i64>,
    raw: Bytes,
) -> Result<Response, ApiError> {
    require_master_key(&principal)?;
    let body = parse_body(&raw)?;

    let mut errors = Vec::new();
    let name = optional_str(&mut errors, &body, "name");
    if let Some(name) = name.as_deref().filter(|_| body.get("name").is_some_and(Value::is_string)) {
        check_len(&mut errors, &["name"], Some(name), 1, 256);
    }
    let description = optional_str(&mut errors, &body, "description");
    if let Some(description) = description
        .as_deref()
        .filter(|_| body.get("description").is_some_and(Value::is_string))
    {
        check_len(&mut errors, &["description"], Some(description), 0, 4096);
    }
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    require_active(&state, workspace_id).await?;

    if let Some(name) = name {
        sqlx::query("UPDATE workspace SET name = ? WHERE id = ?")
            .bind(name.trim())
            .bind(workspace_id)
            .execute(&state.pool)
            .await?;
    }
    if let Some(description) = description {
        sqlx::query("UPDATE workspace SET description = ? WHERE id = ?")
            .bind((!description.is_empty()).then(|| description.trim().to_string()))
            .bind(workspace_id)
            .execute(&state.pool)
            .await?;
    }
    // Stamped on every PATCH, including one that changed nothing.
    sqlx::query("UPDATE workspace SET updated_at = ? WHERE id = ?")
        .bind(sql_now())
        .bind(workspace_id)
        .execute(&state.pool)
        .await?;

    let row = require_active(&state, workspace_id).await?;
    Ok(Json(row.to_out()).into_response())
}

const ARCHIVE_REASON: &str = "Workspace archived";

/// `archive_workspace`. Not a delete: the tenant is hidden, its tokens are
/// revoked so nothing keeps authenticating, and its teams go — with any process
/// that pointed at one detached first, because that FK outlives the team.
async fn delete_workspace(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(workspace_id): PathId<i64>,
) -> Result<Response, ApiError> {
    require_master_key(&principal)?;
    let row = require_active(&state, workspace_id).await?;
    // The `already archived` 409 in Python is unreachable — `require_active`
    // has already 404'd — and it is unreachable here for the same reason.
    if row.slug == "default" {
        return Err(ApiError::bad_request("Cannot archive the Default workspace"));
    }

    let now = sql_now();
    let tokens: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM api_tokens WHERE workspace_id = ? AND status != 'revoked'",
    )
    .bind(workspace_id)
    .fetch_all(&state.pool)
    .await?;
    for token_id in &tokens {
        sqlx::query(
            "UPDATE api_tokens SET status = 'revoked', revoked_at = ?, revoked_reason = ?, \
             updated_at = ? WHERE id = ?",
        )
        .bind(&now)
        .bind(ARCHIVE_REASON)
        .bind(&now)
        .bind(token_id)
        .execute(&state.pool)
        .await?;
    }

    let teams: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM teamtemplate WHERE workspace_id = ?")
            .bind(workspace_id)
            .fetch_all(&state.pool)
            .await?;
    for team_id in &teams {
        sqlx::query("UPDATE process SET team_template_id = NULL WHERE team_template_id = ?")
            .bind(team_id)
            .execute(&state.pool)
            .await?;
        sqlx::query("DELETE FROM teamtemplate WHERE id = ?")
            .bind(team_id)
            .execute(&state.pool)
            .await?;
    }

    sqlx::query("UPDATE workspace SET archived_at = ?, updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(&now)
        .bind(workspace_id)
        .execute(&state.pool)
        .await?;

    Ok(Json(json!({
        "ok": true,
        "archived_at": iso_from_sql(&now),
        "tokens_revoked": tokens.len(),
        "teams_removed": teams.len(),
    }))
    .into_response())
}

async fn get_my_workspace(
    State(state): State<Arc<AppState>>,
    principal: Principal,
) -> Result<Response, ApiError> {
    let Some(workspace_id) = principal.workspace_id else {
        return Err(ApiError::bad_request(
            "Master key is not bound to a workspace; use GET /workspaces to list them.",
        ));
    };
    let row = require_active(&state, workspace_id).await?;
    Ok(Json(row.to_out()).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_collapse_runs_and_never_come_out_empty() {
        assert_eq!(slugify("Acme Corp"), "acme-corp");
        assert_eq!(slugify("  Hello -- World!! "), "hello-world");
        assert_eq!(slugify("ALL/CAPS"), "all-caps");
        // Nothing usable left, so the fallback name.
        assert_eq!(slugify("!!!"), "workspace");
        assert_eq!(slugify(""), "workspace");
    }

    #[test]
    fn only_the_master_key_manages_tenants() {
        assert!(require_master_key(&Principal::unrestricted()).is_ok());
        let tenant =
            Principal { workspace_id: Some(3), token_id: Some(1), scopes: vec!["*".into()] };
        assert_eq!(require_master_key(&tenant).unwrap_err().status, StatusCode::FORBIDDEN);
    }
}
