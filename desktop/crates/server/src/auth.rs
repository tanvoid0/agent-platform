//! Bearer auth, ported from `app/api_tokens/auth.py`, then tied to a user
//! ([ADR 0014](../../../../docs/adr/0014-user-owned-data-local-and-cloud.md)).
//!
//! Tiers:
//!
//! - **master key** — operator. May carry the desktop machine `user_id` so
//!   writes are stamped, but `scoped_user_id` is `None` (sees every tenant).
//! - **`agp_…` workspace token** — one workspace; `user_id` is that
//!   workspace's owner. Cross-tenant reads 404, not 401.
//! - **user session JWT** — one Portal / cloud user. Not the master key.
//! - **open local** — no master key on loopback. Other apps send no
//!   `Authorization` header; the caller is the OS-username user, not
//!   `user_id = None`.
//!
//! Failures name the actual reason (`AUTH_REQUIRED`, `TOKEN_EXPIRED`, …) so
//! another app hitting a hosted URL can tell "this server wants a token" from
//! "this token is dead". `/health` repeats the short form without a credential.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header::AUTHORIZATION, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::{NaiveDateTime, Utc};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use subtle::ConstantTimeEq;

use crate::AppState;

const TOKEN_PREFIX_MARKER: &str = "agp_";
/// `app/llm_proxy/core/errors.py::ERROR_TYPE` — external callers branch on the
/// envelope, so it has to be this string and not something more accurate.
const ERROR_TYPE: &str = "llm_proxy_error";

/// How this caller authenticated. Tenant isolation keys off this, not off
/// `user_id == None` — a local machine user *has* a `user_id` and is still
/// the operator of this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    OpenLocal,
    MasterKey,
    WorkspaceToken,
    UserSession,
}

impl AuthMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthMode::OpenLocal => "open_local",
            AuthMode::MasterKey => "master_key",
            AuthMode::WorkspaceToken => "workspace_token",
            AuthMode::UserSession => "user_session",
        }
    }
}

/// Resolved caller.
#[derive(Debug, Clone)]
pub struct Principal {
    pub workspace_id: Option<i64>,
    pub token_id: Option<i64>,
    pub scopes: Vec<String>,
    pub user_id: Option<i64>,
    pub email: Option<String>,
    pub entitlement: Option<String>,
    pub is_admin: bool,
    pub client: Option<String>,
    pub mode: AuthMode,
}

/// Handlers take `Principal` directly; the middleware below has already put one
/// in the extensions for every path it guards.
impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Principal {
    type Rejection = crate::error::ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        parts.extensions.get::<Principal>().cloned().ok_or_else(|| {
            // Only reachable by mounting a route outside the guarded prefix.
            crate::error::ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "An unexpected error occurred.",
            )
        })
    }
}

/// A caller resolved *at the handler* rather than by the layer.
///
/// `require_token` guards `/api/v1/*`, because that is the prefix `app/main.py`
/// mounts with `_api_deps`. The LLM proxy's `/v1/*` routes are mounted without
/// it and authenticate per route — two of them not at all — so a handler there
/// extracts this instead of reading the extensions.
pub struct ProxyPrincipal(pub Principal);

impl axum::extract::FromRequestParts<Arc<AppState>> for ProxyPrincipal {
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let header = parts.headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok());
        let mut principal = resolve(state, header).await?;
        if principal.user_id.is_none() && header.is_none() {
            if let Some(email) = parts
                .headers
                .get("x-agent-platform-user")
                .and_then(|v| v.to_str().ok())
            {
                principal = crate::accounts::principal_from_dev_header(state, email).await?;
            }
        }
        principal.client = crate::accounts::client_from_headers(&parts.headers);
        Ok(ProxyPrincipal(principal))
    }
}

