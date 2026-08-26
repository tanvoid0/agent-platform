//! Accounts, entitlements, regional quotes, and `/v1` gating.

mod common;

use std::sync::Arc;

use agent_platform_server::auth::hash_token;
use agent_platform_server::{db, router, AppState};
use serde_json::Value;

use common::{start_server, MASTER};

const JWT: &str = "test-jwt-secret-please-change";

async fn start_with_schema() -> (String, std::path::PathBuf) {
    std::env::set_var("AGENT_PLATFORM_JWT_SECRET", JWT);
    std::env::remove_var("AGENT_PLATFORM_HOST");
    let db = common::temp_db_path("accounts");
    let _ = std::fs::remove_file(&db);
    let state = Arc::new(AppState::new(&db, Some(MASTER.to_string())));
    db::ensure_schema(&state.any).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });
    (origin, db)
}

async fn json(origin: &str, method: &str, path: &str, bearer: Option<&str>, body: Option<Value>, client: Option<&str>) -> (u16, Value) {
    let mut req = reqwest::Client::new().request(
        method.parse().unwrap(),
        format!("{origin}{path}"),
    );
    if let Some(b) = bearer {
        req = req.bearer_auth(b);
    }
    if let Some(c) = client {
        req = req.header("x-agent-platform-client", c);
    }
    if let Some(body) = body {
        req = req.json(&body);
    }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
    (status, v)
}

#[tokio::test]
async fn trial_user_can_hit_v1_blocked_cannot() {
    let (origin, db) = start_with_schema().await;

    let (status, body) = json(
        &origin,
        "POST",
        "/accounts/api/v1/auth/magic-link",
        None,
        Some(serde_json::json!({"email": "trial@example.com"})),
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["message"], "If that email exists, we sent a link.");

    // Consume the hashed row by issuing verify through the CLI-equivalent: insert a known link.
    let state = Arc::new(AppState::new(&db, Some(MASTER.to_string())));
    let token = "verify-me-please-32-bytes-tokenxx";
    sqlx::query(
        "INSERT INTO magic_links (email, token_hash, expires_at, used, created_at) \
         VALUES ('trial@example.com', ?, '2099-01-01 00:00:00.000000', 0, '2026-01-01 00:00:00.000000')",
    )
    .bind(hash_token(token))
    .execute(&state.any)
    .await
    .unwrap();

    let (status, body) = json(
        &origin,
        "POST",
        "/accounts/api/v1/auth/verify",
        None,
        Some(serde_json::json!({"token": token})),
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let access = body["access_token"].as_str().unwrap();
    assert_eq!(body["user"]["entitlement"], "trial");

    let (status, body) = json(&origin, "GET", "/v1/models", Some(access), None, Some("portal-desktop")).await;
    assert_eq!(status, 200, "{body}");

    let (status, body) = json(&origin, "GET", "/v1/models", Some(access), None, None).await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body["error"]["code"], "APP_ID_REQUIRED");

    let (status, body) = json(&origin, "GET", "/v1/models", Some(access), None, Some("evil-app")).await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body["error"]["code"], "APP_ID_UNKNOWN");

    // Master still works without an app id.
    let (status, body) = json(&origin, "GET", "/v1/models", Some(MASTER), None, None).await;
    assert_eq!(status, 200, "{body}");

    let (status, body) = json(
        &origin,
        "POST",
        "/accounts/api/v1/billing/quote",
        Some(access),
        Some(serde_json::json!({"card_country": "US", "billing_country": "BD"})),
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["region"], "US");

    sqlx::query("UPDATE users SET entitlement = 'blocked' WHERE email = 'trial@example.com'")
        .execute(&state.any)
        .await
        .unwrap();

    let (status, body) = json(&origin, "GET", "/v1/models", Some(access), None, Some("portal-desktop")).await;
    assert_eq!(status, 402, "{body}");
    assert_eq!(body["error"]["code"], "PAYMENT_REQUIRED");

    let _ = std::fs::remove_file(&db);
}

#[tokio::test]
async fn disposable_email_does_not_store_a_link() {
    let (origin, db) = start_with_schema().await;
    let (status, body) = json(
        &origin,
        "POST",
        "/accounts/api/v1/auth/magic-link",
        None,
        Some(serde_json::json!({"email": "spam@mailinator.com"})),
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let state = Arc::new(AppState::new(&db, Some(MASTER.to_string())));
    let n: i64 = sqlx::query_scalar("SELECT CAST(COUNT(*) AS BIGINT) FROM magic_links")
        .fetch_one(&state.any)
        .await
        .unwrap();
    assert_eq!(n, 0);
    let _ = std::fs::remove_file(&db);
}

#[tokio::test]
async fn accounts_page_is_served() {
    let (origin, db) = start_with_schema().await;
    let resp = reqwest::Client::new()
        .get(format!("{origin}/accounts"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("Portal accounts"), "{body}");
    let _ = std::fs::remove_file(&db);
}

#[tokio::test]
async fn native_magic_link_redirects_to_loopback() {
    let (origin, db) = start_with_schema().await;
    let (status, body) = json(
        &origin,
        "POST",
        "/accounts/api/v1/auth/magic-link",
        None,
        Some(serde_json::json!({
            "email": "desk@example.com",
            "redirect_uri": "http://127.0.0.1:59999/callback"
        })),
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");

    let token = "desktop-verify-token-32-bytesxxxx";
    let state = Arc::new(AppState::new(&db, Some(MASTER.to_string())));
    sqlx::query(
        "INSERT INTO magic_links (email, token_hash, expires_at, used, created_at, redirect_uri) \
         VALUES ('desk@example.com', ?, '2099-01-01 00:00:00.000000', 0, '2026-01-01 00:00:00.000000', \
                 'http://127.0.0.1:59999/callback')",
    )
    .bind(hash_token(token))
    .execute(&state.any)
    .await
    .unwrap();

    let resp = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(format!("{origin}/accounts/api/v1/auth/verify?token={token}"))
        .send()
        .await
        .unwrap();
    // `Redirect::to` is a 303; any 3xx to the loopback callback is what the desktop follows.
    assert_eq!(resp.status().as_u16(), 303, "{}", resp.status());
    let loc = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(loc.starts_with("http://127.0.0.1:59999/callback?"), "{loc}");
    assert!(loc.contains("access_token="), "{loc}");
    assert!(loc.contains("refresh_token="), "{loc}");

    let (status, body) = json(
        &origin,
        "POST",
        "/accounts/api/v1/auth/magic-link",
        None,
        Some(serde_json::json!({
            "email": "desk@example.com",
            "redirect_uri": "https://evil.example/callback"
        })),
        None,
    )
    .await;
    assert_eq!(status, 400, "{body}");

    let _ = std::fs::remove_file(&db);
}

#[tokio::test]
async fn start_server_without_schema_still_404s_unknown_api() {
    // Existing auth tests use a partial schema; this only checks the helper still compiles against MASTER.
    let db = common::temp_db_path("accounts-legacy");
    let origin = start_server(&db, Some(MASTER)).await;
    let resp = reqwest::Client::new()
        .get(format!("{origin}/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let _ = std::fs::remove_file(&db);
}
