//! Workspace-scoped API token CRUD — `app/api_tokens/routes.py`, all eight
//! routes.
//!
//! **Master-key only.** A workspace token must never be able to mint or revoke
//! other tokens, so every route here rejects a principal that has a
//! `workspace_id`. That is the whole authorisation model of this domain: there
//! is no scope check, because no scope grants it.
//!
//! This closes the split that [`crate::auth`] has been living with since the
//! first domain moved: Rust *reads* `api_tokens` on every authenticated
//! request while Python owned every write. It also lets `auth.rs` finally write
//! `last_used_at` — see [`crate::auth::touch_last_used`], which was the
//! `ponytail:` note left there for whichever domain got here first.
//!
//! On the `sqlx::Any` pool: every query goes through `db::sql` and every id and
//! counter is selected as `CAST(… AS BIGINT)`, because a Postgres `integer` is
//! int4 where these fields are `i64`. [`TOKEN_COLUMNS`] is `pub` so
//! `tests/postgres_schema.rs` runs the real string against a real server rather
//! than a copy that drifts.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::FromRow;

use crate::auth::Principal;
use crate::error::{ApiError, PathId};
use crate::wire::{check_len, datetime_to_sql, iso_from_sql, parse_body, sql_now};
use crate::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    const BASE: &str = "/api/v1/workspaces/{workspace_id}/api-tokens";
    Router::new()
        // **Both spellings.** FastAPI answered the bare form with a `307` onto
        // the slashed one, so only the slashed one was registered here and the
        // bare one fell through to the proxy — which returned Python's redirect
        // verbatim. There is no proxy: the bare form became a 404, and any
        // caller that was relying on the redirect broke silently when the
        // interpreter went. Answering it directly is what `projects.rs` already
        // does, for the reason it gives there — a redirect through a hop that
        // no longer exists is not a contract worth reproducing.
        .route(BASE, get(list_tokens).post(create_token))
        .route(&format!("{BASE}/"), get(list_tokens).post(create_token))
        .route(&format!("{BASE}/{{token_id}}"), get(get_token).patch(update_token))
        .route(&format!("{BASE}/{{token_id}}/usage"), get(token_usage))
        .route(&format!("{BASE}/{{token_id}}/revoke"), post(revoke_token))
        .route(&format!("{BASE}/{{token_id}}/hold"), post(hold_token))
        .route(&format!("{BASE}/{{token_id}}/unhold"), post(unhold_token))
}

