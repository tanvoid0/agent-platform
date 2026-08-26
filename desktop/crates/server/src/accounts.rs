//! Portal accounts: magic-link login, entitlements, usage, admin.
//!
//! `/accounts/api/v1/*` is outside `require_token` (that layer is `/api/v1/*`).
//! Store apps and this page authenticate with a user JWT. Master / `agp_` skip
//! billing so operator scripts keep working.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Duration as ChronoDuration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;

use crate::auth::{hash_token, Principal};
use crate::billing;
use crate::error::ApiError;
use crate::wire::sql_now;
use crate::{env_opt, AppState};

const ACCESS_MINUTES: i64 = 15;
const REFRESH_DAYS: i64 = 30;
const MAGIC_MINUTES: i64 = 15;
const MAGIC_COPY: &str = "If that email exists, we sent a link.";
/// Public app id the iced desktop sends on hosted `/v1` (ADR 0013).
pub const DESKTOP_APP_ID: &str = "agent-platform-desktop";
/// Path to `cloud.session.json` — desktop spawn sets this, the daemon reads it.
pub const SESSION_ENV: &str = "AGENT_PLATFORM_CLOUD_SESSION";

const DISPOSABLE: &[&str] = &[
    "mailinator.com", "guerrillamail.com", "guerrillamail.de", "sharklasers.com", "grr.la",
    "tempmail.com", "temp-mail.org", "throwawaymail.com", "yopmail.com", "10minutemail.com",
    "10minutemail.net", "trashmail.com", "getnada.com", "dispostable.com", "maildrop.cc",
    "fakeinbox.com", "emailondeck.com", "moakt.com", "discard.email", "mailnesia.com",
    "inboxkitten.com", "tempail.com", "tempr.email", "trash-mail.com", "mailcatch.com",
    "mytemp.email", "tmpmail.org", "tmpeml.com",
];

#[derive(Debug, Clone, FromRow)]
pub struct UserRow {
    pub id: i64,
    pub email: String,
    pub is_admin: i64,
    pub entitlement: String,
    pub trial_ends_at: Option<String>,
    pub billing_region: Option<String>,
    pub stripe_customer_id: Option<String>,
    pub stripe_subscription_id: Option<String>,
    pub stripe_price_id: Option<String>,
    pub comp_reason: Option<String>,
    pub comp_expires_at: Option<String>,
    pub username: Option<String>,
    pub kind: String,
}

pub const USER_COLS: &str = "CAST(id AS BIGINT) AS id, email, CAST(is_admin AS BIGINT) AS is_admin, \
     entitlement, CAST(trial_ends_at AS TEXT) AS trial_ends_at, \
     billing_region, stripe_customer_id, stripe_subscription_id, stripe_price_id, \
     comp_reason, CAST(comp_expires_at AS TEXT) AS comp_expires_at, \
     username, kind";

pub fn api_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/accounts/api/v1/auth/magic-link", post(magic_link))
        .route("/accounts/api/v1/auth/verify", post(verify).get(verify_get))
        .route("/accounts/api/v1/auth/refresh", post(refresh))
        .route("/accounts/api/v1/auth/logout", post(logout))
        .route("/accounts/api/v1/me", get(me))
        .route("/accounts/api/v1/me/usage", get(me_usage))
        .route("/accounts/api/v1/billing/quote", post(billing_quote))
        .route("/accounts/api/v1/billing/checkout", post(crate::accounts_stripe::billing_checkout))
        .route("/accounts/api/v1/billing/portal", post(crate::accounts_stripe::billing_portal))
        .route("/accounts/api/v1/billing/webhook", post(crate::accounts_stripe::stripe_webhook))
        .route("/accounts/api/v1/admin/users", get(admin_users))
        .route("/accounts/api/v1/admin/grant-comp", post(admin_grant_comp))
        .route("/accounts/api/v1/admin/set-entitlement", post(admin_set_entitlement))
        .route("/accounts/api/v1/admin/revoke-sessions", post(admin_revoke))
}

pub fn static_router() -> Router<Arc<AppState>> {
    // The page is one file compiled into the binary, so a cloud image needs no
    // node toolchain and no build step. A `/accounts/*` deep link renders it and
    // the page routes on the hash; `/accounts/api/*` that reached here matched
    // no API route, so it answers the 404 envelope rather than HTML.
    Router::new()
        .route("/accounts", get(embedded_index))
        .route("/accounts/", get(embedded_index))
        .route("/accounts/{*path}", get(accounts_path))
}

async fn accounts_path(Path(path): Path<String>) -> Response {
    if path.starts_with("api/") {
        return crate::error::ApiError::not_found(format!("No route for GET /accounts/{path}"))
            .into_response();
    }
    embedded_index().await.into_response()
}

async fn embedded_index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"))],
        Html(include_str!("../../../../accounts/index.html")),
    )
}

pub fn payment_required(reason: impl Into<String>) -> ApiError {
    ApiError::coded(StatusCode::PAYMENT_REQUIRED, "PAYMENT_REQUIRED", reason)
        .with_extra(json!({ "subscribe": true, "accounts": "/accounts" }))
}

