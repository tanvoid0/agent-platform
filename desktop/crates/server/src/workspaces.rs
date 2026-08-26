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
//! On the `sqlx::Any` pool: every query goes through `db::sql` and ids are
//! selected as `CAST(… AS BIGINT)`, because a Postgres `integer` is int4.

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

/// `_require_master_key`'s message; the check is
/// [`Principal::require_master_key`].
const NOT_A_TENANT: &str = "Workspaces cannot be managed using an API token.";

#[derive(FromRow)]
struct WorkspaceRow {
    id: i64,
    name: String,
    slug: String,
    description: Option<String>,
    archived_at: Option<String>,
    created_at: String,
    updated_at: String,
    user_id: Option<i64>,
}

pub const COLUMNS: &str = "CAST(id AS BIGINT) AS id, name, slug, description, CAST(archived_at AS TEXT) AS archived_at, \
     CAST(created_at AS TEXT) AS created_at, CAST(updated_at AS TEXT) AS updated_at, \
     CAST(user_id AS BIGINT) AS user_id";

impl WorkspaceRow {
    fn to_out(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "slug": self.slug,
            "description": self.description,
            "archived_at": self.archived_at.as_deref().map(iso_from_sql),
            "user_id": self.user_id,
            "created_at": iso_from_sql(&self.created_at),
            "updated_at": iso_from_sql(&self.updated_at),
        })
    }
}

/// `require_active_workspace`: an archived workspace is **404, not 403** — it is
/// hidden rather than refused.
async fn require_active(state: &AppState, workspace_id: i64) -> Result<WorkspaceRow, ApiError> {
    let row: Option<WorkspaceRow> =
        sqlx::query_as(&crate::db::sql(&format!("SELECT {COLUMNS} FROM workspace WHERE id = ?"), state.backend))
            .bind(workspace_id)
            .fetch_optional(&state.any)
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
    let include_archived = bool_query(query.as_deref(), "include_archived", false)?;
    let (sql, bind_user): (String, Option<i64>) = if let Some(uid) = principal.scoped_user_id() {
        let sql = if include_archived {
            format!("SELECT {COLUMNS} FROM workspace WHERE user_id = ? ORDER BY id ASC")
        } else {
            format!("SELECT {COLUMNS} FROM workspace WHERE user_id = ? AND archived_at IS NULL ORDER BY id ASC")
        };
        (sql, Some(uid))
    } else {
        principal.require_master_key(NOT_A_TENANT)?;
        let sql = if include_archived {
            format!("SELECT {COLUMNS} FROM workspace ORDER BY id ASC")
        } else {
            format!("SELECT {COLUMNS} FROM workspace WHERE archived_at IS NULL ORDER BY id ASC")
        };
        (sql, None)
    };
    let sql = crate::db::sql(&sql, state.backend).into_owned();
    let mut q = sqlx::query_as::<_, WorkspaceRow>(&sql);
    if let Some(uid) = bind_user {
        q = q.bind(uid);
    }
    let rows: Vec<WorkspaceRow> = q.fetch_all(&state.any).await?;
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
    if principal.workspace_id.is_some() {
        return Err(ApiError::new(StatusCode::FORBIDDEN, NOT_A_TENANT));
    }
    if !principal.is_operator() && principal.user_id.is_none() {
        return Err(ApiError::new(StatusCode::FORBIDDEN, NOT_A_TENANT));
    }
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
    let taken: Option<i64> = sqlx::query_scalar(&crate::db::sql("SELECT id FROM workspace WHERE slug = ?", state.backend))
        .bind(&slug)
        .fetch_optional(&state.any)
        .await?;
    if taken.is_some() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("Workspace slug '{slug}' already exists"),
        ));
    }

    let now = sql_now();
    let owner = crate::identity::stamp_user_id(&state, &principal);
    let id: i64 = sqlx::query_scalar(&crate::db::sql(
        "INSERT INTO workspace (name, slug, description, archived_at, user_id, created_at, updated_at) \
         VALUES (?, ?, ?, NULL, ?, ?, ?) RETURNING CAST(id AS BIGINT)", state.backend)
    )
    .bind(name.trim())
    .bind(&slug)
    // `req.description.strip() if req.description else None` — a blank string
    // is stored as NULL, not as "".
    .bind(description.filter(|d| !d.is_empty()).map(|d| d.trim().to_string()))
    .bind(owner)
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.any)
    .await?;

    let row = require_active(&state, id).await?;
    Ok((StatusCode::CREATED, Json(row.to_out())).into_response())
}