/// `_require_dashboard_caller`. A master-key caller (or any caller at all when
/// no master key is configured) has no `workspace_id` and passes.
fn require_dashboard_caller(principal: &Principal) -> Result<(), ApiError> {
    if principal.workspace_id.is_some() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "API tokens cannot be managed using an API token.",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rows and rendering
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct TokenRow {
    id: i64,
    workspace_id: i64,
    project_id: Option<i64>,
    name: String,
    prefix: String,
    scopes_json: Option<String>,
    status: String,
    rate_limit_per_minute: Option<i64>,
    expires_at: Option<String>,
    last_used_at: Option<String>,
    revoked_at: Option<String>,
    revoked_reason: Option<String>,
    held_reason: Option<String>,
    total_requests: i64,
    total_errors: i64,
    total_tokens: i64,
    total_cost: f64,
    created_at: String,
    updated_at: String,
}

pub const TOKEN_COLUMNS: &str = "CAST(id AS BIGINT) AS id, \
     CAST(workspace_id AS BIGINT) AS workspace_id, \
     CAST(project_id AS BIGINT) AS project_id, name, prefix, scopes_json, status, \
     CAST(rate_limit_per_minute AS BIGINT) AS rate_limit_per_minute, CAST(expires_at AS TEXT) AS expires_at, \
     CAST(last_used_at AS TEXT) AS last_used_at, CAST(revoked_at AS TEXT) AS revoked_at, \
     revoked_reason, held_reason, CAST(total_requests AS BIGINT) AS total_requests, \
     CAST(total_errors AS BIGINT) AS total_errors, \
     CAST(total_tokens AS BIGINT) AS total_tokens, total_cost, \
     CAST(created_at AS TEXT) AS created_at, CAST(updated_at AS TEXT) AS updated_at";

fn iso_opt(raw: &Option<String>) -> Value {
    match raw {
        Some(raw) => Value::String(iso_from_sql(raw)),
        None => Value::Null,
    }
}

impl TokenRow {
    /// `ApiToken.scopes` — `json.loads(scopes_json)`. A column that is not a
    /// JSON array reads as empty rather than 500ing, the same discipline every
    /// other JSON column in this crate follows.
    fn scopes(&self) -> Vec<Value> {
        self.scopes_json
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .and_then(|v| match v {
                Value::Array(a) => Some(a),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// `ApiTokenOut`, in pydantic's declaration order — which is the order the
    /// body renders in, and therefore contract.
    fn to_out(&self) -> Value {
        json!({
            "id": self.id,
            "workspace_id": self.workspace_id,
            "project_id": self.project_id,
            "name": self.name,
            "prefix": self.prefix,
            "scopes": self.scopes(),
            "status": self.status,
            "rate_limit_per_minute": self.rate_limit_per_minute,
            "expires_at": iso_opt(&self.expires_at),
            "last_used_at": iso_opt(&self.last_used_at),
            "revoked_at": iso_opt(&self.revoked_at),
            "revoked_reason": self.revoked_reason,
            "held_reason": self.held_reason,
            "total_requests": self.total_requests,
            "total_errors": self.total_errors,
            "total_tokens": self.total_tokens,
            "total_cost": self.total_cost,
            "created_at": iso_from_sql(&self.created_at),
            "updated_at": iso_from_sql(&self.updated_at),
        })
    }
}

async fn load_token(state: &AppState, token_id: i64) -> Result<Option<TokenRow>, ApiError> {
    Ok(sqlx::query_as(&crate::db::sql(
        &format!("SELECT {TOKEN_COLUMNS} FROM api_tokens WHERE id = ?"),
        state.backend,
    ))
        .bind(token_id)
        .fetch_optional(&state.any)
        .await?)
}

/// `_require_token`: `require_one` 404s a missing row, and a row belonging to
/// another workspace gets **the same 404** rather than a 403 — the token is not
/// this caller's to know about.
async fn require_token(
    state: &AppState,
    workspace_id: i64,
    token_id: i64,
) -> Result<TokenRow, ApiError> {
    let row = load_token(state, token_id).await?;
    match row {
        Some(row) if row.workspace_id == workspace_id => Ok(row),
        _ => Err(ApiError::not_found("API token not found")),
    }
}

/// `workspace_archive.require_active_workspace`. Missing *and* archived are the
/// same 404, and the name in the message is the caller-supplied default.
async fn require_active_workspace(state: &AppState, workspace_id: i64) -> Result<(), ApiError> {
    let archived: Option<Option<String>> =
        sqlx::query_scalar(&crate::db::sql(
            "SELECT CAST(archived_at AS TEXT) FROM workspace WHERE id = ?",
            state.backend,
        ))
            .bind(workspace_id)
            .fetch_optional(&state.any)
            .await?;
    match archived {
        Some(None) => Ok(()),
        _ => Err(ApiError::not_found("Workspace not found")),
    }
}

// ---------------------------------------------------------------------------
// Token generation — `api_tokens/token_service.py`
// ---------------------------------------------------------------------------

const TOKEN_PREFIX_DISPLAY_LEN: usize = 8;

/// `secrets.token_urlsafe(32)`: 32 random bytes, base64url, padding stripped —
/// 43 characters. Hand-rolled rather than pulling a base64 crate in for one
/// call site; the alphabet is the contract and it is six lines.
fn token_urlsafe(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        // One output character per 6 bits, minus the ones a short chunk would
        // have padded with `=` — which `rstrip(b"=")` removes anyway.
        for i in 0..chunk.len() + 1 {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 0x3f) as usize] as char);
        }
    }
    out
}

