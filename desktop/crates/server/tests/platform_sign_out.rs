//! Sign-out must revoke the live refresh token, not the one a refresh is rotating away from, and
//! must not leave a session behind: (a) a refresh in flight when sign-out lands, (b) a rotation the
//! daemon could not write to the session file (#1128).
//!
//! Its own `tests/*.rs` binary because it owns the process-wide `AGENT_PLATFORM_CLOUD_SESSION`.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_platform_server::accounts::{ensure_platform_access, sign_out_platform, SESSION_ENV};
use agent_platform_server::AppState;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

use common::temp_db_path;

#[derive(Default)]
struct Origin {
    refreshes: Mutex<u32>,
    logouts: Mutex<Vec<String>>,
}

async fn refresh(State(origin): State<Arc<Origin>>) -> Json<Value> {
    let n = {
        let mut n = origin.refreshes.lock().unwrap();
        *n += 1;
        *n
    };
    // Long enough for sign-out to start while this rotation is in flight.
    tokio::time::sleep(Duration::from_millis(200)).await;
    Json(json!({ "access_token": format!("access-{n}"), "refresh_token": format!("refresh-{n}"), "expires_in": 900 }))
}

async fn logout(State(origin): State<Arc<Origin>>, Json(body): Json<Value>) -> Json<Value> {
    origin.logouts.lock().unwrap().push(body["refresh_token"].as_str().unwrap_or_default().into());
    Json(json!({ "ok": true }))
}

async fn origin(session_file: &std::path::Path) -> Arc<Origin> {
    let origin = Arc::new(Origin::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/accounts/api/v1/auth/refresh", post(refresh))
        .route("/accounts/api/v1/auth/logout", post(logout))
        .with_state(origin.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let session = json!({ "url": url, "refresh_token": "refresh-0", "access_expires_at": 0 });
    std::fs::write(session_file, session.to_string()).unwrap();
    origin
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sign_out_revokes_the_live_token_and_leaves_no_session() {
    let session_file = temp_db_path("platform-sign-out").with_extension("json");
    std::env::set_var(SESSION_ENV, &session_file);
    let state = Arc::new(AppState::new(&temp_db_path("platform-sign-out"), None));

    // (a) Sign-out lands while a refresh is rotating refresh-0 to refresh-1.
    let origin_a = origin(&session_file).await;
    let refreshing = {
        let state = state.clone();
        tokio::spawn(async move { ensure_platform_access(&state).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    sign_out_platform(&state).await.unwrap();
    refreshing.await.unwrap().unwrap();
    assert_eq!(*origin_a.logouts.lock().unwrap(), vec!["refresh-1".to_string()]);
    assert!(!session_file.exists(), "the in-flight refresh wrote the session back");

    // (b) The rotation could not be written (a directory squats on the temp path), so refresh-1
    // lives only in memory. Sign-out must revoke it, not the dead refresh-0 still on disk.
    let origin_b = origin(&session_file).await;
    let tmp = session_file.with_extension("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    ensure_platform_access(&state).await.unwrap();
    sign_out_platform(&state).await.unwrap();
    assert_eq!(*origin_b.logouts.lock().unwrap(), vec!["refresh-1".to_string()]);
    assert!(!session_file.exists());

    let _ = std::fs::remove_dir_all(&tmp);
}