async fn get_workspace(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(workspace_id): PathId<i64>,
) -> Result<Response, ApiError> {
    crate::identity::assert_workspace_visible(&state, &principal, workspace_id).await?;
    let row = require_active(&state, workspace_id).await?;
    Ok(Json(row.to_out()).into_response())
}

async fn update_workspace(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(workspace_id): PathId<i64>,
    raw: Bytes,
) -> Result<Response, ApiError> {
    if principal.workspace_id.is_some() {
        return Err(ApiError::new(StatusCode::FORBIDDEN, NOT_A_TENANT));
    }
    crate::identity::assert_workspace_visible(&state, &principal, workspace_id).await?;
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
        sqlx::query(&crate::db::sql("UPDATE workspace SET name = ? WHERE id = ?", state.backend))
            .bind(name.trim())
            .bind(workspace_id)
            .execute(&state.any)
            .await?;
    }
    if let Some(description) = description {
        sqlx::query(&crate::db::sql("UPDATE workspace SET description = ? WHERE id = ?", state.backend))
            .bind((!description.is_empty()).then(|| description.trim().to_string()))
            .bind(workspace_id)
            .execute(&state.any)
            .await?;
    }
    // Stamped on every PATCH, including one that changed nothing.
    sqlx::query(&crate::db::sql("UPDATE workspace SET updated_at = ? WHERE id = ?", state.backend))
        .bind(sql_now())
        .bind(workspace_id)
        .execute(&state.any)
        .await?;

    let row = require_active(&state, workspace_id).await?;
    Ok(Json(row.to_out()).into_response())
}

const ARCHIVE_REASON: &str = "Workspace archived";

/// Everything an archive touches except the workspace row itself, returning
/// `(tokens_revoked, teams_removed)` for the response to report.
///
/// Both halves used to select the ids and then loop a statement per id, purely
/// so those counts could be `Vec::len()`. `rows_affected` is the same number, so
/// the selects and the loops are gone and an archive is three statements
/// whatever the tenant's size. Split out from the handler because a `Principal`
/// is awkward to build in a test and these counts are the part worth pinning.
async fn archive_rows(
    state: &AppState,
    workspace_id: i64,
    now: &str,
) -> Result<(u64, u64), ApiError> {
    let tokens_revoked = sqlx::query(&crate::db::sql(
        "UPDATE api_tokens SET status = 'revoked', revoked_at = ?, revoked_reason = ?, \
         updated_at = ? WHERE workspace_id = ? AND status != 'revoked'", state.backend)
    )
    .bind(now)
    .bind(ARCHIVE_REASON)
    .bind(now)
    .bind(workspace_id)
    .execute(&state.any)
    .await?
    .rows_affected();

    // Orphan the processes first: the FK points at `teamtemplate`, so deleting
    // the rows out from under a live process is what the null is avoiding.
    sqlx::query(&crate::db::sql(
        "UPDATE process SET team_template_id = NULL WHERE team_template_id IN \
         (SELECT id FROM teamtemplate WHERE workspace_id = ?)", state.backend)
    )
    .bind(workspace_id)
    .execute(&state.any)
    .await?;
    let teams_removed =
        sqlx::query(&crate::db::sql("DELETE FROM teamtemplate WHERE workspace_id = ?", state.backend))
            .bind(workspace_id)
            .execute(&state.any)
            .await?
            .rows_affected();

    Ok((tokens_revoked, teams_removed))
}

/// `archive_workspace`. Not a delete: the tenant is hidden, its tokens are
/// revoked so nothing keeps authenticating, and its teams go — with any process
/// that pointed at one detached first, because that FK outlives the team.
async fn delete_workspace(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(workspace_id): PathId<i64>,
) -> Result<Response, ApiError> {
    if principal.workspace_id.is_some() {
        return Err(ApiError::new(StatusCode::FORBIDDEN, NOT_A_TENANT));
    }
    crate::identity::assert_workspace_visible(&state, &principal, workspace_id).await?;
    let row = require_active(&state, workspace_id).await?;
    // The `already archived` 409 in Python is unreachable — `require_active`
    // has already 404'd — and it is unreachable here for the same reason.
    if row.slug == "default" {
        return Err(ApiError::bad_request("Cannot archive the Default workspace"));
    }

    let now = sql_now();
    let (tokens_revoked, teams_removed) = archive_rows(&state, workspace_id, &now).await?;

    sqlx::query(&crate::db::sql("UPDATE workspace SET archived_at = ?, updated_at = ? WHERE id = ?", state.backend))
        .bind(&now)
        .bind(&now)
        .bind(workspace_id)
        .execute(&state.any)
        .await?;

    Ok(Json(json!({
        "ok": true,
        "archived_at": iso_from_sql(&now),
        "tokens_revoked": tokens_revoked,
        "teams_removed": teams_removed,
    }))
    .into_response())
}