pub async fn require_ai_entitlement(
    state: &AppState,
    principal: &Principal,
    client: Option<&str>,
) -> Result<(), ApiError> {
    let Some(user_id) = principal.user_id else {
        return Ok(());
    };
    if principal.mode != crate::auth::AuthMode::UserSession {
        return Ok(());
    }
    require_allowlisted_client(client)?;
    let row = load_user(state, user_id)
        .await?
        .ok_or_else(|| payment_required("Account not found."))?;
    if !ai_allowed(&row) {
        return Err(payment_required(entitlement_message(&row)));
    }
    if row.entitlement == "trial" && trial_over_quota(state, user_id).await? {
        return Err(payment_required("Trial quota reached. Subscribe to keep using AI Access."));
    }
    check_user_rate(state, user_id, env_u64("AGENT_PLATFORM_USER_RPM", 60))?;
    Ok(())
}

fn require_allowlisted_client(client: Option<&str>) -> Result<(), ApiError> {
    let Some(raw) = client.map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(ApiError::coded(
            StatusCode::UNAUTHORIZED,
            "APP_ID_REQUIRED",
            "Missing X-Agent-Platform-Client (public app id).",
        ));
    };
    if client_allowed(raw) {
        return Ok(());
    }
    Err(ApiError::coded(
        StatusCode::UNAUTHORIZED,
        "APP_ID_UNKNOWN",
        "This app is not allowlisted for AI Access.",
    ))
}

pub fn client_allowed(client: &str) -> bool {
    let c = client.trim();
    if c.to_ascii_lowercase().starts_with("android-") {
        return true;
    }
    env_opt("AGENT_PLATFORM_APP_ALLOWLIST")
        .unwrap_or_else(|| "portal-desktop,portal-equalizer,accounts,agent-platform-desktop".into())
        .split(',')
        .map(str::trim)
        .any(|a| !a.is_empty() && a.eq_ignore_ascii_case(c))
}

fn ai_allowed(row: &UserRow) -> bool {
    match row.entitlement.as_str() {
        "paid" => true,
        "comp" => !expired(row.comp_expires_at.as_deref()),
        "trial" => !expired(row.trial_ends_at.as_deref()),
        _ => false,
    }
}

fn entitlement_message(row: &UserRow) -> String {
    match row.entitlement.as_str() {
        "blocked" => "AI Access is not active. Subscribe to continue.".into(),
        "trial" => "Trial ended. Subscribe to continue using AI Access.".into(),
        "comp" => "Complimentary access has ended.".into(),
        _ => "AI Access is not active.".into(),
    }
}

pub fn expired(raw: Option<&str>) -> bool {
    match raw.and_then(crate::wire::parse_naive) {
        Some(at) => at < Utc::now().naive_utc(),
        None => false,
    }
}

async fn trial_over_quota(state: &AppState, user_id: i64) -> Result<bool, ApiError> {
    let max_req = env_u64("AGENT_PLATFORM_TRIAL_MAX_REQUESTS", 100) as i64;
    let max_tok = env_u64("AGENT_PLATFORM_TRIAL_MAX_TOKENS", 200_000) as i64;
    let row: Option<(i64, i64)> = sqlx::query_as(&crate::db::sql(
        "SELECT COALESCE(SUM(CAST(request_count AS BIGINT)), 0), \
                COALESCE(SUM(CAST(total_tokens AS BIGINT)), 0) \
         FROM user_usage_daily WHERE user_id = ?",
        state.backend,
    ))
    .bind(user_id)
    .fetch_optional(&state.any)
    .await?;
    let (reqs, toks) = row.unwrap_or((0, 0));
    Ok(reqs >= max_req || toks >= max_tok)
}

fn check_user_rate(state: &AppState, user_id: i64, limit: u64) -> Result<(), ApiError> {
    if limit == 0 {
        return Ok(());
    }
    let count = bump_i64(&state.user_windows, user_id);
    if count > limit {
        return Err(ApiError::coded(
            StatusCode::TOO_MANY_REQUESTS,
            "RATE_LIMIT_EXCEEDED",
            format!("Rate limit exceeded ({limit} requests/min)."),
        ));
    }
    Ok(())
}

fn bump_i64(map: &Mutex<HashMap<i64, (u64, u32)>>, key: i64) -> u64 {
    let minute = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() / 60).unwrap_or(0);
    let mut windows = map.lock().unwrap();
    if windows.len() > 4096 {
        windows.retain(|_, (m, _)| *m == minute);
    }
    let entry = windows.entry(key).or_insert((minute, 0));
    if entry.0 != minute {
        *entry = (minute, 0);
    }
    entry.1 += 1;
    u64::from(entry.1)
}

pub fn check_ip_rate(state: &AppState, ip: &str, limit: u64) -> Result<(), ApiError> {
    if limit == 0 || ip.is_empty() {
        return Ok(());
    }
    let minute = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() / 60).unwrap_or(0);
    let count = {
        let mut windows = state.ip_windows.lock().unwrap();
        if windows.len() > 8192 {
            windows.retain(|_, (m, _)| *m == minute);
        }
        let entry = windows.entry(ip.to_string()).or_insert((minute, 0));
        if entry.0 != minute {
            *entry = (minute, 0);
        }
        entry.1 += 1;
        entry.1
    };
    if u64::from(count) > limit {
        return Err(ApiError::coded(
            StatusCode::TOO_MANY_REQUESTS,
            "RATE_LIMIT_EXCEEDED",
            format!("Too many requests from this address ({limit}/min)."),
        ));
    }
    Ok(())
}

pub fn client_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-agent-platform-client")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(256).collect())
}

pub fn ip_from_headers(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown")
        .to_string()
}

