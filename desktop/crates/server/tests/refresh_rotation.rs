//! Board #1081: a refresh token must survive a refresh that fails after its session was
//! checked (the 503 `JWT_UNCONFIGURED` of a hosted box with no secret), and one refresh
//! token must rotate exactly once however many requests present it at once. And a revoke that
//! lands while a refresh holds its claim (`revoked = 2`) must not be undone by that refresh.
//!
//! One test, because it owns the process-wide env (`AGENT_PLATFORM_HOST`,
//! `AGENT_PLATFORM_JWT_SECRET`) — which is also why it is its own `tests/*.rs`
//! binary: `accounts.rs` sets the secret and its tests run in parallel.

mod common;

use std::sync::Arc;

use agent_platform_server::accounts::revoke_sessions;
use agent_platform_server::auth::hash_token;
use agent_platform_server::{db, router, AppState};
use serde_json::{json, Value};

use common::{temp_db_path, MASTER};

const SECRET: &str = "test-jwt-secret-please-change";

async fn post(origin: &str, route: &str, body: Value) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{origin}/accounts/api/v1/auth/{route}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    (resp.status().as_u16(), resp.json().await.unwrap())
}

async fn refresh(origin: &str, token: &str) -> (u16, Value) {
    post(origin, "refresh", json!({ "refresh_token": token })).await
}

