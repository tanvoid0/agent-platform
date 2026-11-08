//! Board #95, round two: a local/desktop install (no `AGENT_PLATFORM_JWT_SECRET`,
//! loopback bind) hands out a magic-link `dev_link` on that exact gate — this
//! proves redeeming it actually works instead of dead-ending on
//! `JWT_UNCONFIGURED` one step later. Own process (own `tests/*.rs` binary), so
//! clearing the env var here cannot race `accounts.rs`'s tests, which set it.

mod common;

use std::sync::Arc;

use agent_platform_server::{db, router, AppState};
use serde_json::Value;

use common::{temp_db_path, MASTER};

#[tokio::test]
async fn a_local_install_can_redeem_its_own_dev_link() {
    std::env::remove_var("AGENT_PLATFORM_JWT_SECRET");
    std::env::remove_var("AGENT_PLATFORM_HOST");
    let db = temp_db_path("accounts-local-jwt");
    std::env::set_var("AGENT_PLATFORM_DB_PATH", &db);
    let _ = std::fs::remove_file(&db);
    let secret_path = db.with_file_name("jwt_secret");
    let _ = std::fs::remove_file(&secret_path);

    let state = Arc::new(AppState::new(&db, Some(MASTER.to_string())));
    db::ensure_schema(&state.any).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{origin}/accounts/api/v1/auth/magic-link"))
        .json(&serde_json::json!({"email": "local@example.com"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    let dev_link = body["dev_link"].as_str().expect("dev_link on a local/loopback install");
    let token = dev_link.rsplit("token=").next().unwrap();

    let resp = client
        .post(format!("{origin}/accounts/api/v1/auth/verify"))
        .json(&serde_json::json!({"token": token}))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(status, 200, "{body}");
    let access = body["access_token"].as_str().expect("a real access token, not JWT_UNCONFIGURED");

    let resp = client
        .get(format!("{origin}/accounts/api/v1/me"))
        .bearer_auth(access)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    assert!(secret_path.exists(), "the generated secret should persist beside the db");

    let _ = std::fs::remove_file(&db);
    let _ = std::fs::remove_file(&secret_path);
}