pub async fn record_user_usage(state: &AppState, user_id: Option<i64>, tokens: i64, is_error: bool) {
    let Some(user_id) = user_id else { return };
    let errors = i64::from(is_error);
    let today = Utc::now().naive_utc().format("%Y-%m-%d").to_string();
    let updated = sqlx::query(&crate::db::sql(
        "UPDATE user_usage_daily \
         SET request_count = request_count + 1, error_count = error_count + ?, \
             total_tokens = total_tokens + ? \
         WHERE user_id = ? AND usage_date = ?",
        state.backend,
    ))
    .bind(errors)
    .bind(tokens)
    .bind(user_id)
    .bind(&today)
    .execute(&state.any)
    .await;

    match updated {
        Ok(result) if result.rows_affected() == 0 => {
            let _ = sqlx::query(&crate::db::sql(
                "INSERT INTO user_usage_daily \
                 (user_id, usage_date, request_count, error_count, total_tokens) \
                 VALUES (?, ?, 1, ?, ?)",
                state.backend,
            ))
            .bind(user_id)
            .bind(&today)
            .bind(errors)
            .bind(tokens)
            .execute(&state.any)
            .await;
        }
        Err(e) => logd!("user usage write failed for {user_id}: {e}"),
        _ => {}
    }
}

pub async fn unauth_v1_ip_limit(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let path = req.uri().path().to_string();
    if path.starts_with("/v1/") && path != "/v1/health" && path != "/v1/health/readiness" {
        let has_auth = req.headers().get(header::AUTHORIZATION).is_some();
        if !has_auth {
            check_ip_rate(&state, &ip_from_headers(req.headers()), env_u64("AGENT_PLATFORM_V1_IP_RPM", 30))?;
        }
    }
    Ok(next.run(req).await)
}

#[derive(Deserialize)]
struct MagicBody {
    email: String,
    redirect_uri: Option<String>,
}

async fn magic_link(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<MagicBody>,
) -> Result<Json<Value>, ApiError> {
    check_ip_rate(&state, &ip_from_headers(&headers), env_u64("AGENT_PLATFORM_MAGIC_LINK_IP_RPM", 5))?;
    let email = normalize_email(&body.email)?;
    if is_disposable(&email) {
        return Ok(Json(json!({ "ok": true, "message": MAGIC_COPY })));
    }
    let redirect = match body.redirect_uri.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => Some(validate_loopback_redirect(raw)?),
        None => None,
    };
    let raw = random_hex(32);
    let expires = Utc::now().naive_utc() + ChronoDuration::minutes(MAGIC_MINUTES);
    sqlx::query(&crate::db::sql(
        "INSERT INTO magic_links (email, token_hash, expires_at, used, created_at, redirect_uri) \
         VALUES (?, ?, ?, 0, ?, ?)",
        state.backend,
    ))
    .bind(&email)
    .bind(hash_token(&raw))
    .bind(crate::wire::sql_string(expires))
    .bind(sql_now())
    .bind(&redirect)
    .execute(&state.any)
    .await?;
    let page = match &redirect {
        Some(_) => format!("{}/accounts/api/v1/auth/verify?token={}", public_base(), raw),
        None => format!("{}/accounts/#/verify?token={}", public_base(), raw),
    };
    logd!("[accounts] magic link for {email}: {page}");
    Ok(Json(json!({ "ok": true, "message": MAGIC_COPY })))
}