impl Principal {
    pub fn unrestricted() -> Self {
        Self {
            workspace_id: None,
            token_id: None,
            scopes: vec!["*".into()],
            user_id: None,
            email: None,
            entitlement: None,
            is_admin: false,
            client: None,
            mode: AuthMode::MasterKey,
        }
    }

    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == "*" || s == scope)
    }

    /// `app/api_tokens/auth.py::require_scope` — a 403 with a code the caller
    /// can tell apart from "this token is held".
    pub fn require_scope(&self, scope: &'static str) -> Result<(), crate::error::ApiError> {
        if self.has_scope(scope) {
            return Ok(());
        }
        Err(crate::error::ApiError::coded(
            StatusCode::FORBIDDEN,
            "INSUFFICIENT_SCOPE",
            format!("Token lacks required scope '{scope}'."),
        ))
    }

    /// Operator of this process: master key, or the local machine user on an
    /// open loopback daemon. Workspace tokens and cloud JWTs are tenants.
    pub fn is_operator(&self) -> bool {
        self.workspace_id.is_none()
            && matches!(self.mode, AuthMode::OpenLocal | AuthMode::MasterKey)
    }

    /// Tenant isolation. `None` is the cloud/master operator (sees every row).
    /// Local and JWT callers get their `user_id` so one account cannot read
    /// another. Workspace tokens also carry the workspace owner's id.
    pub fn scoped_user_id(&self) -> Option<i64> {
        if self.mode == AuthMode::MasterKey {
            return None;
        }
        self.user_id
    }

    /// An operator action, not a tenant one. A workspace token is a valid Bearer
    /// credential and gets past auth, so every route that manages the platform
    /// rather than the tenant's own data has to say so itself.
    ///
    /// `denial` is the whole message because the two callers explain the same
    /// rule from different ends — "this endpoint wants the master key" for the
    /// `.env` surface, "workspaces are not yours to manage" for tenancy — and
    /// the wording is the only part of this that was ever different.
    ///
    /// A local machine user is an operator with a `user_id`; a cloud JWT is
    /// not. The check is the mode, not `user_id.is_some()`.
    pub fn require_master_key(&self, denial: &'static str) -> Result<(), crate::error::ApiError> {
        if self.is_operator() {
            return Ok(());
        }
        Err(crate::error::ApiError::new(StatusCode::FORBIDDEN, denial))
    }
}

/// Mirrors `app/api_tokens/exceptions.py`: an HTTP status plus a machine-readable
/// `code` callers branch on.
#[derive(Debug)]
pub struct AuthError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub token_prefix: Option<String>,
}

impl AuthError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into(), token_prefix: None }
    }

    fn with_prefix(mut self, prefix: Option<String>) -> Self {
        self.token_prefix = prefix;
        self
    }

    fn invalid(message: &str) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "TOKEN_INVALID", message)
    }

    pub fn invalid_pub(message: &str) -> Self {
        Self::invalid(message)
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let mut extra = json!({
            "auth": {
                "required": true,
                "header": "Authorization: Bearer <session JWT | agp_ workspace token | master key>",
                "health": "/health",
                "status": "/api/v1/system/status",
                "me": "/api/v1/me",
                "accounts": "/accounts",
                "refresh": "/accounts/api/v1/auth/refresh",
            }
        });
        if let Some(prefix) = self.token_prefix {
            extra["token_prefix"] = json!(prefix);
        }
        let mut err = json!({
            "message": self.message,
            "type": ERROR_TYPE,
            "code": self.code,
            "extra": extra,
        });
        if let Some(request_id) = crate::request_id::current() {
            err["request_id"] = json!(request_id);
        }
        let www = match self.code {
            "AUTH_REQUIRED" => {
                "Bearer realm=\"agent-platform\", error=\"invalid_request\", error_description=\"missing access token\""
            }
            "TOKEN_EXPIRED" => {
                "Bearer realm=\"agent-platform\", error=\"invalid_token\", error_description=\"token expired\""
            }
            _ => {
                "Bearer realm=\"agent-platform\", error=\"invalid_token\", error_description=\"invalid access token\""
            }
        };
        let mut response = (self.status, axum::Json(json!({ "error": err }))).into_response();
        if let Ok(value) = HeaderValue::from_str(www) {
            response.headers_mut().insert("www-authenticate", value);
        }
        response
    }
}

/// `expires_at` and `last_used_at` are `String`, not `NaiveDateTime`: the `Any`
/// driver refuses a timestamp column on *both* backends, so the query casts
/// them to text and [`crate::wire::parse_naive`] turns them back where they are
/// actually compared. See the note on [`crate::db`].
#[derive(FromRow)]
struct TokenRow {
    id: i64,
    workspace_id: i64,
    prefix: String,
    scopes_json: String,
    status: String,
    held_reason: Option<String>,
    rate_limit_per_minute: Option<i64>,
    expires_at: Option<String>,
    last_used_at: Option<String>,
}

