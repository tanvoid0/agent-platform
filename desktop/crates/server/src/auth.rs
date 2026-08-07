//! Bearer auth, ported from `app/api_tokens/auth.py`.
//!
//! Three tiers, same as the Python server (see the tenancy contract in
//! `app/tests/test_workspace_tenancy.py`):
//!
//! - **master key** — operator, `workspace_id == None`, unrestricted
//! - **`agp_…` workspace token** — one tenant; cross-tenant reads must 404, not 401
//! - **`X-Agent-Platform-Client`** — a caller-supplied namespace, *not* a security
//!   boundary, so it is not checked here (route handlers read it when they care)
//!
//! With no master key configured, auth is fully open. That is the Python server's
//! documented dev convenience, and diverging from it here would break local runs.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header::AUTHORIZATION, StatusCode};
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

/// Resolved caller. `workspace_id == None` is the unrestricted (master key or
/// auth-disabled) caller.
#[derive(Debug, Clone)]
pub struct Principal {
    pub workspace_id: Option<i64>,
    pub token_id: Option<i64>,
    pub scopes: Vec<String>,
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
        resolve(state, header).await.map(ProxyPrincipal)
    }
}

impl Principal {
    pub fn unrestricted() -> Self {
        Self { workspace_id: None, token_id: None, scopes: vec!["*".into()] }
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
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into(), token_prefix: None }
    }

    fn with_prefix(mut self, prefix: Option<String>) -> Self {
        self.token_prefix = prefix;
        self
    }

    fn invalid(message: &str) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "TOKEN_INVALID", message)
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let mut err = json!({
            "message": self.message,
            "type": ERROR_TYPE,
            "code": self.code,
        });
        if let Some(request_id) = crate::request_id::current() {
            err["request_id"] = json!(request_id);
        }
        if let Some(prefix) = self.token_prefix {
            err["extra"] = json!({ "token_prefix": prefix });
        }
        (self.status, axum::Json(json!({ "error": err }))).into_response()
    }
}

#[derive(FromRow)]
struct TokenRow {
    id: i64,
    workspace_id: i64,
    prefix: String,
    scopes_json: String,
    status: String,
    held_reason: Option<String>,
    rate_limit_per_minute: Option<i64>,
    expires_at: Option<NaiveDateTime>,
    last_used_at: Option<NaiveDateTime>,
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
    req.extensions_mut().insert(principal);
    Ok(next.run(req).await)
}

pub async fn resolve(state: &AppState, authorization: Option<&str>) -> Result<Principal, AuthError> {
    let Some(expected) = state.master_key.as_deref() else {
        return Ok(Principal::unrestricted());
    };

    let raw = authorization
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .ok_or_else(|| {
            AuthError::invalid("Missing or invalid Authorization (expected Bearer token)")
        })?;

    if raw.starts_with(TOKEN_PREFIX_MARKER) {
        return resolve_workspace_token(state, raw).await;
    }

    if !ct_eq(raw, expected) {
        return Err(AuthError::invalid("Invalid API key"));
    }
    Ok(Principal::unrestricted())
}

async fn resolve_workspace_token(state: &AppState, raw: &str) -> Result<Principal, AuthError> {
    let row: Option<TokenRow> = sqlx::query_as(
        "SELECT id, workspace_id, prefix, scopes_json, status, held_reason, \
                rate_limit_per_minute, expires_at, last_used_at \
         FROM api_tokens WHERE token_hash = ?",
    )
    .bind(hash_token(raw))
    .fetch_optional(&state.pool)
    .await
    .map_err(db_error)?;

    let row = row.ok_or_else(|| AuthError::invalid("Invalid API token"))?;
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
    if row.expires_at.is_some_and(|at| at < Utc::now().naive_utc()) {
        return Err(
            AuthError::new(StatusCode::UNAUTHORIZED, "TOKEN_EXPIRED", "This token has expired.")
                .with_prefix(prefix),
        );
    }

    let archived: Option<Option<NaiveDateTime>> =
        sqlx::query_scalar("SELECT archived_at FROM workspace WHERE id = ?")
            .bind(row.workspace_id)
            .fetch_optional(&state.pool)
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

    touch_last_used(state, row.id, row.last_used_at).await;

    Ok(Principal {
        workspace_id: Some(row.workspace_id),
        token_id: Some(row.id),
        scopes: serde_json::from_str(&row.scopes_json).unwrap_or_default(),
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
    let result = sqlx::query("UPDATE api_tokens SET last_used_at = ? WHERE id = ?")
        .bind(crate::wire::sql_string(now))
        .bind(token_id)
        .execute(&state.pool)
        .await;
    if let Err(e) = result {
        eprintln!("[agent-platformd] last_used_at update failed for token {token_id}: {e}");
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
    eprintln!("[agent-platformd] token lookup failed: {e}");
    AuthError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "TOKEN_ERROR",
        "Could not verify the API token.",
    )
}