#[derive(Deserialize)]
struct TokenBody {
    token: Option<String>,
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct TokenQuery { token: Option<String> }

async fn verify_get(
    State(state): State<Arc<AppState>>,
    Query(q): Query<TokenQuery>,
) -> Result<Response, ApiError> {
    let token = q.token.ok_or_else(|| ApiError::bad_request("Missing token."))?;
    let (body, redirect) = complete_verify(&state, &token).await?;
    if let Some(redirect) = redirect {
        let mut url = reqwest::Url::parse(&redirect).map_err(|_| {
            ApiError::bad_request("Stored redirect_uri is not a URL.")
        })?;
        {
            let access = body["access_token"].as_str().unwrap_or_default();
            let refresh = body["refresh_token"].as_str().unwrap_or_default();
            url.query_pairs_mut()
                .append_pair("access_token", access)
                .append_pair("refresh_token", refresh);
        }
        return Ok(Redirect::to(url.as_str()).into_response());
    }
    Ok(Json(body).into_response())
}

async fn verify(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TokenBody>,
) -> Result<Json<Value>, ApiError> {
    let token = body.token.ok_or_else(|| ApiError::bad_request("Missing token."))?;
    let (session, _) = complete_verify(&state, &token).await?;
    Ok(Json(session))
}

async fn complete_verify(state: &AppState, raw: &str) -> Result<(Value, Option<String>), ApiError> {
    let row: Option<(i64, String, String, i64, Option<String>)> = sqlx::query_as(&crate::db::sql(
        "SELECT CAST(id AS BIGINT), email, CAST(expires_at AS TEXT), CAST(used AS BIGINT), \
                redirect_uri \
         FROM magic_links WHERE token_hash = ?",
        state.backend,
    ))
    .bind(hash_token(raw))
    .fetch_optional(&state.any)
    .await?;
    let Some((id, email, expires, used, redirect)) = row else {
        return Err(ApiError::coded(StatusCode::UNAUTHORIZED, "LINK_INVALID", "This link is invalid."));
    };
    if used != 0 {
        return Err(ApiError::coded(StatusCode::UNAUTHORIZED, "LINK_USED", "This link was already used."));
    }
    if expired(Some(&expires)) {
        return Err(ApiError::coded(StatusCode::UNAUTHORIZED, "LINK_EXPIRED", "This link has expired."));
    }
    sqlx::query(&crate::db::sql("UPDATE magic_links SET used = 1 WHERE id = ?", state.backend))
        .bind(id)
        .execute(&state.any)
        .await?;
    let user = upsert_user_on_login(state, &email).await?;
    let Json(session) = issue_session(state, &user).await?;
    Ok((session, redirect.filter(|s| !s.is_empty())))
}

async fn refresh(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TokenBody>,
) -> Result<Json<Value>, ApiError> {
    let raw = body.refresh_token.ok_or_else(|| ApiError::bad_request("Missing refresh_token."))?;
    let row: Option<(i64, i64, String, i64)> = sqlx::query_as(&crate::db::sql(
        "SELECT CAST(id AS BIGINT), CAST(user_id AS BIGINT), CAST(expires_at AS TEXT), \
                CAST(revoked AS BIGINT) FROM sessions WHERE refresh_token_hash = ?",
        state.backend,
    ))
    .bind(hash_token(&raw))
    .fetch_optional(&state.any)
    .await?;
    let Some((sid, user_id, expires, revoked)) = row else {
        return Err(ApiError::coded(StatusCode::UNAUTHORIZED, "SESSION_INVALID", "Refresh token invalid."));
    };
    if revoked != 0 || expired(Some(&expires)) {
        return Err(ApiError::coded(StatusCode::UNAUTHORIZED, "SESSION_REVOKED", "Refresh token expired."));
    }
    sqlx::query(&crate::db::sql("UPDATE sessions SET revoked = 1 WHERE id = ?", state.backend))
        .bind(sid)
        .execute(&state.any)
        .await?;
    let user = load_user(&state, user_id).await?.ok_or_else(|| {
        ApiError::coded(StatusCode::UNAUTHORIZED, "SESSION_INVALID", "Account missing.")
    })?;
    issue_session(&state, &user).await
}

async fn logout(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TokenBody>,
) -> Result<Json<Value>, ApiError> {
    if let Some(raw) = body.refresh_token {
        sqlx::query(&crate::db::sql(
            "UPDATE sessions SET revoked = 1 WHERE refresh_token_hash = ?",
            state.backend,
        ))
        .bind(hash_token(&raw))
        .execute(&state.any)
        .await?;
    }
    Ok(Json(json!({ "ok": true })))
}

async fn me(State(state): State<Arc<AppState>>, user: AccountUser) -> Result<Json<Value>, ApiError> {
    let row = load_user(&state, user.id).await?.ok_or_else(|| ApiError::not_found("Account not found."))?;
    Ok(Json(user_json(&row)))
}

async fn me_usage(State(state): State<Arc<AppState>>, user: AccountUser) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({ "days": usage_days(&state, user.id).await? })))
}

#[derive(Deserialize)]
pub struct QuoteBody {
    pub card_country: Option<String>,
    pub billing_country: Option<String>,
}

async fn billing_quote(_user: AccountUser, Json(body): Json<QuoteBody>) -> Result<Json<Value>, ApiError> {
    let q = billing::quote(body.card_country.as_deref(), body.billing_country.as_deref());
    Ok(Json(json!({
        "region": q.region,
        "currency": q.currency,
        "amount_minor": q.amount_minor,
        "stripe_price_id": q.stripe_price_id,
    })))
}

async fn admin_users(State(state): State<Arc<AppState>>, _admin: AdminUser) -> Result<Json<Value>, ApiError> {
    let rows: Vec<UserRow> = sqlx::query_as(&crate::db::sql(
        &format!("SELECT {USER_COLS} FROM users ORDER BY id DESC LIMIT 200"),
        state.backend,
    ))
    .fetch_all(&state.any)
    .await?;
    let mut out = Vec::new();
    for row in rows {
        let mut v = user_json(&row);
        if let Ok(days) = usage_days(&state, row.id).await {
            v["usage"] = json!(days);
        }
        out.push(v);
    }
    Ok(Json(json!({ "users": out })))
}

#[derive(Deserialize)]
struct GrantBody {
    email: String,
    reason: Option<String>,
    expires: Option<String>,
}

async fn admin_grant_comp(
    State(state): State<Arc<AppState>>,
    _admin: AdminUser,
    Json(body): Json<GrantBody>,
) -> Result<Json<Value>, ApiError> {
    let email = normalize_email(&body.email)?;
    let row = grant_comp(&state, &email, body.reason.as_deref().unwrap_or("admin"), body.expires.as_deref()).await?;
    Ok(Json(user_json(&row)))
}

#[derive(Deserialize)]
struct SetEntBody {
    email: String,
    entitlement: String,
    card_country: Option<String>,
    billing_country: Option<String>,
}

async fn admin_set_entitlement(
    State(state): State<Arc<AppState>>,
    _admin: AdminUser,
    Json(body): Json<SetEntBody>,
) -> Result<Json<Value>, ApiError> {
    let email = normalize_email(&body.email)?;
    let row = set_entitlement(
        &state,
        &email,
        &body.entitlement,
        body.card_country.as_deref(),
        body.billing_country.as_deref(),
    )
    .await?;
    Ok(Json(user_json(&row)))
}

#[derive(Deserialize)]
struct EmailBody { email: String }

async fn admin_revoke(
    State(state): State<Arc<AppState>>,
    _admin: AdminUser,
    Json(body): Json<EmailBody>,
) -> Result<Json<Value>, ApiError> {
    let email = normalize_email(&body.email)?;
    let n = revoke_sessions(&state, &email).await?;
    Ok(Json(json!({ "revoked": n })))
}