async fn revoked(state: &AppState, token: &str) -> i64 {
    sqlx::query_scalar("SELECT CAST(revoked AS BIGINT) FROM sessions WHERE refresh_token_hash = ?")
        .bind(hash_token(token))
        .fetch_one(&state.any)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_refresh_keeps_the_token_and_one_token_rotates_once() {
    // A server bound beyond the loopback signs sessions only while a secret is set.
    std::env::set_var("AGENT_PLATFORM_JWT_SECRET", SECRET);
    std::env::set_var("AGENT_PLATFORM_HOST", "0.0.0.0");
    let db = temp_db_path("refresh-rotation");
    let _ = std::fs::remove_file(&db);
    let state = Arc::new(AppState::new(&db, Some(MASTER.to_string())));
    db::ensure_schema(&state.any).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn({
        let state = state.clone();
        async move { axum::serve(listener, router(state)).await.unwrap() }
    });

    // The first session, minted the way tests/accounts.rs does: seed a link, redeem it.
    let link = "rotate-me-please-32-bytes-tokenx";
    sqlx::query(
        "INSERT INTO magic_links (email, token_hash, expires_at, used, created_at) \
         VALUES ('rotate@example.com', ?, '2099-01-01 00:00:00.000000', 0, '2026-01-01 00:00:00.000000')",
    )
    .bind(hash_token(link))
    .execute(&state.any)
    .await
    .unwrap();
    let (status, body) = post(&origin, "verify", json!({ "token": link })).await;
    assert_eq!(status, 200, "{body}");
    let first = body["refresh_token"].as_str().unwrap().to_string();

    // (a) The secret goes away: the refresh answers 503, and that must not burn the token.
    std::env::remove_var("AGENT_PLATFORM_JWT_SECRET");
    let (status, body) = refresh(&origin, &first).await;
    assert_eq!(status, 503, "{body}");
    assert_eq!(body["error"]["code"], "JWT_UNCONFIGURED");
    assert_eq!(
        revoked(&state, &first).await,
        0,
        "a failed refresh burned the token"
    );

    // (b) The operator sets it again: the same token now rotates — once.
    std::env::set_var("AGENT_PLATFORM_JWT_SECRET", SECRET);
    let (status, body) = refresh(&origin, &first).await;
    assert_eq!(status, 200, "{body}");
    let second = body["refresh_token"].as_str().unwrap().to_string();
    assert_eq!(
        revoked(&state, &first).await,
        1,
        "a rotated token must end revoked"
    );
    let (status, body) = refresh(&origin, &first).await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body["error"]["code"], "SESSION_REVOKED");

    // (c) Eight refreshes of one fresh token at once: exactly one wins the claim. Best effort for
    // the atomic claim itself: a request that arrives after the winner's claim is already stopped by
    // the SELECT check, so only requests that overlap exercise `AND revoked = 0` (before the fix
    // this minted 3 to 8 sessions).
    let results = futures::future::join_all((0..8).map(|_| refresh(&origin, &second))).await;
    let wins = results.iter().filter(|(status, _)| *status == 200).count();
    assert_eq!(
        wins, 1,
        "one refresh token minted {wins} sessions: {results:?}"
    );
    for (status, body) in results.iter().filter(|(status, _)| *status != 200) {
        assert_eq!(*status, 401, "{body}");
        assert_eq!(body["error"]["code"], "SESSION_REVOKED", "{body}");
    }
    let live: i64 =
        sqlx::query_scalar("SELECT CAST(COUNT(*) AS BIGINT) FROM sessions WHERE revoked = 0")
            .fetch_one(&state.any)
            .await
            .unwrap();
    assert_eq!(live, 1, "the rotation left {live} live sessions");

    // (d) A revoke that lands between the claim and the settle must survive a failed refresh. The
    // trigger stands in for a logout at exactly that point (it flips a freshly claimed row to 1
    // inside the claim statement, so nothing is raced); a settle that handed the claim back
    // blindly, `SET revoked = 0 WHERE id = ?`, would revive the session here.
    let third = results
        .iter()
        .find(|(status, _)| *status == 200)
        .map(|(_, body)| body["refresh_token"].as_str().unwrap().to_string())
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER revoked_in_flight AFTER UPDATE OF revoked ON sessions \
         WHEN NEW.revoked = 2 BEGIN UPDATE sessions SET revoked = 1 WHERE id = NEW.id; END",
    )
    .execute(&state.any)
    .await
    .unwrap();
    std::env::remove_var("AGENT_PLATFORM_JWT_SECRET");
    let (status, body) = refresh(&origin, &third).await;
    assert_eq!(status, 503, "{body}");
    assert_eq!(
        revoked(&state, &third).await,
        1,
        "a failed refresh revived a session that was revoked mid-flight"
    );
    sqlx::query("DROP TRIGGER revoked_in_flight")
        .execute(&state.any)
        .await
        .unwrap();
    std::env::set_var("AGENT_PLATFORM_JWT_SECRET", SECRET);
    let (status, body) = refresh(&origin, &third).await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body["error"]["code"], "SESSION_REVOKED");

    // (e) Revoke-all must take a session whose refresh holds the claim too, or that refresh's
    // settle would hand it back. Seed one at 2, as if a refresh were in flight.
    sqlx::query(
        "INSERT INTO sessions (user_id, refresh_token_hash, expires_at, revoked, created_at) \
         SELECT id, ?, '2099-01-01 00:00:00.000000', 2, '2026-01-01 00:00:00.000000' \
         FROM users WHERE email = 'rotate@example.com'",
    )
    .bind(hash_token("in-flight-refresh-token"))
    .execute(&state.any)
    .await
    .unwrap();
    let taken = revoke_sessions(&state, "rotate@example.com").await.unwrap();
    assert_eq!(
        taken, 1,
        "revoke-all skipped a session whose refresh holds the claim"
    );
    assert_eq!(revoked(&state, "in-flight-refresh-token").await, 1);

    // (f) #1098: a revoke that lands while a *successful* refresh mints must take the session it
    // mints too, or the account keeps one live session the admin just revoked. Same trigger as (d)
    // standing in for revoke-all at that instant, but this time the mint succeeds.
    sqlx::query(
        "INSERT INTO sessions (user_id, refresh_token_hash, expires_at, revoked, created_at) \
         SELECT id, ?, '2099-01-01 00:00:00.000000', 0, '2026-01-01 00:00:00.000000' \
         FROM users WHERE email = 'rotate@example.com'",
    )
    .bind(hash_token("revoked-while-minting"))
    .execute(&state.any)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER revoked_mid_mint AFTER UPDATE OF revoked ON sessions \
         WHEN NEW.revoked = 2 BEGIN UPDATE sessions SET revoked = 1 WHERE id = NEW.id; END",
    )
    .execute(&state.any)
    .await
    .unwrap();
    let (status, body) = refresh(&origin, "revoked-while-minting").await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body["error"]["code"], "SESSION_REVOKED");
    let live: i64 =
        sqlx::query_scalar("SELECT CAST(COUNT(*) AS BIGINT) FROM sessions WHERE revoked = 0")
            .fetch_one(&state.any)
            .await
            .unwrap();
    assert_eq!(live, 0, "a refresh revoked mid-mint left {live} live sessions");

    let _ = std::fs::remove_file(&db);
}