async fn get_my_workspace(
    State(state): State<Arc<AppState>>,
    principal: Principal,
) -> Result<Response, ApiError> {
    if let Some(workspace_id) = principal.workspace_id {
        crate::identity::assert_workspace_visible(&state, &principal, workspace_id).await?;
        let row = require_active(&state, workspace_id).await?;
        return Ok(Json(row.to_out()).into_response());
    }
    let Some(uid) = principal.user_id else {
        return Err(ApiError::bad_request(
            "This credential is not bound to a user or workspace; use GET /workspaces to list them.",
        ));
    };
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
    let workspace_id = crate::identity::ensure_user_workspace(&state, uid, username, kind).await?;
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
        assert!(Principal::unrestricted().require_master_key(NOT_A_TENANT).is_ok());
        let tenant =
            Principal { workspace_id: Some(3), token_id: Some(1), scopes: vec!["*".into()], ..Principal::unrestricted() };
        assert_eq!(tenant.require_master_key(NOT_A_TENANT).unwrap_err().status, StatusCode::FORBIDDEN);
    }

    /// The counts the response reports used to be `Vec::len()` over ids the
    /// handler had just selected; they are `rows_affected` now. Same numbers or
    /// the archive lies to the caller — and the two rows belonging to the *other*
    /// workspace are what a missing `WHERE workspace_id` would take with it.
    #[tokio::test]
    async fn archiving_touches_one_tenant_and_counts_what_it_changed() {
        let path = std::env::temp_dir()
            .join(format!("agp-archive-{}-{}.db", std::process::id(), line!()));
        let _ = std::fs::remove_file(&path);
        let state = AppState::new(&path, None);
        crate::db::ensure_schema(&state.any).await.unwrap();

        for (id, slug) in [(1, "acme"), (2, "other")] {
            sqlx::query("INSERT INTO workspace (id, name, slug) VALUES (?, ?, ?)")
                .bind(id)
                .bind(slug)
                .bind(slug)
                .execute(&state.any)
                .await
                .unwrap();
        }
        // Two live tokens and one already revoked in the tenant being archived,
        // one live token in the tenant that must be left alone.
        for (id, ws, status) in
            [(1, 1, "active"), (2, 1, "active"), (3, 1, "revoked"), (4, 2, "active")]
        {
            sqlx::query(
                "INSERT INTO api_tokens (id, name, prefix, token_hash, status, workspace_id) \
                 VALUES (?, 't', 'p', ?, ?, ?)",
            )
            .bind(id)
            .bind(format!("hash-{id}"))
            .bind(status)
            .bind(ws)
            .execute(&state.any)
            .await
            .unwrap();
        }
        for (id, ws) in [(1, 1), (2, 1), (3, 2)] {
            sqlx::query(
                "INSERT INTO teamtemplate (id, name, roster_json, created_at, updated_at, \
                 workspace_id) VALUES (?, 'team', '[]', '2026-01-01', '2026-01-01', ?)",
            )
            .bind(id)
            .bind(ws)
            .execute(&state.any)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO process (id, goal, status, total_tokens, total_cost, \
             tool_invocations_used, created_at, updated_at, team_template_id) \
             VALUES (1, 'g', 'pending', 0, 0.0, 0, '2026-01-01', '2026-01-01', 1)",
        )
        .execute(&state.any)
        .await
        .unwrap();

        let (tokens, teams) = archive_rows(&state, 1, "2026-01-02 00:00:00").await.unwrap();
        assert_eq!(tokens, 2, "the already-revoked token is not counted again");
        assert_eq!(teams, 2);

        let live: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM api_tokens WHERE status != 'revoked' AND workspace_id = 2",
        )
        .fetch_one(&state.any)
        .await
        .unwrap();
        assert_eq!(live, 1, "the other tenant's token survives");
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM teamtemplate")
            .fetch_one(&state.any)
            .await
            .unwrap();
        assert_eq!(left, 1, "only the other tenant's team is left");
        // The FK would have refused the delete if the process still pointed at it.
        let orphaned: Option<i64> =
            sqlx::query_scalar("SELECT team_template_id FROM process WHERE id = 1")
                .fetch_one(&state.any)
                .await
                .unwrap();
        assert_eq!(orphaned, None);

        state.any.close().await;
        let _ = std::fs::remove_file(&path);
    }
}