pub async fn grant_comp(
    state: &AppState,
    email: &str,
    reason: &str,
    expires: Option<&str>,
) -> Result<UserRow, ApiError> {
    let user = upsert_user_shell(state, email).await?;
    let exp = expires.map(crate::wire::datetime_to_sql).filter(|s| !s.is_empty());
    sqlx::query(&crate::db::sql(
        "UPDATE users SET entitlement = 'comp', comp_reason = ?, comp_expires_at = ?, updated_at = ? WHERE id = ?",
        state.backend,
    ))
    .bind(reason)
    .bind(exp.as_deref())
    .bind(sql_now())
    .bind(user.id)
    .execute(&state.any)
    .await?;
    load_user(state, user.id).await?.ok_or_else(|| ApiError::not_found("Account not found."))
}

pub async fn set_entitlement(
    state: &AppState,
    email: &str,
    entitlement: &str,
    card_country: Option<&str>,
    billing_country: Option<&str>,
) -> Result<UserRow, ApiError> {
    let ent = entitlement.trim().to_ascii_lowercase();
    if !matches!(ent.as_str(), "trial" | "paid" | "comp" | "blocked") {
        return Err(ApiError::bad_request("entitlement must be trial|paid|comp|blocked"));
    }
    let user = upsert_user_shell(state, email).await?;
    let mut region = user.billing_region.clone();
    let mut price = user.stripe_price_id.clone();
    if ent == "paid" && region.is_none() {
        let q = billing::quote(card_country, billing_country);
        region = Some(q.region);
        price = q.stripe_price_id;
    }
    if ent == "trial" && user.trial_ends_at.is_none() {
        let ends = Utc::now().naive_utc() + ChronoDuration::days(trial_days());
        sqlx::query(&crate::db::sql(
            "UPDATE users SET trial_ends_at = ? WHERE id = ?",
            state.backend,
        ))
        .bind(crate::wire::sql_string(ends))
        .bind(user.id)
        .execute(&state.any)
        .await?;
    }
    sqlx::query(&crate::db::sql(
        "UPDATE users SET entitlement = ?, billing_region = ?, stripe_price_id = ?, updated_at = ? WHERE id = ?",
        state.backend,
    ))
    .bind(&ent)
    .bind(region.as_deref())
    .bind(price.as_deref())
    .bind(sql_now())
    .bind(user.id)
    .execute(&state.any)
    .await?;
    load_user(state, user.id).await?.ok_or_else(|| ApiError::not_found("Account not found."))
}

pub async fn revoke_sessions(state: &AppState, email: &str) -> Result<u64, ApiError> {
    let Some(user) = find_user_by_email(state, email).await? else {
        return Ok(0);
    };
    let res = sqlx::query(&crate::db::sql(
        "UPDATE sessions SET revoked = 1 WHERE user_id = ? AND revoked = 0",
        state.backend,
    ))
    .bind(user.id)
    .execute(&state.any)
    .await?;
    Ok(res.rows_affected())
}

pub async fn seed_admin_emails(state: &AppState) {
    let Some(raw) = env_opt("AGENT_PLATFORM_ADMIN_EMAILS") else { return };
    for part in raw.split(',') {
        let Ok(email) = normalize_email(part) else { continue };
        if let Err(e) = grant_comp(state, &email, "ADMIN_EMAILS", None).await {
            logd!("[accounts] seed admin {email}: {e:?}");
            continue;
        }
        let _ = sqlx::query(&crate::db::sql(
            "UPDATE users SET is_admin = 1, updated_at = ? WHERE email = ?",
            state.backend,
        ))
        .bind(sql_now())
        .bind(&email)
        .execute(&state.any)
        .await;
        logd!("[accounts] seeded admin {email}");
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    email: String,
    ent: String,
    adm: bool,
    exp: usize,
    iat: usize,
}

pub async fn principal_from_jwt(state: &AppState, token: &str) -> Result<Principal, crate::auth::AuthError> {
    let secret = jwt_secret().ok_or_else(|| {
        crate::auth::AuthError::new(
            axum::http::StatusCode::UNAUTHORIZED,
            "TOKEN_INVALID",
            "This server has no JWT secret configured, so a session token cannot be verified. \
             Use an agp_ workspace token or the master key, or set AGENT_PLATFORM_JWT_SECRET.",
        )
    })?;
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|e| match *e.kind() {
        jsonwebtoken::errors::ErrorKind::ExpiredSignature => crate::auth::AuthError::new(
            axum::http::StatusCode::UNAUTHORIZED,
            "TOKEN_EXPIRED",
            "The session JWT has expired. POST /accounts/api/v1/auth/refresh with the refresh \
             token, or sign in again at /accounts.",
        ),
        jsonwebtoken::errors::ErrorKind::InvalidSignature
        | jsonwebtoken::errors::ErrorKind::InvalidToken => crate::auth::AuthError::new(
            axum::http::StatusCode::UNAUTHORIZED,
            "TOKEN_INVALID",
            "The session JWT could not be verified. It may belong to another Agent Platform server.",
        ),
        _ => crate::auth::AuthError::new(
            axum::http::StatusCode::UNAUTHORIZED,
            "TOKEN_INVALID",
            "The session JWT is not valid.",
        ),
    })?;
    let user_id: i64 = data.claims.sub.parse().map_err(|_| {
        crate::auth::AuthError::new(
            axum::http::StatusCode::UNAUTHORIZED,
            "TOKEN_INVALID",
            "The session JWT is missing a user id. Sign in again at /accounts.",
        )
    })?;
    let row = load_user(state, user_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| {
            crate::auth::AuthError::new(
                axum::http::StatusCode::UNAUTHORIZED,
                "TOKEN_INVALID",
                "This session's account was not found. Sign in again at /accounts.",
            )
        })?;
    Ok(user_principal(&row))
}

