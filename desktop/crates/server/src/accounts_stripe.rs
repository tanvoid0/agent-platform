//! Stripe Checkout, Customer Portal, and webhooks for Portal AI Access.
//! Price ID is chosen on the server from card + billing country. Clients cannot pick it.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;

use crate::accounts::{self, AccountUser, QuoteBody};
use crate::billing;
use crate::error::ApiError;
use crate::wire::sql_now;
use crate::{env_opt, AppState};

pub async fn billing_checkout(
    State(state): State<Arc<AppState>>,
    user: AccountUser,
    Json(body): Json<QuoteBody>,
) -> Result<Json<Value>, ApiError> {
    let row = accounts::load_user(&state, user.id)
        .await?
        .ok_or_else(|| ApiError::not_found("Account not found."))?;
    let q = billing::quote(body.card_country.as_deref(), body.billing_country.as_deref());
    let Some(secret) = env_opt("STRIPE_SECRET_KEY") else {
        return Err(ApiError::coded(
            StatusCode::SERVICE_UNAVAILABLE,
            "STRIPE_UNCONFIGURED",
            "Stripe is not configured. Locally, use set-entitlement paid.",
        ));
    };
    let Some(price) = q.stripe_price_id.clone() else {
        return Err(ApiError::coded(
            StatusCode::SERVICE_UNAVAILABLE,
            "PRICE_UNCONFIGURED",
            format!("No Stripe Price ID for region {}.", q.region),
        ));
    };
    let customer = ensure_stripe_customer(&state.http, &secret, &row).await?;
    sqlx::query(&crate::db::sql(
        "UPDATE users SET stripe_customer_id = ?, updated_at = ? WHERE id = ?",
        state.backend,
    ))
    .bind(&customer)
    .bind(sql_now())
    .bind(row.id)
    .execute(&state.any)
    .await?;

    let success = format!("{}/accounts/#/me?checkout=success", accounts::public_base());
    let cancel = format!("{}/accounts/#/me?checkout=cancel", accounts::public_base());
    let form = vec![
        ("mode", "subscription".into()),
        ("customer", customer),
        ("success_url", success),
        ("cancel_url", cancel),
        ("line_items[0][price]", price.clone()),
        ("line_items[0][quantity]", "1".into()),
        ("automatic_tax[enabled]", "true".into()),
        ("client_reference_id", row.id.to_string()),
        ("metadata[user_id]", row.id.to_string()),
        ("metadata[region]", q.region.clone()),
        ("subscription_data[metadata][user_id]", row.id.to_string()),
        ("subscription_data[metadata][region]", q.region.clone()),
        ("subscription_data[metadata][price_id]", price),
    ];
    let session =
        stripe_form(&state.http, &secret, "https://api.stripe.com/v1/checkout/sessions", &form).await?;
    let url = session
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::new(StatusCode::BAD_GATEWAY, "Stripe Checkout returned no URL."))?;
    Ok(Json(json!({ "url": url, "region": q.region, "amount_minor": q.amount_minor })))
}

pub async fn billing_portal(
    State(state): State<Arc<AppState>>,
    user: AccountUser,
) -> Result<Json<Value>, ApiError> {
    let row = accounts::load_user(&state, user.id)
        .await?
        .ok_or_else(|| ApiError::not_found("Account not found."))?;
    let Some(secret) = env_opt("STRIPE_SECRET_KEY") else {
        return Err(ApiError::coded(
            StatusCode::SERVICE_UNAVAILABLE,
            "STRIPE_UNCONFIGURED",
            "Stripe is not configured.",
        ));
    };
    let Some(customer) = row.stripe_customer_id.clone() else {
        return Err(ApiError::bad_request("No Stripe customer on this account."));
    };
    let return_url = format!("{}/accounts/#/me", accounts::public_base());
    let form = vec![("customer", customer), ("return_url", return_url)];
    let session =
        stripe_form(&state.http, &secret, "https://api.stripe.com/v1/billing_portal/sessions", &form)
            .await?;
    let url = session
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::new(StatusCode::BAD_GATEWAY, "Stripe Portal returned no URL."))?;
    Ok(Json(json!({ "url": url })))
}

pub async fn stripe_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let secret = env_opt("STRIPE_WEBHOOK_SECRET").ok_or_else(|| {
        ApiError::coded(StatusCode::SERVICE_UNAVAILABLE, "STRIPE_UNCONFIGURED", "No webhook secret.")
    })?;
    let sig = headers
        .get("stripe-signature")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::coded(StatusCode::BAD_REQUEST, "SIG_MISSING", "Missing Stripe-Signature."))?;
    verify_stripe_sig(&secret, sig, &body)?;
    let event: Value = serde_json::from_slice(&body).map_err(|_| ApiError::bad_request("Invalid JSON."))?;
    let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
    let obj = event.get("data").and_then(|d| d.get("object")).cloned().unwrap_or(Value::Null);
    match kind {
        "checkout.session.completed" | "invoice.paid" => apply_paid(&state, &obj).await?,
        "customer.subscription.deleted" | "invoice.payment_failed" => apply_blocked(&state, &obj).await?,
        _ => {}
    }
    Ok(Json(json!({ "received": true })))
}

