//! One user row for every install — OS username locally, magic-link in the cloud.
//!
//! Local loopback stays open ([ADR 0013](../../../../docs/adr/0013-desktop-local-open-cloud-account.md));
//! the unauthenticated caller is this machine user rather than `user_id = None`.
//! Cloud JWT callers use the same `users` / `workspace.user_id` columns, so
//! list/get shapes do not fork. See [ADR 0014](../../../../docs/adr/0014-user-owned-data-local-and-cloud.md).

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};
use sqlx::FromRow;

use crate::auth::{AuthMode, Principal};
use crate::error::ApiError;
use crate::wire::sql_now;
use crate::{db, env_opt, AppState};

#[derive(Debug, Clone)]
pub struct MachineUser {
    pub id: i64,
    pub email: String,
    pub username: String,
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/me", get(me))
}

/// What another app can read with no token: is auth required, and why `/api/v1`
/// would suddenly 401.
pub fn public_auth_info(state: &AppState) -> Value {
    if state.master_key.is_some() {
        json!({
            "required": true,
            "mode": "bearer",
            "hint": "This Agent Platform API requires Authorization: Bearer \
                     <session JWT | agp_ workspace token | master key>. \
                     401 AUTH_REQUIRED means no token was sent; TOKEN_EXPIRED means \
                     refresh at POST /accounts/api/v1/auth/refresh; TOKEN_INVALID \
                     means the secret is not from this server.",
        })
    } else {
        json!({
            "required": false,
            "mode": "open_local",
            "hint": "This is a local Agent Platform daemon. No Authorization header \
                     is required on loopback. Data is owned by the machine user \
                     (GET /api/v1/me).",
        })
    }
}

pub fn machine_user(state: &AppState) -> Option<MachineUser> {
    state.machine_user.lock().ok().and_then(|g| g.clone())
}

/// SQLite desktop: create the OS user, their workspace, and backfill ownerless
/// rows. Postgres (cloud) skips this — users arrive through magic-link.
pub async fn bootstrap(state: &AppState) -> Result<(), sqlx::Error> {
    if state.backend != db::Backend::Sqlite {
        return Ok(());
    }
    let user = ensure_machine_user(state).await?;
    *state.machine_user.lock().unwrap() = Some(user.clone());
    if let Err(e) = ensure_user_workspace(state, user.id, &user.username, "local").await {
        logd!("[identity] personal workspace for local:{}: {e:?}", user.username);
        return Err(sqlx::Error::Protocol("could not create the local user's workspace".into()));
    }
    backfill_owner(state, user.id).await?;
    logd!("[identity] machine user {} ({})", user.username, user.email);
    Ok(())
}

pub fn os_username() -> String {
    if let Some(raw) = env_opt("AGENT_PLATFORM_LOCAL_USERNAME") {
        return sanitize_username(&raw);
    }
    let raw = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "user".into());
    sanitize_username(&raw)
}

pub fn sanitize_username(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.trim().chars() {
        if out.len() >= 64 {
            break;
        }
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
            out.push(c.to_ascii_lowercase());
        }
    }
    if out.is_empty() {
        "user".into()
    } else {
        out
    }
}

fn local_email(username: &str) -> String {
    format!("local:{username}@localhost")
}

async fn ensure_machine_user(state: &AppState) -> Result<MachineUser, sqlx::Error> {
    let username = os_username();
    let email = local_email(&username);
    if let Some(existing) = find_local(state, &email).await? {
        return Ok(existing);
    }
    sqlx::query(&db::sql(
        "INSERT INTO users (email, username, kind, is_admin, entitlement, trial_ends_at, \
         created_at, updated_at) VALUES (?, ?, 'local', 0, 'comp', NULL, ?, ?)",
        state.backend,
    ))
    .bind(&email)
    .bind(&username)
    .bind(sql_now())
    .bind(sql_now())
    .execute(&state.any)
    .await?;
    find_local(state, &email).await?.ok_or_else(|| {
        sqlx::Error::Protocol("inserted local user but could not reload it".into())
    })
}

async fn find_local(state: &AppState, email: &str) -> Result<Option<MachineUser>, sqlx::Error> {
    let row: Option<(i64, String, Option<String>)> = sqlx::query_as(&db::sql(
        "SELECT CAST(id AS BIGINT), email, username FROM users WHERE email = ?",
        state.backend,
    ))
    .bind(email)
    .fetch_optional(&state.any)
    .await?;
    Ok(row.map(|(id, email, username)| MachineUser {
        id,
        username: username.unwrap_or_else(|| os_username()),
        email,
    }))
}