pub async fn principal_from_dev_header(state: &AppState, email: &str) -> Result<Principal, crate::auth::AuthError> {
    if jwt_secret().is_some() || !loopback_bind() {
        return Err(crate::auth::AuthError::invalid_pub("Invalid API key"));
    }
    let Ok(email) = normalize_email(email) else {
        return Err(crate::auth::AuthError::invalid_pub("Invalid API key"));
    };
    let row = find_user_by_email(state, &email)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| crate::auth::AuthError::invalid_pub("Invalid API key"))?;
    Ok(user_principal(&row))
}

fn user_principal(row: &UserRow) -> Principal {
    Principal {
        workspace_id: None,
        token_id: None,
        scopes: vec!["*".into(), "chat:write".into()],
        user_id: Some(row.id),
        email: Some(row.email.clone()),
        entitlement: Some(row.entitlement.clone()),
        is_admin: row.is_admin != 0,
        client: None,
        mode: crate::auth::AuthMode::UserSession,
    }
}

fn sign_access(user: &UserRow) -> Result<String, ApiError> {
    let secret = jwt_secret().ok_or_else(|| {
        ApiError::coded(
            StatusCode::SERVICE_UNAVAILABLE,
            "JWT_UNCONFIGURED",
            "AGENT_PLATFORM_JWT_SECRET is not set.",
        )
    })?;
    let now = Utc::now().timestamp() as usize;
    let claims = Claims {
        sub: user.id.to_string(),
        email: user.email.clone(),
        ent: user.entitlement.clone(),
        adm: user.is_admin != 0,
        iat: now,
        exp: now + (ACCESS_MINUTES * 60) as usize,
    };
    encode(&Header::default(), &claims, &EncodingKey::from_secret(secret.as_bytes()))
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

pub fn jwt_secret() -> Option<String> {
    env_opt("AGENT_PLATFORM_JWT_SECRET")
}

pub fn loopback_bind() -> bool {
    let host = env_opt("AGENT_PLATFORM_HOST").unwrap_or_else(|| "127.0.0.1".into());
    crate::is_loopback_host(&host)
}

pub struct AccountUser {
    pub id: i64,
    pub is_admin: bool,
}

pub struct AdminUser(pub AccountUser);

impl axum::extract::FromRequestParts<Arc<AppState>> for AccountUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let header = parts.headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok());
        let principal = crate::auth::resolve(state, header)
            .await
            .map_err(|e| ApiError::coded(e.status, e.code, e.message))?;
        let id = principal.user_id.ok_or_else(|| {
            ApiError::coded(StatusCode::UNAUTHORIZED, "USER_REQUIRED", "Sign in with a Portal account.")
        })?;
        Ok(AccountUser { id, is_admin: principal.is_admin })
    }
}

impl axum::extract::FromRequestParts<Arc<AppState>> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let user = AccountUser::from_request_parts(parts, state).await?;
        if !user.is_admin {
            return Err(ApiError::coded(StatusCode::FORBIDDEN, "ADMIN_REQUIRED", "Admin only."));
        }
        Ok(AdminUser(user))
    }
}

async fn issue_session(state: &AppState, user: &UserRow) -> Result<Json<Value>, ApiError> {
    let access = sign_access(user)?;
    let refresh = random_hex(32);
    let expires = Utc::now().naive_utc() + ChronoDuration::days(REFRESH_DAYS);
    sqlx::query(&crate::db::sql(
        "INSERT INTO sessions (user_id, refresh_token_hash, expires_at, revoked, created_at) \
         VALUES (?, ?, ?, 0, ?)",
        state.backend,
    ))
    .bind(user.id)
    .bind(hash_token(&refresh))
    .bind(crate::wire::sql_string(expires))
    .bind(sql_now())
    .execute(&state.any)
    .await?;
    Ok(Json(json!({
        "access_token": access,
        "refresh_token": refresh,
        "token_type": "Bearer",
        "expires_in": ACCESS_MINUTES * 60,
        "user": user_json(user),
    })))
}

async fn upsert_user_on_login(state: &AppState, email: &str) -> Result<UserRow, ApiError> {
    if let Some(existing) = find_user_by_email(state, email).await? {
        let _ = crate::identity::ensure_user_workspace(
            state,
            existing.id,
            existing.username.as_deref().unwrap_or("user"),
            &existing.kind,
        )
        .await;
        return Ok(existing);
    }
    let admin = is_admin_email(email);
    let entitlement = if admin { "comp" } else { "trial" };
    let trial_ends = if admin {
        None
    } else {
        Some(crate::wire::sql_string(Utc::now().naive_utc() + ChronoDuration::days(trial_days())))
    };
    sqlx::query(&crate::db::sql(
        "INSERT INTO users (email, username, kind, is_admin, entitlement, trial_ends_at, created_at, updated_at) \
         VALUES (?, NULL, 'cloud', ?, ?, ?, ?, ?)",
        state.backend,
    ))
    .bind(email)
    .bind(i64::from(admin))
    .bind(entitlement)
    .bind(trial_ends.as_deref())
    .bind(sql_now())
    .bind(sql_now())
    .execute(&state.any)
    .await?;
    let row = find_user_by_email(state, email)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Failed to create account."))?;
    let _ = crate::identity::ensure_user_workspace(state, row.id, "user", "cloud").await;
    Ok(row)
}

