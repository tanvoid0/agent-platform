//! Launching a **real** `sd-server` (ADR 0011, step 2).
//!
//! Ignored by default and gated on a binary the runner supplies, the same shape
//! as the local-inference check in `desktop/CLAUDE.md`:
//!
//! ```text
//! AGENT_PLATFORM_TEST_SDSERVER=<path to sd-server.exe> \
//!   cargo test -p agent-platform-server --test media_sdcpp_spawn -- --ignored
//! ```
//!
//! What it pins is the failure path, because that is the one that was wrong
//! before it was measured. `sd-server` given a model path that does not exist
//! **exits in under a second** rather than starting and serving an error, so a
//! health check that only polled the port would sit there for the full
//! five-minute start timeout before reporting a typo. This asserts the wait
//! ends promptly *and* that sd-server's own words reach the caller.
//!
//! The success path is not covered here: it needs multi-gigabyte weights that
//! no test should download. `media_sdcpp_routes.rs` covers the wire contract
//! against a stub instead.

mod common;

use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};

use common::{start_server, temp_db_path, MASTER};

/// A loopback base with nothing behind it, so `ensure_running` has to launch.
async fn dead_loopback_base() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}")
}

#[tokio::test]
#[ignore = "needs a real sd-server binary in AGENT_PLATFORM_TEST_SDSERVER"]
async fn a_bad_model_path_fails_fast_in_sd_servers_own_words() {
    let Ok(binary) = std::env::var("AGENT_PLATFORM_TEST_SDSERVER") else {
        panic!("set AGENT_PLATFORM_TEST_SDSERVER to an sd-server binary");
    };
    assert!(
        std::path::Path::new(&binary).is_file(),
        "AGENT_PLATFORM_TEST_SDSERVER={binary} is not a file"
    );

    let base = dead_loopback_base().await;
    let db = temp_db_path("media-spawn");
    let media_dir = std::env::temp_dir().join(format!("agp-media-spawn-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&media_dir);

    std::env::set_var("MEDIA_BACKEND", "sdcpp");
    std::env::set_var("MEDIA_API_BASE", &base);
    std::env::set_var("MEDIA_DATA_DIR", &media_dir);
    std::env::set_var("MEDIA_SDCPP_BIN", &binary);
    // Configured, so there is something to launch — and wrong, so it dies.
    std::env::set_var("MEDIA_SDCPP_ARGS", "--diffusion-model C:\\nope\\missing-model.gguf");

    let state = Arc::new(agent_platform_server::AppState::new(&db, Some(MASTER.to_string())));
    agent_platform_server::db::ensure_schema(&state.any).await.unwrap();
    drop(state);

    let origin = start_server(&db, Some(MASTER)).await;
    let http = reqwest::Client::new();

    let began = Instant::now();
    let response = http
        .post(format!("{origin}/api/v1/media/generate"))
        .header("Authorization", format!("Bearer {MASTER}"))
        .json(&json!({ "kind": "image", "prompt": "a red apple" }))
        .send()
        .await
        .unwrap();
    let elapsed = began.elapsed();

    assert_eq!(response.status(), 502);
    let body: Value = response.json().await.unwrap();
    let rendered = body.to_string();

    // The measured behaviour: sd-server prints `error: the following arguments
    // are required: model_path/diffusion_model` and exits. Matching on "exited"
    // rather than on its exact wording, which is upstream's to change.
    assert!(
        rendered.contains("exited"),
        "the error must say the process died, not time out: {rendered}"
    );
    // sd-server's wording for this case is `get sd version from file failed` /
    // `new_sd_ctx_t failed`, both on `[ERROR]` lines. Asserting on the marker
    // rather than the prose, which is upstream's to reword.
    assert!(
        rendered.contains("[ERROR]"),
        "sd-server's own reason must survive into the response: {rendered}"
    );
    assert!(
        !rendered.contains("Vulkan devices"),
        "the startup banner must not be quoted as the reason: {rendered}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(60),
        "a dead child must end the wait promptly, not run out the start timeout \
         (took {elapsed:?})"
    );

    // And the status route agrees, so the Studio screen shows the same reason.
    let status: Value = http
        .get(format!("{origin}/api/v1/media/status"))
        .header("Authorization", format!("Bearer {MASTER}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["backend_stage"], json!("failed"), "status: {status}");
    assert!(
        status["backend_detail"].as_str().is_some_and(|d| d.contains("exited")),
        "the failure detail must reach the status route: {status}"
    );

    let _ = std::fs::remove_dir_all(&media_dir);
}
