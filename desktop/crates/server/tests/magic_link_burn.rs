//! Board #1009: a magic link must survive a redeem that fails after the link was
//! checked (the 503 `JWT_UNCONFIGURED` of a hosted box with no secret), and one
//! link must mint exactly one session however many requests redeem it at once.
//!
//! One test, because it owns the process-wide env (`AGENT_PLATFORM_HOST`,
//! `AGENT_PLATFORM_JWT_SECRET`) — which is also why it is its own `tests/*.rs`
//! binary: `accounts.rs` sets the secret and its tests run in parallel.

mod common;

use std::sync::Arc;

use agent_platform_server::auth::hash_token;
use agent_platform_server::{db, router, AppState};
use serde_json::{json, Value};

use common::{temp_db_path, MASTER};

async fn seed_link(state: &AppState, token: &str) {
    sqlx::query(
        "INSERT INTO magic_links (email, token_hash, expires_at, used, created_at) \
         VALUES ('burn@example.com', ?, '2099-01-01 00:00:00.000000', 0, '2026-01-01 00:00:00.000000')",
    )
    .bind(hash_token(token))
    .execute(&state.any)
    .await
    .unwrap();
}

async fn link_used(state: &AppState, token: &str) -> i64 {
    sqlx::query_scalar("SELECT CAST(used AS BIGINT) FROM magic_links WHERE token_hash = ?")
        .bind(hash_token(token))
        .fetch_one(&state.any)
        .await
        .unwrap()
}

async fn verify(origin: &str, token: &str) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{origin}/accounts/api/v1/auth/verify"))
        .json(&json!({ "token": token }))
        .send()
        .await
        .unwrap();
    (resp.status().as_u16(), resp.json().await.unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_redeem_keeps_the_link_and_one_link_mints_one_session() {
    // A server bound beyond the loopback with no secret: it cannot sign a session.
    std::env::remove_var("AGENT_PLATFORM_JWT_SECRET");
    std::env::set_var("AGENT_PLATFORM_HOST", "0.0.0.0");
    let db = temp_db_path("magic-link-burn");
    let _ = std::fs::remove_file(&db);
    let state = Arc::new(AppState::new(&db, Some(MASTER.to_string())));
    db::ensure_schema(&state.any).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn({
        let state = state.clone();
        async move { axum::serve(listener, router(state)).await.unwrap() }
    });

    // (a) The 503 must not burn the link.
    let token = "burn-me-not-please-32-bytes-token";
    seed_link(&state, token).await;
    let (status, body) = verify(&origin, token).await;
    assert_eq!(status, 503, "{body}");
    assert_eq!(body["error"]["code"], "JWT_UNCONFIGURED");
    assert_eq!(
        link_used(&state, token).await,
        0,
        "a failed redeem burned the link"
    );

    // (b) The operator sets the secret: the same link now works — once.
    std::env::set_var("AGENT_PLATFORM_JWT_SECRET", "test-jwt-secret-please-change");
    let (status, body) = verify(&origin, token).await;
    assert_eq!(status, 200, "{body}");
    assert!(body["access_token"].as_str().is_some(), "{body}");
    let (status, body) = verify(&origin, token).await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body["error"]["code"], "LINK_USED");

    // (c) Eight redeems of one fresh link at once: exactly one wins the claim.
    let token = "race-the-link-please-32-bytes-tok";
    seed_link(&state, token).await;
    let results = futures::future::join_all((0..8).map(|_| verify(&origin, token))).await;
    let wins = results.iter().filter(|(status, _)| *status == 200).count();
    assert_eq!(wins, 1, "one link minted {wins} sessions: {results:?}");
    for (status, body) in results.iter().filter(|(status, _)| *status != 200) {
        assert_eq!(*status, 401, "{body}");
        assert_eq!(body["error"]["code"], "LINK_USED", "{body}");
    }
    assert_eq!(link_used(&state, token).await, 1);

    let _ = std::fs::remove_file(&db);
}