async fn upsert_user_shell(state: &AppState, email: &str) -> Result<UserRow, ApiError> {
    if let Some(existing) = find_user_by_email(state, email).await? {
        return Ok(existing);
    }
    sqlx::query(&crate::db::sql(
        "INSERT INTO users (email, username, kind, is_admin, entitlement, trial_ends_at, created_at, updated_at) \
         VALUES (?, NULL, 'cloud', 0, 'blocked', NULL, ?, ?)",
        state.backend,
    ))
    .bind(email)
    .bind(sql_now())
    .bind(sql_now())
    .execute(&state.any)
    .await?;
    find_user_by_email(state, email)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Failed to create account."))
}

pub async fn load_user(state: &AppState, id: i64) -> Result<Option<UserRow>, ApiError> {
    sqlx::query_as(&crate::db::sql(&format!("SELECT {USER_COLS} FROM users WHERE id = ?"), state.backend))
        .bind(id)
        .fetch_optional(&state.any)
        .await
        .map_err(Into::into)
}

pub async fn find_user_by_email(state: &AppState, email: &str) -> Result<Option<UserRow>, ApiError> {
    sqlx::query_as(&crate::db::sql(&format!("SELECT {USER_COLS} FROM users WHERE email = ?"), state.backend))
        .bind(email)
        .fetch_optional(&state.any)
        .await
        .map_err(Into::into)
}

async fn usage_days(state: &AppState, user_id: i64) -> Result<Vec<Value>, ApiError> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(&crate::db::sql(
        "SELECT usage_date, CAST(request_count AS BIGINT), CAST(error_count AS BIGINT), \
                CAST(total_tokens AS BIGINT) \
         FROM user_usage_daily WHERE user_id = ? ORDER BY usage_date DESC LIMIT 31",
        state.backend,
    ))
    .bind(user_id)
    .fetch_all(&state.any)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(d, r, e, t)| json!({"date": d, "requests": r, "errors": e, "tokens": t}))
        .collect())
}

pub fn user_json(row: &UserRow) -> Value {
    json!({
        "id": row.id,
        "email": row.email,
        "username": row.username,
        "kind": row.kind,
        "is_admin": row.is_admin != 0,
        "entitlement": row.entitlement,
        "trial_ends_at": row.trial_ends_at,
        "billing_region": row.billing_region,
        "stripe_customer_id": row.stripe_customer_id,
        "stripe_price_id": row.stripe_price_id,
        "comp_reason": row.comp_reason,
        "comp_expires_at": row.comp_expires_at,
    })
}

pub fn normalize_email(raw: &str) -> Result<String, ApiError> {
    let email = raw.trim().to_ascii_lowercase();
    if !email.contains('@') || email.len() < 5 || email.len() > 320 {
        return Err(ApiError::bad_request("Enter a valid email."));
    }
    Ok(email)
}

fn is_disposable(email: &str) -> bool {
    let domain = email.rsplit('@').next().unwrap_or("");
    DISPOSABLE.iter().any(|d| domain.eq_ignore_ascii_case(d))
}

fn is_admin_email(email: &str) -> bool {
    env_opt("AGENT_PLATFORM_ADMIN_EMAILS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .any(|s| s == email)
}

fn trial_days() -> i64 {
    env_opt("AGENT_PLATFORM_TRIAL_DAYS")
        .and_then(|s| s.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(14)
}

pub fn env_u64(name: &str, default: u64) -> u64 {
    env_opt(name).and_then(|s| s.parse().ok()).unwrap_or(default)
}

pub fn public_base() -> String {
    env_opt("AGENT_PLATFORM_PUBLIC_URL")
        .unwrap_or_else(|| {
            format!(
                "http://127.0.0.1:{}",
                env_opt("AGENT_PLATFORM_PORT").unwrap_or_else(|| "18410".into())
            )
        })
        .trim_end_matches('/')
        .to_string()
}

pub fn random_hex(n: usize) -> String {
    let mut buf = vec![0u8; n];
    let _ = getrandom::getrandom(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Native apps pass a loopback callback so the email click lands back in the
/// process that asked, not in a browser session the iced app cannot read.
pub fn validate_loopback_redirect(raw: &str) -> Result<String, ApiError> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| {
        ApiError::bad_request("redirect_uri must be an http URL on this machine.")
    })?;
    if url.scheme() != "http" {
        return Err(ApiError::bad_request("redirect_uri must be http on loopback."));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ApiError::bad_request("redirect_uri must not carry credentials."));
    }
    let host = url.host_str().unwrap_or("");
    // `host_str` keeps the brackets on an IPv6 literal; `IpAddr` will not parse them.
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let loopback = host.eq_ignore_ascii_case("localhost")
        || bare.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
    if !loopback {
        return Err(ApiError::bad_request("redirect_uri must be 127.0.0.1 or localhost."));
    }
    Ok(url.to_string())
}

/// What the desktop writes to `cloud.session.json` (ADR 0013). The daemon reads
/// it as provider `platform`; the desktop Account card is the writer at login
/// and logout. Refresh rotation is the daemon's after that.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CloudSessionFile {
    pub url: String,
    pub refresh_token: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub access_expires_at: i64,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub entitlement: String,
    #[serde(default)]
    pub is_admin: bool,
}