/// `generate_token()` → `(full, display_prefix, sha256_hex)`.
fn generate_token() -> (String, String, String) {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("OS entropy is available");
    let secret = token_urlsafe(&bytes);
    let full = format!("agp_live_{secret}");
    let prefix = format!("agp_live_{}", &secret[..TOKEN_PREFIX_DISPLAY_LEN]);
    let hash = crate::auth::hash_token(&full);
    (full, prefix, hash)
}

// ---------------------------------------------------------------------------
// Bodies
// ---------------------------------------------------------------------------

/// `list[str]`, rejecting a non-list and a non-string entry the way pydantic
/// does, entry by entry.
fn parse_scopes(errors: &mut Vec<Value>, value: Option<&Value>) -> Vec<Value> {
    let Some(value) = value else { return Vec::new() };
    let Some(items) = value.as_array() else {
        errors.push(ApiError::field_error("scopes", "list_type", "Input should be a valid list"));
        return Vec::new();
    };
    for (i, item) in items.iter().enumerate() {
        if !item.is_string() {
            // The index is an **integer** in pydantic's `loc`, not a string —
            // `field_error_at` only builds string segments, so this one is
            // assembled directly.
            errors.push(json!({
                "type": "string_type",
                "loc": ["body", "scopes", i],
                "msg": "Input should be a valid string",
            }));
        }
    }
    items.clone()
}

/// `int | None = Field(ge=1)`.
fn parse_rate_limit(errors: &mut Vec<Value>, value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    match value.as_i64() {
        None => {
            errors.push(ApiError::field_error(
                "rate_limit_per_minute",
                "int_parsing",
                "Input should be a valid integer, unable to parse string as an integer",
            ));
            None
        }
        Some(n) if n < 1 => {
            errors.push(ApiError::field_error(
                "rate_limit_per_minute",
                "greater_than_equal",
                "Input should be greater than or equal to 1",
            ));
            None
        }
        Some(n) => Some(n),
    }
}

/// `datetime | None`, stored as SQLAlchemy would store it — **the offset is
/// dropped, not applied** (see [`datetime_to_sql`]).
///
/// Pydantic accepts three things here and this accepts the same three: an ISO
/// string, `null`, and **a number, which is a unix timestamp**. That last one
/// is not a curiosity — a string of fewer than ten characters is parsed as a
/// timestamp too, so `"12345"` stores `1970-01-01 03:25:45` rather than
/// failing. Confirmed against the running Python server, not reasoned about.
fn parse_expires_at(errors: &mut Vec<Value>, value: Option<&Value>) -> Option<String> {
    let value = value?;
    if value.is_null() {
        return None;
    }

    // A number, or a numeric string short enough that speedate reads it as one.
    let seconds = match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) if s.len() < 10 && !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => {
            s.parse::<f64>().ok()
        }
        _ => None,
    };
    if let Some(seconds) = seconds {
        return chrono::DateTime::from_timestamp(seconds as i64, 0)
            .map(|at| crate::wire::sql_string(at.naive_utc()));
    }

    let Some(raw) = value.as_str() else {
        // A bool or a container is a different error entirely: pydantic never
        // gets as far as parsing one.
        errors.push(ApiError::field_error(
            "expires_at",
            "datetime_type",
            "Input should be a valid datetime",
        ));
        return None;
    };
    if let Some(_) = crate::wire::parse_naive(raw) {
        return Some(datetime_to_sql(raw));
    }
    errors.push(ApiError::field_error(
        "expires_at",
        "datetime_from_date_parsing",
        &format!("Input should be a valid datetime or date, {}", speedate_reason(raw)),
    ));
    None
}