/// The caller's personal workspace, created on first use so `/api/v1/me` and
/// project-create-without-id have somewhere to go on both local and cloud.
pub async fn ensure_user_workspace(
    state: &AppState,
    user_id: i64,
    username: &str,
    kind: &str,
) -> Result<i64, ApiError> {
    let existing: Option<i64> = sqlx::query_scalar(&db::sql(
        "SELECT CAST(id AS BIGINT) FROM workspace \
         WHERE user_id = ? AND archived_at IS NULL ORDER BY id ASC LIMIT 1",
        state.backend,
    ))
    .bind(user_id)
    .fetch_optional(&state.any)
    .await?;
    if let Some(id) = existing {
        return Ok(id);
    }

    let mut slug = if kind == "local" {
        format!("local-{username}")
    } else {
        format!("u{user_id}")
    };
    if slug_taken(state, &slug).await? {
        slug = format!("{slug}-{user_id}");
    }
    let name = if kind == "local" {
        username.to_string()
    } else {
        format!("Workspace")
    };
    let now = sql_now();
    let id: i64 = sqlx::query_scalar(&db::sql(
        "INSERT INTO workspace (name, slug, description, archived_at, user_id, created_at, updated_at) \
         VALUES (?, ?, NULL, NULL, ?, ?, ?) RETURNING CAST(id AS BIGINT)",
        state.backend,
    ))
    .bind(&name)
    .bind(&slug)
    .bind(user_id)
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.any)
    .await?;
    Ok(id)
}

async fn slug_taken(state: &AppState, slug: &str) -> Result<bool, ApiError> {
    let found: Option<i64> = sqlx::query_scalar(&db::sql(
        "SELECT CAST(id AS BIGINT) FROM workspace WHERE slug = ?",
        state.backend,
    ))
    .bind(slug)
    .fetch_optional(&state.any)
    .await?;
    Ok(found.is_some())
}

async fn backfill_owner(state: &AppState, user_id: i64) -> Result<(), sqlx::Error> {
    for table in [
        "workspace",
        "coder_chat_threads",
        "media_jobs",
        "workflows",
        "action_sets",
        "search_history",
    ] {
        let raw = format!("UPDATE {table} SET user_id = ? WHERE user_id IS NULL");
        let sql = db::sql(&raw, state.backend);
        sqlx::query(&sql).bind(user_id).execute(&state.any).await?;
    }
    Ok(())
}

pub fn principal_from_machine(user: &MachineUser) -> Principal {
    Principal {
        workspace_id: None,
        token_id: None,
        scopes: vec!["*".into()],
        user_id: Some(user.id),
        email: Some(user.email.clone()),
        entitlement: Some("comp".into()),
        is_admin: false,
        client: None,
        mode: AuthMode::OpenLocal,
    }
}

/// Stamp a new row with whoever this caller is, falling back to the machine
/// user on a keyed desktop daemon that authenticated as master.
pub fn stamp_user_id(state: &AppState, principal: &Principal) -> Option<i64> {
    principal.user_id.or_else(|| machine_user(state).map(|u| u.id))
}

/// 404 — never 401 — if this workspace is not the caller's.
pub async fn assert_workspace_visible(
    state: &AppState,
    principal: &Principal,
    workspace_id: i64,
) -> Result<(), ApiError> {
    #[derive(FromRow)]
    struct Row {
        user_id: Option<i64>,
        archived_at: Option<String>,
    }
    let row: Option<Row> = sqlx::query_as(&db::sql(
        "SELECT CAST(user_id AS BIGINT) AS user_id, CAST(archived_at AS TEXT) AS archived_at \
         FROM workspace WHERE id = ?",
        state.backend,
    ))
    .bind(workspace_id)
    .fetch_optional(&state.any)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::not_found("Workspace not found"));
    };
    if row.archived_at.is_some() {
        return Err(ApiError::not_found("Workspace not found"));
    }
    if let Some(ws) = principal.workspace_id {
        if ws != workspace_id {
            return Err(ApiError::not_found("Workspace not found"));
        }
        return Ok(());
    }
    if let Some(uid) = principal.scoped_user_id() {
        if row.user_id != Some(uid) {
            return Err(ApiError::not_found("Workspace not found"));
        }
    }
    Ok(())
}