/// Guards exactly the prefix `app/main.py` mounts with `_api_deps`. `/v1/*` (the
/// LLM proxy) authenticates itself and `/health` is open, so both fall through.
pub async fn require_token(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, AuthError> {
    if !req.uri().path().starts_with("/api/v1/") {
        return Ok(next.run(req).await);
    }

    let header = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    let principal = resolve(&state, header.as_deref()).await?;
    let mut principal = principal;
    principal.client = crate::accounts::client_from_headers(req.headers());
    req.extensions_mut().insert(principal);
    Ok(next.run(req).await)
}

fn with_machine_stamp(mut principal: Principal, state: &AppState) -> Principal {
    if principal.user_id.is_none() {
        if let Some(user) = crate::identity::machine_user(state) {
            principal.user_id = Some(user.id);
            principal.email = Some(user.email);
        }
    }
    principal
}

const AUTH_REQUIRED_MSG: &str = "\
This Agent Platform API requires authentication. Send Authorization: Bearer \
<session JWT | agp_ workspace token | master key>. GET /health (no token) \
reports auth.required. 401 AUTH_REQUIRED means the header was missing; \
TOKEN_EXPIRED means POST /accounts/api/v1/auth/refresh; TOKEN_INVALID means \
the secret is not from this server.";

const TOKEN_INVALID_MSG: &str = "\
The Bearer token was not recognized. Use a current session JWT, an agp_ \
workspace token issued by this server, or the master key. A leftover token \
from another Agent Platform install fails this way.";

pub async fn resolve(state: &AppState, authorization: Option<&str>) -> Result<Principal, AuthError> {
    let Some(expected) = state.master_key.as_deref() else {
        if let Some(user) = crate::identity::machine_user(state) {
            return Ok(crate::identity::principal_from_machine(&user));
        }
        return Ok(Principal::unrestricted());
    };

    let raw = authorization
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .ok_or_else(|| {
            AuthError::new(StatusCode::UNAUTHORIZED, "AUTH_REQUIRED", AUTH_REQUIRED_MSG)
        })?;

    if raw.starts_with(TOKEN_PREFIX_MARKER) {
        return resolve_workspace_token(state, raw).await;
    }

    if ct_eq(raw, expected) {
        return Ok(with_machine_stamp(Principal::unrestricted(), state));
    }

    if raw.matches('.').count() == 2 {
        return crate::accounts::principal_from_jwt(state, raw).await;
    }

    Err(AuthError::invalid(TOKEN_INVALID_MSG))
}

async fn resolve_workspace_token(state: &AppState, raw: &str) -> Result<Principal, AuthError> {
    let row: Option<TokenRow> = sqlx::query_as(&crate::db::sql(
        "SELECT CAST(id AS BIGINT) AS id, CAST(workspace_id AS BIGINT) AS workspace_id, \
                prefix, scopes_json, status, held_reason, \
                CAST(rate_limit_per_minute AS BIGINT) AS rate_limit_per_minute, \
                CAST(expires_at AS TEXT) AS expires_at, \
                CAST(last_used_at AS TEXT) AS last_used_at \
         FROM api_tokens WHERE token_hash = ?",
        state.backend,
    ))
    .bind(hash_token(raw))
    .fetch_optional(&state.any)
    .await
    .map_err(db_error)?;

    let row = row.ok_or_else(|| AuthError::invalid(TOKEN_INVALID_MSG))?;
    let prefix = Some(row.prefix.clone());

    match row.status.as_str() {
        "revoked" => {
            return Err(
                AuthError::new(StatusCode::UNAUTHORIZED, "TOKEN_REVOKED", "This token has been revoked.")
                    .with_prefix(prefix),
            )
        }
        "held" => {
            let msg = row
                .held_reason
                .clone()
                .unwrap_or_else(|| "This token is temporarily on hold.".into());
            return Err(AuthError::new(StatusCode::FORBIDDEN, "TOKEN_HELD", msg).with_prefix(prefix));
        }
        _ => {}
    }

    // Stored naive-UTC by `time_utils.utc_now_naive`, so compare naive-UTC.
    let expires_at = row.expires_at.as_deref().and_then(crate::wire::parse_naive);
    if expires_at.is_some_and(|at| at < Utc::now().naive_utc()) {
        return Err(
            AuthError::new(StatusCode::UNAUTHORIZED, "TOKEN_EXPIRED", "This token has expired.")
                .with_prefix(prefix),
        );
    }

    let archived: Option<Option<String>> = sqlx::query_scalar(&crate::db::sql(
        "SELECT CAST(archived_at AS TEXT) FROM workspace WHERE id = ?",
        state.backend,
    ))
            .bind(row.workspace_id)
            .fetch_optional(&state.any)
            .await
            .map_err(db_error)?;

    if !matches!(archived, Some(None)) {
        return Err(AuthError::new(
            StatusCode::UNAUTHORIZED,
            "TOKEN_REVOKED",
            "This token's workspace has been archived.",
        )
        .with_prefix(prefix));
    }

    check_and_increment(state, row.id, row.rate_limit_per_minute).map_err(|e| e.with_prefix(prefix))?;

    touch_last_used(state, row.id, row.last_used_at.as_deref().and_then(crate::wire::parse_naive))
        .await;

    // Separate from the token query so a test database that predates
    // `workspace.user_id` still authenticates; a missing column is "no owner",
    // not "this token is bad".
    let owner_user_id: Option<i64> = sqlx::query_scalar(&crate::db::sql(
        "SELECT CAST(user_id AS BIGINT) FROM workspace WHERE id = ?",
        state.backend,
    ))
    .bind(row.workspace_id)
    .fetch_optional(&state.any)
    .await
    .ok()
    .flatten();

    Ok(Principal {
        workspace_id: Some(row.workspace_id),
        token_id: Some(row.id),
        scopes: serde_json::from_str(&row.scopes_json).unwrap_or_default(),
        user_id: owner_user_id,
        email: None,
        entitlement: None,
        is_admin: false,
        client: None,
        mode: AuthMode::WorkspaceToken,
    })
}

/// `_LAST_USED_THROTTLE_SECONDS`. One write per token per minute, not per
/// request — the column exists to answer "is anyone still using this?", and
/// that question does not need second resolution.
const LAST_USED_THROTTLE_SECONDS: i64 = 60;

/// `auth.py`'s throttled `last_used_at` update.
///
/// **This was missing until `api_tokens` moved, and it was a real defect, not a
/// cosmetic one.** Rust never wrote the column and Python only writes it for
/// requests that reach Python — so once a domain migrated, a token whose
/// traffic Rust answers stopped advancing it entirely. A coder-only or
/// processes-only token read as never used in `GET /api-tokens`, which is
/// exactly the signal an operator revokes on.
///
/// A failure here is logged and swallowed: this is bookkeeping on the auth path,
/// and failing a valid caller's request over it would be the worse bug.
async fn touch_last_used(state: &AppState, token_id: i64, last_used_at: Option<NaiveDateTime>) {
    let now = Utc::now().naive_utc();
    let due = match last_used_at {
        None => true,
        Some(at) => (now - at).num_seconds() > LAST_USED_THROTTLE_SECONDS,
    };
    if !due {
        return;
    }
    let result = sqlx::query(&crate::db::sql(
        "UPDATE api_tokens SET last_used_at = ? WHERE id = ?",
        state.backend,
    ))
        .bind(crate::wire::sql_string(now))
        .bind(token_id)
        .execute(&state.any)
        .await;
    if let Err(e) = result {
        logd!("last_used_at update failed for token {token_id}: {e}");
    }
}

/// Fixed-window counter, same shape as `app/api_tokens/rate_limiter.py`.
fn check_and_increment(state: &AppState, token_id: i64, limit: Option<i64>) -> Result<(), AuthError> {
    let Some(limit) = limit.filter(|l| *l > 0) else {
        return Ok(());
    };
    let minute = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 60)
        .unwrap_or(0);

    let count = {
        let mut windows = state.windows.lock().unwrap();
        // Entries are only ever added, and a token that stops calling leaves
        // one behind forever. Harmless at any realistic token count, which is
        // why this is a size trigger and not a timer: a sweep of stale minutes
        // costs less than the map that made it necessary. Nothing here is
        // per-request work in the normal case.
        if windows.len() > 1024 {
            windows.retain(|_, (m, _)| *m == minute);
        }
        let entry = windows.entry(token_id).or_insert((minute, 0));
        if entry.0 != minute {
            *entry = (minute, 0);
        }
        entry.1 += 1;
        entry.1
    };

    if u64::from(count) > limit as u64 {
        return Err(AuthError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "RATE_LIMIT_EXCEEDED",
            format!("Rate limit exceeded ({limit} requests/min)."),
        ));
    }
    Ok(())
}

pub fn hash_token(raw: &str) -> String {
    let digest = Sha256::digest(raw.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

/// A database that is missing or mid-migration must not read as "bad credential" —
/// that would send a caller chasing their token instead of the server.
fn db_error(e: sqlx::Error) -> AuthError {
    logd!("token lookup failed: {e}");
    AuthError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "TOKEN_ERROR",
        "Could not verify the API token.",
    )
}