/// The tail of pydantic's `datetime_from_date_parsing` message.
///
/// speedate's own wording, and the boundary is not where it looks: **anything
/// shorter than ten characters is "input is too short"** whatever it contains,
/// because a shorter string was already tried as a unix timestamp. Read off a
/// `python -c` table against the real validator rather than guessed.
///
/// ponytail: the two dominant classes only. Out-of-range components
/// ("month value is outside expected range of 1-12") fall into the third
/// message here — same `type`, same `loc`, same status, and this is the
/// `input`/`ctx` gap the whole migration already documents.
fn speedate_reason(raw: &str) -> &'static str {
    if raw.len() < 10 {
        "input is too short"
    } else if !raw.as_bytes()[..4].iter().all(u8::is_ascii_digit) {
        "invalid character in year"
    } else {
        "unexpected extra characters at the end of the input"
    }
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

async fn create_token(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(workspace_id): PathId<i64>,
    body: Bytes,
) -> Result<Response, ApiError> {
    require_dashboard_caller(&principal)?;
    let body = parse_body(&body)?;

    let mut errors = Vec::new();
    // The length check runs **only on an actual string**: pydantic reports one
    // failure per field, so a non-string `name` is `string_type` alone and not
    // also `string_too_short`.
    let name = match body.get("name").filter(|v| !v.is_null()) {
        None => {
            errors.push(ApiError::field_error("name", "missing", "Field required"));
            ""
        }
        Some(Value::String(s)) => {
            check_len(&mut errors, &["name"], Some(s.as_str()), 1, 256);
            s.as_str()
        }
        Some(_) => {
            errors.push(ApiError::field_error("name", "string_type", "Input should be a valid string"));
            ""
        }
    };
    let scopes = parse_scopes(&mut errors, body.get("scopes").filter(|v| !v.is_null()));
    let rate_limit = parse_rate_limit(&mut errors, body.get("rate_limit_per_minute"));
    let expires_at = parse_expires_at(&mut errors, body.get("expires_at"));
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    // Ordered as Python orders it: the caller check, then the workspace, then
    // the mint. An archived workspace must not consume a token id.
    require_active_workspace(&state, workspace_id).await?;

    let (full_token, prefix, token_hash) = generate_token();
    let now = sql_now();
    let id: i64 = sqlx::query_scalar(&crate::db::sql(
        "INSERT INTO api_tokens \
         (workspace_id, project_id, name, prefix, token_hash, scopes_json, status, \
          rate_limit_per_minute, expires_at, total_requests, total_errors, total_tokens, \
          total_cost, created_at, updated_at) \
         VALUES (?, NULL, ?, ?, ?, ?, 'active', ?, ?, 0, 0, 0, 0.0, ?, ?) RETURNING CAST(id AS BIGINT)",
        state.backend,
    ))
    .bind(workspace_id)
    .bind(name.trim())
    .bind(&prefix)
    .bind(&token_hash)
    .bind(serde_json::to_string(&Value::Array(scopes)).unwrap_or_else(|_| "[]".into()))
    .bind(rate_limit)
    .bind(&expires_at)
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.any)
    .await?;

    let row = require_token(&state, workspace_id, id).await?;
    let mut out = row.to_out();
    // `ApiTokenCreateOut(**out.model_dump(), token=...)` — the raw token last,
    // and this is the only time it is ever returned.
    out["token"] = json!(full_token);
    Ok((StatusCode::CREATED, Json(out)).into_response())
}

async fn list_tokens(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(workspace_id): PathId<i64>,
) -> Result<Response, ApiError> {
    require_dashboard_caller(&principal)?;
    require_active_workspace(&state, workspace_id).await?;
    let rows: Vec<TokenRow> = sqlx::query_as(&crate::db::sql(&format!(
        "SELECT {TOKEN_COLUMNS} FROM api_tokens WHERE workspace_id = ? ORDER BY id DESC"
    ), state.backend))
    .bind(workspace_id)
    .fetch_all(&state.any)
    .await?;
    let tokens: Vec<Value> = rows.iter().map(TokenRow::to_out).collect();
    Ok(Json(json!({ "tokens": tokens })).into_response())
}