async fn ensure_stripe_customer(
    http: &reqwest::Client,
    secret: &str,
    user: &accounts::UserRow,
) -> Result<String, ApiError> {
    if let Some(id) = user.stripe_customer_id.clone() {
        return Ok(id);
    }
    let form = vec![("email", user.email.clone()), ("metadata[user_id]", user.id.to_string())];
    let v = stripe_form(http, secret, "https://api.stripe.com/v1/customers", &form).await?;
    v.get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ApiError::new(StatusCode::BAD_GATEWAY, "Stripe customer had no id."))
}

async fn stripe_form(
    http: &reqwest::Client,
    secret: &str,
    url: &str,
    form: &[(&str, String)],
) -> Result<Value, ApiError> {
    let resp = http
        .post(url)
        .basic_auth(secret, None::<&str>)
        .form(form)
        .send()
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, format!("Stripe: {e}")))?;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(json!({}));
    if !status.is_success() {
        let msg = body
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("Stripe request failed");
        return Err(ApiError::new(StatusCode::BAD_GATEWAY, msg.to_string()));
    }
    Ok(body)
}

fn verify_stripe_sig(secret: &str, header: &str, payload: &[u8]) -> Result<(), ApiError> {
    let mut timestamp = None;
    let mut signatures: Vec<&str> = Vec::new();
    for part in header.split(',') {
        let mut kv = part.splitn(2, '=');
        match (kv.next(), kv.next()) {
            (Some("t"), Some(v)) => timestamp = Some(v.trim()),
            (Some("v1"), Some(v)) => signatures.push(v.trim()),
            _ => {}
        }
    }
    let t = timestamp.ok_or_else(|| {
        ApiError::coded(StatusCode::BAD_REQUEST, "SIG_INVALID", "Bad Stripe-Signature.")
    })?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "hmac"))?;
    mac.update(t.as_bytes());
    mac.update(b".");
    mac.update(payload);
    let expect = mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>();
    if !signatures.iter().any(|s| s.eq_ignore_ascii_case(&expect)) {
        return Err(ApiError::coded(StatusCode::BAD_REQUEST, "SIG_INVALID", "Stripe signature mismatch."));
    }
    Ok(())
}

async fn apply_paid(state: &AppState, obj: &Value) -> Result<(), ApiError> {
    let user_id = obj
        .pointer("/metadata/user_id")
        .or_else(|| obj.pointer("/client_reference_id"))
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<i64>().ok());
    let Some(user_id) = user_id else {
        logd!("[stripe] paid event with no user_id");
        return Ok(());
    };
    let customer = obj.get("customer").and_then(Value::as_str);
    let subscription = obj.get("subscription").and_then(Value::as_str);
    let card = obj
        .pointer("/payment_method_details/card/country")
        .or_else(|| obj.pointer("/customer_details/address/country"))
        .and_then(Value::as_str);
    let billing = obj.pointer("/customer_details/address/country").and_then(Value::as_str);
    let q = billing::quote(card, billing);
    sqlx::query(&crate::db::sql(
        "UPDATE users SET entitlement = 'paid', billing_region = COALESCE(billing_region, ?), \
         stripe_customer_id = COALESCE(?, stripe_customer_id), \
         stripe_subscription_id = COALESCE(?, stripe_subscription_id), \
         stripe_price_id = COALESCE(?, stripe_price_id), updated_at = ? \
         WHERE id = ?",
        state.backend,
    ))
    .bind(&q.region)
    .bind(customer)
    .bind(subscription)
    .bind(q.stripe_price_id.as_deref())
    .bind(sql_now())
    .bind(user_id)
    .execute(&state.any)
    .await?;
    Ok(())
}

async fn apply_blocked(state: &AppState, obj: &Value) -> Result<(), ApiError> {
    if let Some(user_id) = obj
        .pointer("/metadata/user_id")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<i64>().ok())
    {
        sqlx::query(&crate::db::sql(
            "UPDATE users SET entitlement = 'blocked', updated_at = ? WHERE id = ? AND entitlement != 'comp'",
            state.backend,
        ))
        .bind(sql_now())
        .bind(user_id)
        .execute(&state.any)
        .await?;
        return Ok(());
    }
    if let Some(customer) = obj.get("customer").and_then(Value::as_str) {
        sqlx::query(&crate::db::sql(
            "UPDATE users SET entitlement = 'blocked', updated_at = ? \
             WHERE stripe_customer_id = ? AND entitlement != 'comp'",
            state.backend,
        ))
        .bind(sql_now())
        .bind(customer)
        .execute(&state.any)
        .await?;
    }
    Ok(())
}