pub fn cloud_session_path() -> Option<PathBuf> {
    env_opt(SESSION_ENV).map(PathBuf::from).filter(|p| !p.as_os_str().is_empty())
}

pub fn read_cloud_session() -> Option<CloudSessionFile> {
    let path = cloud_session_path()?;
    let raw = std::fs::read_to_string(&path).ok()?;
    let session: CloudSessionFile = serde_json::from_str(&raw).ok()?;
    if session.url.trim().is_empty() || session.refresh_token.trim().is_empty() {
        return None;
    }
    Some(session)
}

pub fn platform_configured() -> bool {
    read_cloud_session().is_some()
}

fn write_cloud_session(path: &std::path::Path, session: &CloudSessionFile) -> Result<(), ApiError> {
    let tmp = path.with_extension("tmp");
    let body = serde_json::to_vec_pretty(session)
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    std::fs::write(&tmp, body)
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(())
}

fn is_self_origin(raw: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(raw) else {
        return false;
    };
    let host = url.host_str().unwrap_or("");
    if !crate::is_loopback_host(host) {
        return false;
    }
    let port = url.port_or_known_default().unwrap_or(80);
    let ours: u16 = env_opt("AGENT_PLATFORM_PORT")
        .and_then(|s| s.parse().ok())
        .unwrap_or(18410);
    port == ours
}

/// A live access JWT for provider `platform`. Refreshes the session file when
/// the cached one is within a minute of expiry.
pub async fn ensure_platform_access(state: &AppState) -> Result<String, ApiError> {
    let now = Utc::now().timestamp();
    {
        let guard = state.platform_access.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((token, exp)) = guard.as_ref() {
            if *exp > now + 60 {
                return Ok(token.clone());
            }
        }
    }
    let mut session = read_cloud_session().ok_or_else(|| {
        ApiError::coded(
            StatusCode::SERVICE_UNAVAILABLE,
            "platform_unconfigured",
            "Sign in under Settings → Account to use Platform AI.",
        )
    })?;
    let base = session.url.trim_end_matches('/').to_string();
    if is_self_origin(&base) {
        return Err(ApiError::coded(
            StatusCode::SERVICE_UNAVAILABLE,
            "platform_self",
            "Cloud URL cannot be this server. Point Account at the hosted origin.",
        ));
    }
    if session.access_expires_at > now + 60 && !session.access_token.is_empty() {
        let mut guard = state.platform_access.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some((session.access_token.clone(), session.access_expires_at));
        return Ok(session.access_token);
    }
    let url = format!("{base}/accounts/api/v1/auth/refresh");
    let resp = state
        .http
        .post(&url)
        .json(&json!({ "refresh_token": session.refresh_token }))
        .send()
        .await
        .map_err(|e| {
            ApiError::coded(
                StatusCode::BAD_GATEWAY,
                "platform_refresh_failed",
                format!("Could not refresh the cloud session: {e}"),
            )
        })?;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(json!({}));
    if !status.is_success() {
        let msg = body
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("Cloud session expired. Sign in again.");
        return Err(ApiError::coded(
            StatusCode::UNAUTHORIZED,
            "platform_session_expired",
            msg,
        ));
    }
    let access = body["access_token"]
        .as_str()
        .ok_or_else(|| ApiError::bad_request("Cloud refresh returned no access_token."))?
        .to_string();
    let refresh = body["refresh_token"]
        .as_str()
        .unwrap_or(&session.refresh_token)
        .to_string();
    let expires_in = body["expires_in"].as_i64().unwrap_or(ACCESS_MINUTES * 60);
    session.access_token = access.clone();
    session.refresh_token = refresh;
    session.access_expires_at = now + expires_in;
    if let Some(email) = body.pointer("/user/email").and_then(Value::as_str) {
        session.email = email.to_string();
    }
    if let Some(ent) = body.pointer("/user/entitlement").and_then(Value::as_str) {
        session.entitlement = ent.to_string();
    }
    if let Some(path) = cloud_session_path() {
        let _ = write_cloud_session(&path, &session);
    }
    let mut guard = state.platform_access.lock().unwrap_or_else(|e| e.into_inner());
    *guard = Some((access.clone(), session.access_expires_at));
    Ok(access)
}

pub fn cached_platform_access(state: &AppState) -> Result<String, ApiError> {
    state
        .platform_access
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|(t, _)| t.clone())
        .ok_or_else(|| {
            ApiError::coded(
                StatusCode::SERVICE_UNAVAILABLE,
                "platform_unconfigured",
                "Sign in under Settings → Account to use Platform AI.",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_redirects_are_accepted() {
        assert!(validate_loopback_redirect("http://127.0.0.1:54321/callback").is_ok());
        assert!(validate_loopback_redirect("http://localhost:9/callback").is_ok());
        assert!(validate_loopback_redirect("http://[::1]:9/callback").is_ok());
    }

    #[test]
    fn off_box_redirects_are_refused() {
        assert!(validate_loopback_redirect("https://127.0.0.1/callback").is_err());
        assert!(validate_loopback_redirect("http://example.com/callback").is_err());
        assert!(validate_loopback_redirect("http://127.0.0.1.evil.test/callback").is_err());
        assert!(validate_loopback_redirect("http://user:pass@127.0.0.1/callback").is_err());
    }
}
