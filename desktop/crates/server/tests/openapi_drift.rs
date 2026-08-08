//! The check that fails when `openapi.json` describes a route this server no
//! longer serves.
//!
//! `openapi.json` is a checked-in file. FastAPI generated it from the route
//! declarations while the server was Python; axum cannot enumerate its own
//! router, so keeping it means either annotating 141 paths with `utoipa` or
//! maintaining the document by hand. The document won — and the note on
//! `lib.rs::openapi` says, correctly, that it will drift and nothing detects
//! that. This is the cheap half of detecting it.
//!
//! **It checks one direction only.** Every operation the document declares must
//! reach a handler; a route that exists but is *undocumented* is invisible here,
//! because there is no way to ask an axum `Router` what it serves. That
//! remaining gap is what `utoipa` would close, and it is worth its cost the
//! first time an undocumented route actually matters.
//!
//! What counts as reaching a handler: anything except this server's own
//! fallback. A 401, 404-from-a-handler, 422 or 500 all mean routing found
//! something — only `lib.rs::not_found` writes "No route for …".

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_platform_server::{db, router, AppState};
use serde_json::Value;
use tower::ServiceExt;

const OPENAPI: &str = include_str!("../src/openapi.json");

/// The LLM proxy is skipped: its handlers call an upstream provider, and a test
/// that waits on nine outbound HTTP attempts to prove nine routes are mounted is
/// paying far too much for the answer. They are covered by name in
/// `auth_and_routing::llm_proxy_routes_authenticate_per_route`.
fn skipped(path: &str) -> bool {
    !path.starts_with("/api/v1/")
}

#[tokio::test]
async fn every_documented_operation_reaches_a_handler() {
    let db_path: PathBuf =
        std::env::temp_dir().join(format!("agp-openapi-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db_path);

    // A real schema, so a handler that runs fails on its own terms rather than
    // on a missing table — the assertion does not care, but a 500 storm in the
    // captured output makes a genuine failure much harder to read.
    let state = Arc::new(AppState::new(&db_path, None));
    db::ensure_schema(&state.any).await.expect("schema");
    let app = router(state);

    let doc: Value = serde_json::from_str(OPENAPI).expect("openapi.json parses");
    let paths = doc["paths"].as_object().expect("paths object");

    let mut checked = 0usize;
    let mut missing: Vec<String> = Vec::new();

    for (path, item) in paths {
        if skipped(path) {
            continue;
        }
        for method in ["get", "post", "put", "patch", "delete"] {
            if item.get(method).is_none() {
                continue;
            }
            checked += 1;

            // `{project_id}` → `1`. Every path parameter here is an id or a
            // name, and both accept it — the request only has to route.
            let concrete = substitute(path);
            let req = axum::http::Request::builder()
                .method(method.to_uppercase().as_str())
                .uri(&concrete)
                // An empty JSON body, so a handler with a `Json<T>` extractor
                // rejects at 422 instead of running. Routing has already
                // happened by then, which is all this asserts.
                .header("content-type", "application/json")
                .body(axum::body::Body::from("{}"))
                .unwrap();

            // Some handlers stream (SSE) or reach for a provider that is not
            // running. Either way the route exists — that is the answer this
            // test wanted, and waiting for the rest of it is waste.
            let Ok(response) = tokio::time::timeout(
                Duration::from_secs(3),
                app.clone().oneshot(req),
            )
            .await
            else {
                continue;
            };
            let response = response.unwrap();

            if response.status() != 404 {
                continue;
            }
            let body = axum::body::to_bytes(response.into_body(), 64 * 1024).await.unwrap();
            let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            if parsed["error"]["message"]
                .as_str()
                .is_some_and(|m| m.starts_with("No route for"))
            {
                missing.push(format!("{} {path}", method.to_uppercase()));
            }
        }
    }

    let _ = std::fs::remove_file(&db_path);

    assert!(checked > 150, "only {checked} operations checked; did the document shrink?");
    assert!(
        missing.is_empty(),
        "openapi.json documents {} operation(s) this server does not serve:\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
}

/// The collection paths answer with and without their trailing slash.
///
/// FastAPI answered the bare form with a 307 onto the slashed one, so two
/// routers here registered only the slashed form and let the bare one fall
/// through to the proxy. Deleting the proxy turned those into 404s, and nothing
/// noticed — `openapi.json` documents only the slashed spelling, so the drift
/// test above cannot see it either.
#[tokio::test]
async fn collection_paths_answer_with_and_without_the_trailing_slash() {
    let db_path: PathBuf =
        std::env::temp_dir().join(format!("agp-slash-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db_path);
    let state = Arc::new(AppState::new(&db_path, None));
    db::ensure_schema(&state.any).await.expect("schema");
    let app = router(state);

    for path in [
        "/api/v1/workspaces",
        "/api/v1/workspaces/",
        "/api/v1/workspaces/1/api-tokens",
        "/api/v1/workspaces/1/api-tokens/",
        // Already correct, and here so a regression in the one router that got
        // this right from the start fails with the other two.
        "/api/v1/projects",
        "/api/v1/projects/",
    ] {
        let req = axum::http::Request::get(path).body(axum::body::Body::empty()).unwrap();
        let response = app.clone().oneshot(req).await.unwrap();
        // Not `== 200`: `/workspaces/1/api-tokens` 404s on its own terms here,
        // because workspace 1 does not exist in an empty database. What is
        // under test is that routing found a handler at all.
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        assert!(
            !parsed["error"]["message"]
                .as_str()
                .is_some_and(|m| m.starts_with("No route for")),
            "GET {path} hit the fallback"
        );
    }

    let _ = std::fs::remove_file(&db_path);
}

fn substitute(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut depth = 0usize;
    for c in path.chars() {
        match c {
            '{' => {
                if depth == 0 {
                    out.push('1');
                }
                depth += 1;
            }
            '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

#[test]
fn path_parameters_are_replaced_and_the_rest_is_left_alone() {
    assert_eq!(substitute("/api/v1/projects"), "/api/v1/projects");
    assert_eq!(substitute("/api/v1/projects/{project_id}"), "/api/v1/projects/1");
    assert_eq!(
        substitute("/api/v1/projects/{project_id}/workspace/file"),
        "/api/v1/projects/1/workspace/file"
    );
}
