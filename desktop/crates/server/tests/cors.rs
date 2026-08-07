//! The check that fails if browser callers stop being able to reach this server
//! — or if they start being able to reach it when nobody asked.
//!
//! Its own test binary on purpose: `AGENT_PLATFORM_CORS_ORIGINS` is read once
//! per `router()` call out of the process environment, and setting it from one
//! test in a shared binary would decide the answer for every other test running
//! beside it.

use std::path::PathBuf;
use std::sync::Arc;

use agent_platform_server::{router, AppState};

const ALLOWED: &str = "http://localhost:5173";

fn temp_db_path() -> PathBuf {
    std::env::temp_dir().join(format!("agp-cors-test-{}.db", std::process::id()))
}

async fn start_server() -> String {
    let db = temp_db_path();
    let _ = std::fs::remove_file(&db);
    let state = Arc::new(AppState::new(&db, Some("master-key-under-test".into())));
    agent_platform_server::db::ensure_schema(&state.any).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });
    origin
}

#[tokio::test]
async fn cors_off_by_default_then_on_with_the_env_var() {
    // Both halves in one test: the env var is process-global, so two `#[test]`s
    // in this binary would race on it.
    std::env::remove_var("AGENT_PLATFORM_CORS_ORIGINS");
    let origin = start_server().await;
    let client = reqwest::Client::new();

    let res = client
        .get(format!("{origin}/health"))
        .header("Origin", ALLOWED)
        .send()
        .await
        .unwrap();
    assert!(
        res.headers().get("access-control-allow-origin").is_none(),
        "unset env var must mean no CORS headers at all"
    );

    std::env::set_var("AGENT_PLATFORM_CORS_ORIGINS", format!("{ALLOWED}, https://app.example.com"));
    let origin = start_server().await;

    // Preflight: no `Authorization` header, so this is the request that proves
    // CORS sits outside the auth layer rather than behind it.
    let res = client
        .request(reqwest::Method::OPTIONS, format!("{origin}/api/v1/workspaces/"))
        .header("Origin", ALLOWED)
        .header("Access-Control-Request-Method", "GET")
        .header("Access-Control-Request-Headers", "authorization")
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "preflight got {} — auth ate it", res.status());
    assert_eq!(
        res.headers().get("access-control-allow-origin").unwrap(),
        ALLOWED,
        "preflight must echo the allowed origin"
    );

    // A real request from an origin nobody allowed gets no header, so the
    // browser refuses to hand the response to the page.
    let res = client
        .get(format!("{origin}/health"))
        .header("Origin", "https://evil.example.com")
        .send()
        .await
        .unwrap();
    assert!(
        res.headers().get("access-control-allow-origin").is_none(),
        "an unlisted origin must not be echoed back"
    );
}
