//! `/api/v1/media/*` end to end against a stub ComfyUI (ADR 0009).
//!
//! **Why this is an integration test and not a unit one.** `media.rs`'s pure
//! parts — template filling, output picking, error extraction — are unit
//! tested in the module. What they cannot cover is the `INSERT`, and the
//! first live run of this feature failed on exactly that: the statement named
//! eleven columns and bound ten values, so every column shifted by one and
//! `prompt` landed in `kind` while `NOT NULL prompt` took a null. Nothing but
//! a real database and a real row could have caught it, which is what this
//! does.
//!
//! The stub answers the four ComfyUI routes the module calls, and reports a
//! job as unfinished on its first poll — so the running → completed
//! transition is exercised rather than only the settled end state.

mod common;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use axum::extract::Query;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use common::{start_server, temp_db_path, MASTER};

/// A 1×1 PNG — the smallest thing that proves the bytes made the round trip
/// from the stub, through the server's media folder, back out of the file
/// route unchanged.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

/// How many times `/history` has been asked about the one job, so the stub can
/// answer "not yet" before it answers "done".
static POLLS: AtomicU32 = AtomicU32::new(0);

/// The graph the server actually submitted, kept so the test can assert the
/// prompt and the resolved checkpoint reached ComfyUI.
static SUBMITTED: std::sync::Mutex<Option<Value>> = std::sync::Mutex::new(None);

async fn stub_comfyui() -> String {
    let app = Router::new()
        .route("/system_stats", get(|| async { Json(json!({ "system": { "comfyui_version": "stub" } })) }))
        .route(
            "/object_info/CheckpointLoaderSimple",
            get(|| async {
                Json(json!({
                    "CheckpointLoaderSimple": {
                        "input": { "required": { "ckpt_name": [["other.safetensors", "flux2-klein.safetensors"]] } }
                    }
                }))
            }),
        )
        .route(
            "/prompt",
            post(|Json(body): Json<Value>| async move {
                *SUBMITTED.lock().unwrap() = body.get("prompt").cloned();
                Json(json!({ "prompt_id": "stub-1", "number": 1, "node_errors": {} }))
            }),
        )
        .route(
            "/history/{prompt_id}",
            get(|axum::extract::Path(id): axum::extract::Path<String>| async move {
                if POLLS.fetch_add(1, Ordering::Relaxed) == 0 {
                    // Still rendering — the state the desktop polls through.
                    return Json(json!({ id: { "status": { "completed": false }, "outputs": {} } }));
                }
                Json(json!({
                    id: {
                        "status": { "completed": true, "status_str": "success" },
                        "outputs": { "9": { "images": [
                            { "filename": "agent-platform_00001_.png", "subfolder": "", "type": "output" }
                        ]}}
                    }
                }))
            }),
        )
        .route(
            "/view",
            get(|Query(_q): Query<std::collections::HashMap<String, String>>| async {
                ([(axum::http::header::CONTENT_TYPE, "image/png")], PNG)
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

fn authed(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    request.header("Authorization", format!("Bearer {MASTER}"))
}

/// The whole lifecycle, in one test because it is one story and each step
/// needs the state the previous one left: probe → generate → poll → fetch.
///
/// Serial by necessity — the stub's counters and `MEDIA_*` process
/// environment are global — which is also why the crate has exactly one test
/// of this shape rather than four sharing a fixture.
#[tokio::test]
async fn a_generation_runs_from_probe_to_finished_bytes() {
    let comfy = stub_comfyui().await;
    let db = temp_db_path("media");
    let media_dir = std::env::temp_dir().join(format!("agp-media-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&media_dir);

    std::env::set_var("MEDIA_API_BASE", &comfy);
    std::env::set_var("MEDIA_DATA_DIR", &media_dir);

    // The schema, which `AppState::new` does not run — this suite's other
    // tests either seed their own or never reach a table.
    let state = Arc::new(agent_platform_server::AppState::new(&db, Some(MASTER.to_string())));
    agent_platform_server::db::ensure_schema(&state.any).await.unwrap();
    drop(state);

    let origin = start_server(&db, Some(MASTER)).await;
    let http = reqwest::Client::new();

    // -- the probe ----------------------------------------------------------
    let status: Value = authed(http.get(format!("{origin}/api/v1/media/status")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["reachable"], json!(true));
    assert_eq!(
        status["image_model"],
        json!("flux2-klein.safetensors"),
        "a known family must win over the alphabetically-first checkpoint"
    );

    // -- generate -----------------------------------------------------------
    let created: Value = authed(http.post(format!("{origin}/api/v1/media/generate")))
        .json(&json!({ "kind": "image", "prompt": "a red apple", "width": 1000, "height": 768 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // The regression this file exists for: every column holds its own value.
    assert_eq!(created["kind"], json!("image"), "the row's columns must not be shifted");
    assert_eq!(created["prompt"], json!("a red apple"));
    assert_eq!(created["status"], json!("running"));
    assert_eq!(created["width"], json!(992), "1000 snaps down to a multiple of 16");
    assert_eq!(created["height"], json!(768));
    assert_eq!(created["length"], json!(0), "an image carries no frame count");
    let id = created["id"].as_i64().unwrap();

    // What ComfyUI was actually handed.
    let graph = SUBMITTED.lock().unwrap().clone().expect("a workflow was submitted");
    assert_eq!(graph["6"]["inputs"]["text"], json!("a red apple"));
    assert_eq!(graph["4"]["inputs"]["ckpt_name"], json!("flux2-klein.safetensors"));
    assert_eq!(graph["5"]["inputs"]["width"], json!(992));

    // -- the watcher finishes it -------------------------------------------
    let mut finished = None;
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let job: Value = authed(http.get(format!("{origin}/api/v1/media/jobs/{id}")))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if job["status"] != json!("running") && job["status"] != json!("queued") {
            finished = Some(job);
            break;
        }
    }
    let job = finished.expect("the job settled within ten seconds");
    assert_eq!(job["status"], json!("completed"), "job ended: {job}");
    assert_eq!(
        job["file_name"],
        json!(format!("{id}_agent-platform_00001_.png")),
        "the saved name is prefixed with the job id"
    );
    assert!(POLLS.load(Ordering::Relaxed) >= 2, "the unfinished poll must have been tolerated");

    // -- the bytes come back ------------------------------------------------
    let response = authed(http.get(format!("{origin}/api/v1/media/jobs/{id}/file")))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "image/png");
    assert_eq!(response.bytes().await.unwrap().as_ref(), PNG, "the file must survive the round trip");

    // -- the list -----------------------------------------------------------
    let list: Value = authed(http.get(format!("{origin}/api/v1/media/jobs")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list["jobs"].as_array().unwrap().len(), 1);

    let _ = std::fs::remove_dir_all(&media_dir);
    let _ = std::fs::remove_file(&db);
}