async fn get_token(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId((workspace_id, token_id)): PathId<(i64, i64)>,
) -> Result<Response, ApiError> {
    require_dashboard_caller(&principal)?;
    // No workspace check here: Python's `get` does not run one either, so an
    // archived workspace's tokens are still readable one at a time.
    let row = require_token(&state, workspace_id, token_id).await?;
    Ok(Json(row.to_out()).into_response())
}

async fn update_token(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId((workspace_id, token_id)): PathId<(i64, i64)>,
    body: Bytes,
) -> Result<Response, ApiError> {
    require_dashboard_caller(&principal)?;
    let body = parse_body(&body)?;
    let row = require_token(&state, workspace_id, token_id).await?;

    // `model_fields_set` semantics: a key **present** in the body is applied,
    // including an explicit `null` for the two clearable columns. `name` and
    // `scopes` are the exception — Python guards them with `is not None`, so a
    // null there is accepted and then ignored.
    let mut errors = Vec::new();
    let name = match body.get("name") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            check_len(&mut errors, &["name"], Some(s.as_str()), 1, 256);
            Some(s.clone())
        }
        Some(_) => {
            errors.push(ApiError::field_error("name", "string_type", "Input should be a valid string"));
            None
        }
    };
    let scopes = body
        .get("scopes")
        .filter(|v| !v.is_null())
        .map(|v| parse_scopes(&mut errors, Some(v)));
    let rate_limit_set = body.get("rate_limit_per_minute").is_some();
    let rate_limit = parse_rate_limit(&mut errors, body.get("rate_limit_per_minute"));
    let expires_set = body.get("expires_at").is_some();
    let expires_at = parse_expires_at(&mut errors, body.get("expires_at"));
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    let now = sql_now();
    sqlx::query(&crate::db::sql(
        "UPDATE api_tokens SET name = ?, scopes_json = ?, rate_limit_per_minute = ?, \
         expires_at = ?, updated_at = ? WHERE id = ?",
        state.backend,
    ))
    .bind(name.map(|n| n.trim().to_string()).unwrap_or_else(|| row.name.clone()))
    .bind(
        scopes
            .map(|s| serde_json::to_string(&Value::Array(s)).unwrap_or_else(|_| "[]".into()))
            .unwrap_or_else(|| row.scopes_json.clone().unwrap_or_else(|| "[]".into())),
    )
    .bind(if rate_limit_set { rate_limit } else { row.rate_limit_per_minute })
    .bind(if expires_set { expires_at } else { row.expires_at.clone() })
    .bind(&now)
    .bind(token_id)
    .execute(&state.any)
    .await?;

    let row = require_token(&state, workspace_id, token_id).await?;
    Ok(Json(row.to_out()).into_response())
}

#[derive(Deserialize)]
struct UsageQuery {
    #[serde(default)]
    from_date: Option<String>,
    #[serde(default)]
    to_date: Option<String>,
}

#[derive(FromRow)]
struct UsageRow {
    usage_date: String,
    request_count: i64,
    error_count: i64,
    total_tokens: i64,
    total_cost: f64,
}