/// A row on an orphan table (coder, media, …). Master key sees NULL owners;
/// everyone else only sees their own id.
pub fn assert_user_row(principal: &Principal, row_user_id: Option<i64>) -> Result<(), ApiError> {
    match principal.scoped_user_id() {
        None => Ok(()),
        Some(uid) if row_user_id == Some(uid) => Ok(()),
        Some(_) => Err(ApiError::not_found("Not found")),
    }
}

async fn me(State(state): State<Arc<AppState>>, principal: Principal) -> Result<Json<Value>, ApiError> {
    Ok(Json(me_json(&state, &principal).await?))
}

pub async fn me_json(state: &AppState, principal: &Principal) -> Result<Value, ApiError> {
    let user = if let Some(id) = principal.user_id {
        crate::accounts::load_user(state, id)
            .await?
            .map(|row| crate::accounts::user_json(&row))
    } else {
        None
    };
    let workspace = match (principal.workspace_id, principal.user_id) {
        (Some(ws), _) => workspace_json(state, ws).await?,
        (None, Some(uid)) => {
            let username = user
                .as_ref()
                .and_then(|v| v["username"].as_str().map(str::to_string))
                .unwrap_or_else(os_username);
            let kind = user
                .as_ref()
                .and_then(|v| v["kind"].as_str())
                .unwrap_or("local");
            let id = ensure_user_workspace(state, uid, &username, kind).await?;
            workspace_json(state, id).await?
        }
        (None, None) => None,
    };
    Ok(json!({
        "user": user,
        "workspace": workspace,
        "auth": {
            "required": state.master_key.is_some(),
            "mode": principal.mode.as_str(),
        },
    }))
}

async fn workspace_json(state: &AppState, id: i64) -> Result<Option<Value>, ApiError> {
    #[derive(FromRow)]
    struct Ws {
        id: i64,
        name: String,
        slug: String,
        user_id: Option<i64>,
    }
    let row: Option<Ws> = sqlx::query_as(&db::sql(
        "SELECT CAST(id AS BIGINT) AS id, name, slug, CAST(user_id AS BIGINT) AS user_id \
         FROM workspace WHERE id = ? AND archived_at IS NULL",
        state.backend,
    ))
    .bind(id)
    .fetch_optional(&state.any)
    .await?;
    Ok(row.map(|r| {
        json!({
            "id": r.id,
            "name": r.name,
            "slug": r.slug,
            "user_id": r.user_id,
        })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;

    #[test]
    fn usernames_are_safe_unique_keys() {
        assert_eq!(sanitize_username("Tan Vo"), "tanvo");
        assert_eq!(sanitize_username("Alice.Smith"), "alice.smith");
        assert_eq!(sanitize_username("!!!"), "user");
        assert_eq!(sanitize_username(""), "user");
    }

    #[tokio::test]
    async fn sqlite_bootstrap_registers_the_os_user_and_backfills() {
        let path = std::env::temp_dir().join(format!(
            "agp-identity-{}-{}.db",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_file(&path);
        std::env::set_var("AGENT_PLATFORM_LOCAL_USERNAME", "Test.User");
        let state = AppState::new(&path, None);
        crate::db::ensure_schema(&state.any).await.unwrap();
        sqlx::query("INSERT INTO workspace (name, slug) VALUES ('old', 'old')")
            .execute(&state.any)
            .await
            .unwrap();

        bootstrap(&state).await.unwrap();
        let user = machine_user(&state).expect("machine user");
        assert_eq!(user.username, "test.user");
        assert_eq!(user.email, "local:test.user@localhost");

        let owner: i64 = sqlx::query_scalar("SELECT user_id FROM workspace WHERE slug = 'old'")
            .fetch_one(&state.any)
            .await
            .unwrap();
        assert_eq!(owner, user.id);

        let slug: String = sqlx::query_scalar("SELECT slug FROM workspace WHERE user_id = ? AND slug LIKE 'local-%' LIMIT 1")
            .bind(user.id)
            .fetch_one(&state.any)
            .await
            .unwrap();
        assert_eq!(slug, "local-test.user");

        std::env::remove_var("AGENT_PLATFORM_LOCAL_USERNAME");
        let _ = std::fs::remove_file(&path);
    }
}
