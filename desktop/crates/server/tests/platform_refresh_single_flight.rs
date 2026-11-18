//! Board #1097: `ensure_platform_access` is the only client of `/auth/refresh`, and the origin now
//! rotates the refresh token (#1081: the first POST of a token wins, a replay is SESSION_REVOKED).
//! Callers that overlap at access-token expiry must share one refresh instead of each POSTing the
//! token in the session file.
//!
//! One test, because it owns the process-wide env (`AGENT_PLATFORM_CLOUD_SESSION`, the session
//! file the daemon reads) — which is also why it is its own `tests/*.rs` binary.

mod common;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_platform_server::accounts::{
    cached_platform_access, ensure_platform_access, SESSION_ENV,
};
use agent_platform_server::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

use common::temp_db_path;

const CALLERS: usize = 6;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// The #1081 origin: a refresh token is honoured once and rotated, a replay is SESSION_REVOKED.
    Rotate,
    /// Every refresh token is revoked.
    Revoked,
    /// Another process refreshed first: it rewrites the session file, then this POST is SESSION_REVOKED.
    Raced,
}

struct Origin {
    mode: Mode,
    posts: AtomicUsize,
    honoured: Mutex<HashSet<String>>,
    session_file: PathBuf,
}

fn revoked() -> (StatusCode, Json<Value>) {
    let error = json!({ "code": "SESSION_REVOKED", "message": "Refresh token expired." });
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": error })))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

async fn refresh(
    State(origin): State<Arc<Origin>>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let n = origin.posts.fetch_add(1, Ordering::SeqCst) + 1;
    // Long enough that every overlapping caller is in flight before the first answer lands.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let token = body["refresh_token"].as_str().unwrap_or_default();
    let honoured = origin.mode == Mode::Rotate && origin.honoured.lock().unwrap().remove(token);
    if honoured {
        let next = format!("refresh-{n}");
        origin.honoured.lock().unwrap().insert(next.clone());
        let user = json!({ "email": "rotate@example.com", "entitlement": "trial" });
        let body = json!({
            "access_token": format!("access-{n}"),
            "refresh_token": next,
            "expires_in": 900,
            "user": user,
        });
        return (StatusCode::OK, Json(body));
    }
    if origin.mode == Mode::Raced && n == 1 {
        let raw = std::fs::read_to_string(&origin.session_file).unwrap();
        let mut session: Value = serde_json::from_str(&raw).unwrap();
        session["refresh_token"] = json!("other-process-refresh");
        session["access_token"] = json!("access-other-process");
        session["access_expires_at"] = json!(now() + 900);
        std::fs::write(&origin.session_file, session.to_string()).unwrap();
    }
    revoked()
}

/// A stub origin in `mode`, a session file that points at it with an expired access token, and an
/// `AppState` with an empty cache: everything one scenario needs.
async fn scenario(mode: Mode, session_file: &Path) -> (Arc<AppState>, Arc<Origin>) {
    let origin = Arc::new(Origin {
        mode,
        posts: AtomicUsize::new(0),
        honoured: Mutex::new(HashSet::from(["refresh-0".to_string()])),
        session_file: session_file.to_path_buf(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/accounts/api/v1/auth/refresh", post(refresh))
        .with_state(origin.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let session = json!({
        "url": url,
        "refresh_token": "refresh-0",
        "access_token": "access-expired",
        "access_expires_at": 0,
        "email": "rotate@example.com",
        "entitlement": "trial",
    });
    std::fs::write(session_file, session.to_string()).unwrap();
    let state = AppState::new(&temp_db_path("platform-refresh"), None);
    (Arc::new(state), origin)
}

/// `CALLERS` calls at once, each on its own task, each seeing the same expired session.
async fn overlapping_callers(state: &Arc<AppState>) -> Vec<Result<String, String>> {
    let calls = (0..CALLERS).map(|_| {
        let state = state.clone();
        tokio::spawn(async move { ensure_platform_access(&state).await })
    });
    let results = futures::future::join_all(calls).await;
    results
        .into_iter()
        .map(|r| {
            r.unwrap()
                .map_err(|e| format!("{} {}: {}", e.status.as_u16(), e.code, e.message))
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlapping_callers_share_one_refresh() {
    let session_file = temp_db_path("platform-refresh").with_extension("json");
    std::env::set_var(SESSION_ENV, &session_file);

    // (a) The origin honours a refresh token once: all six callers must end up with the one
    // rotated access token, from one POST, and the rotated refresh token must be what is on disk.
    let (state, origin) = scenario(Mode::Rotate, &session_file).await;
    let results = overlapping_callers(&state).await;
    let posts = origin.posts.load(Ordering::SeqCst);
    assert_eq!(
        results,
        vec![Ok("access-1".to_string()); CALLERS],
        "overlapping callers POSTed the same refresh token {posts} times"
    );
    assert_eq!(posts, 1, "one refresh was enough for every caller");
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(&session_file).unwrap()).unwrap();
    assert_eq!(saved["refresh_token"], "refresh-1");
    assert_eq!(saved["access_token"], "access-1");
    assert_eq!(cached_platform_access(&state).unwrap(), "access-1");

    // (b) A refresh token that is truly revoked must still surface as an expired session to every
    // caller, leave the session file alone, and not wedge the lock: each caller got its turn.
    let (state, origin) = scenario(Mode::Revoked, &session_file).await;
    let before = std::fs::read_to_string(&session_file).unwrap();
    let results = overlapping_callers(&state).await;
    let expired = Err("401 platform_session_expired: Refresh token expired.".to_string());
    assert_eq!(results, vec![expired; CALLERS]);
    assert_eq!(
        std::fs::read_to_string(&session_file).unwrap(),
        before,
        "a revoked token rewrote the session file"
    );
    assert_eq!(
        origin.posts.load(Ordering::SeqCst),
        CALLERS,
        "a failed refresh must let the next caller retry"
    );

    // (c) Another process refreshed first: the origin answers SESSION_REVOKED, but the session file
    // now holds a different refresh token with a live access token, and that is the one to use.
    let (state, origin) = scenario(Mode::Raced, &session_file).await;
    let results = overlapping_callers(&state).await;
    assert_eq!(
        results,
        vec![Ok("access-other-process".to_string()); CALLERS]
    );
    assert_eq!(origin.posts.load(Ordering::SeqCst), 1);
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(&session_file).unwrap()).unwrap();
    assert_eq!(
        saved["refresh_token"], "other-process-refresh",
        "the other process's tokens were overwritten"
    );

    let _ = std::fs::remove_file(&session_file);
}