async fn token_usage(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId((workspace_id, token_id)): PathId<(i64, i64)>,
    Query(q): Query<UsageQuery>,
) -> Result<Response, ApiError> {
    require_dashboard_caller(&principal)?;
    require_token(&state, workspace_id, token_id).await?;

    // The bounds are compared as **text**, exactly as Python does — the column
    // is a `YYYY-MM-DD` string, so a malformed bound filters lexically rather
    // than failing. Empty strings are falsy in Python and add no clause.
    let mut sql = String::from(
        "SELECT usage_date, request_count, error_count, total_tokens, total_cost \
         FROM api_token_usage_daily WHERE token_id = ?",
    );
    let from = q.from_date.filter(|s| !s.is_empty());
    let to = q.to_date.filter(|s| !s.is_empty());
    if from.is_some() {
        sql.push_str(" AND usage_date >= ?");
    }
    if to.is_some() {
        sql.push_str(" AND usage_date <= ?");
    }
    sql.push_str(" ORDER BY usage_date ASC");

    // Bound to a local: the query borrows the rewritten string for as long as
    // the binds are added to it.
    let sql = crate::db::sql(&sql, state.backend).into_owned();
    let mut query = sqlx::query_as::<_, UsageRow>(&sql).bind(token_id);
    if let Some(from) = &from {
        query = query.bind(from);
    }
    if let Some(to) = &to {
        query = query.bind(to);
    }
    let rows = query.fetch_all(&state.any).await?;

    let daily: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "usage_date": r.usage_date,
                "request_count": r.request_count,
                "error_count": r.error_count,
                "total_tokens": r.total_tokens,
                "total_cost": r.total_cost,
            })
        })
        .collect();
    Ok(Json(json!({ "daily": daily })).into_response())
}

/// `ApiTokenActionBody.reason` — optional, 512 characters, and the body itself
/// is **required** on `/revoke` and `/hold` even though every field in it is
/// optional. `/unhold` declares no body at all.
fn parse_reason(body: &Value) -> Result<Option<String>, ApiError> {
    let mut errors = Vec::new();
    let reason = match body.get("reason") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            check_len(&mut errors, &["reason"], Some(s.as_str()), 0, 512);
            Some(s.clone())
        }
        Some(_) => {
            errors.push(ApiError::field_error("reason", "string_type", "Input should be a valid string"));
            None
        }
    };
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }
    Ok(reason)
}

async fn revoke_token(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId((workspace_id, token_id)): PathId<(i64, i64)>,
    body: Bytes,
) -> Result<Response, ApiError> {
    require_dashboard_caller(&principal)?;
    let reason = parse_reason(&parse_body(&body)?)?;
    require_token(&state, workspace_id, token_id).await?;

    // Irreversible, and it does not check the current status: revoking a
    // revoked token succeeds and refreshes `revoked_at`.
    let now = sql_now();
    sqlx::query(&crate::db::sql(
        "UPDATE api_tokens SET status = 'revoked', revoked_at = ?, revoked_reason = ?, \
         updated_at = ? WHERE id = ?",
        state.backend,
    ))
    .bind(&now)
    .bind(&reason)
    .bind(&now)
    .bind(token_id)
    .execute(&state.any)
    .await?;

    let row = require_token(&state, workspace_id, token_id).await?;
    Ok(Json(row.to_out()).into_response())
}

async fn hold_token(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId((workspace_id, token_id)): PathId<(i64, i64)>,
    body: Bytes,
) -> Result<Response, ApiError> {
    require_dashboard_caller(&principal)?;
    let reason = parse_reason(&parse_body(&body)?)?;
    let row = require_token(&state, workspace_id, token_id).await?;
    if row.status == "revoked" {
        return Err(ApiError::new(StatusCode::CONFLICT, "Cannot hold a revoked token"));
    }

    let now = sql_now();
    sqlx::query(&crate::db::sql(
        "UPDATE api_tokens SET status = 'held', held_reason = ?, updated_at = ? WHERE id = ?",
        state.backend,
    ))
        .bind(&reason)
        .bind(&now)
        .bind(token_id)
        .execute(&state.any)
        .await?;

    let row = require_token(&state, workspace_id, token_id).await?;
    Ok(Json(row.to_out()).into_response())
}

async fn unhold_token(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId((workspace_id, token_id)): PathId<(i64, i64)>,
) -> Result<Response, ApiError> {
    require_dashboard_caller(&principal)?;
    let row = require_token(&state, workspace_id, token_id).await?;
    if row.status != "held" {
        return Err(ApiError::new(StatusCode::CONFLICT, "Token is not on hold"));
    }

    let now = sql_now();
    sqlx::query(&crate::db::sql(
        "UPDATE api_tokens SET status = 'active', held_reason = NULL, updated_at = ? WHERE id = ?",
        state.backend,
    ))
    .bind(&now)
    .bind(token_id)
    .execute(&state.any)
    .await?;

    let row = require_token(&state, workspace_id, token_id).await?;
    Ok(Json(row.to_out()).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checked against `python -c "import secrets, base64; ..."` on the same
    /// bytes: 32 bytes in, 43 unpadded base64url characters out.
    #[test]
    fn token_urlsafe_matches_pythons_alphabet_and_length() {
        assert_eq!(token_urlsafe(&[0u8; 32]).len(), 43);
        assert_eq!(token_urlsafe(&[0u8; 32]), "A".repeat(43));
        assert_eq!(token_urlsafe(&[255u8; 32]), format!("{}{}", "_".repeat(42), "8"));
        // The two characters that make it *url*-safe rather than standard
        // base64: `-` and `_` where `+` and `/` would be.
        assert_eq!(token_urlsafe(&[251, 255, 254]), "-__-");
        assert!(!token_urlsafe(&[0u8; 32]).contains('='), "padding is stripped");
    }

    /// The public prefix is the first eight characters of the secret, so it is
    /// safe to store and display while the rest is only ever hashed.
    #[test]
    fn a_minted_token_carries_its_own_prefix_and_hashes_to_the_stored_value() {
        let (full, prefix, hash) = generate_token();
        assert!(full.starts_with("agp_live_"));
        assert_eq!(full.len(), "agp_live_".len() + 43);
        assert!(full.starts_with(&prefix));
        assert_eq!(prefix.len(), "agp_live_".len() + TOKEN_PREFIX_DISPLAY_LEN);
        assert_eq!(hash, crate::auth::hash_token(&full));
        // Two mints never collide.
        assert_ne!(generate_token().0, full);
    }

    #[test]
    fn scopes_degrade_to_empty_rather_than_failing_the_read() {
        let row = |scopes_json: Option<&str>| TokenRow {
            id: 1,
            workspace_id: 1,
            project_id: None,
            name: "n".into(),
            prefix: "agp_live_x".into(),
            scopes_json: scopes_json.map(str::to_string),
            status: "active".into(),
            rate_limit_per_minute: None,
            expires_at: None,
            last_used_at: None,
            revoked_at: None,
            revoked_reason: None,
            held_reason: None,
            total_requests: 0,
            total_errors: 0,
            total_tokens: 0,
            total_cost: 0.0,
            created_at: "2026-08-07 10:00:00.000000".into(),
            updated_at: "2026-08-07 10:00:00.000000".into(),
        };
        assert_eq!(row(Some(r#"["chat:write"]"#)).scopes(), vec![json!("chat:write")]);
        assert!(row(Some("{}")).scopes().is_empty());
        assert!(row(Some("not json")).scopes().is_empty());
        assert!(row(None).scopes().is_empty());

        // Null timestamps stay null; the two that are set render without a `Z`,
        // because the columns are naive.
        let out = row(Some("[]")).to_out();
        assert_eq!(out["last_used_at"], Value::Null);
        assert_eq!(out["created_at"], "2026-08-07T10:00:00");
        // Field order is pydantic's declaration order, and it is contract.
        let keys: Vec<&str> = out.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys[..6], ["id", "workspace_id", "project_id", "name", "prefix", "scopes"]);
        assert_eq!(keys.last(), Some(&"updated_at"));
    }

    #[test]
    fn only_the_master_key_may_manage_tokens() {
        assert!(require_dashboard_caller(&Principal::unrestricted()).is_ok());
        let scoped =
            Principal { workspace_id: Some(1), token_id: Some(2), scopes: vec!["*".into()], ..Principal::unrestricted() };
        let err = require_dashboard_caller(&scoped).unwrap_err();
        assert_eq!(err.status, StatusCode::FORBIDDEN);
        // Even a `*` scope does not help: no scope grants this.
        assert_eq!(err.message, "API tokens cannot be managed using an API token.");
    }
}
