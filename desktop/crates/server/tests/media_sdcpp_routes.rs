//! `/api/v1/media/*` end to end against a stub `sd-server` (ADR 0011).
//!
//! **Its own file, not a second test in `media_routes.rs`.** Both suites drive
//! the module through the `MEDIA_*` *process* environment, and `MEDIA_BACKEND`
//! is exactly the variable they would disagree about. Cargo runs each
//! integration test file as its own process, which is the cheapest real
//! isolation available here — sharing the file would mean serialising two
//! tests around a global, which is what the note at the top of `media_routes`
//! already warns about.
//!
//! What this covers that the unit tests in `media_sdcpp.rs` cannot: the
//! backend selector actually routing to this adapter, the row landing with its
//! columns in the right order, and base64 in the poll body coming back out of
//! the file route as the same bytes.

mod common;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use common::{start_server, temp_db_path, MASTER};

/// A 1×1 PNG, and the same bytes base64'd the way `sd-server` returns them.
/// Both spellings are here on purpose: the test asserts the round trip from
/// the encoded form back to the raw one.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];
const PNG_B64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAACklEQVR4nGMAAQAABQABDQottAAAAABJRU5ErkJggg==";

/// How many times the one job has been polled, so the stub can answer
/// `generating` before it answers `completed`.
static POLLS: AtomicU32 = AtomicU32::new(0);

/// The body the server actually submitted, kept so the test can assert both
/// what was sent and — just as deliberately — what was not.
static SUBMITTED: std::sync::Mutex<Option<Value>> = std::sync::Mutex::new(None);

async fn stub_sd_server() -> String {
    let app = Router::new()
        .route(
            "/sdcpp/v1/capabilities",
            get(|| async {
                Json(json!({
                    "model": { "name": "z_image_turbo_bf16.safetensors", "stem": "z_image_turbo_bf16" },
                    "current_mode": "img_gen",
                    "supported_modes": ["img_gen", "vid_gen"],
                    "samplers": ["euler_a"],
                    "schedulers": ["discrete"],
                }))
            }),
        )
        .route(
            "/sdcpp/v1/img_gen",
            post(|Json(body): Json<Value>| async move {
                *SUBMITTED.lock().unwrap() = Some(body);
                (
                    axum::http::StatusCode::ACCEPTED,
                    Json(json!({
                        "id": "job_01HTXYZABC",
                        "kind": "img_gen",
                        "status": "queued",
                        "created": 1775401200,
                        "poll_url": "/sdcpp/v1/jobs/job_01HTXYZABC",
                    })),
                )
            }),
        )
        .route(
            "/sdcpp/v1/jobs/{id}",
            get(|axum::extract::Path(id): axum::extract::Path<String>| async move {
                if POLLS.fetch_add(1, Ordering::Relaxed) == 0 {
                    // Mid-render — the state the desktop polls through, and the
                    // one a watcher that treats "not completed" as an error
                    // would get wrong.
                    return Json(json!({
                        "id": id, "kind": "img_gen", "status": "generating",
                        "queue_position": 0, "result": null, "error": null,
                    }));
                }
                Json(json!({
                    "id": id,
                    "kind": "img_gen",
                    "status": "completed",
                    "result": {
                        "output_format": "png",
                        "images": [{ "index": 0, "b64_json": PNG_B64 }],
                    },
                    "error": null,
                }))
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

/// A loopback base with certainly nothing behind it: bind a port, read it back,
/// then drop the listener. Beats picking a number and hoping — a hard-coded
/// port that something else happens to hold turns this into a flake that only
/// fails on one developer's machine.
async fn dead_loopback_base() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}")
}

/// Probe → generate → poll → bytes, in one test because it is one story and
/// each step needs the state the previous one left.
#[tokio::test]
async fn a_generation_runs_through_the_sdcpp_backend() {
    let sd = stub_sd_server().await;
    let db = temp_db_path("media-sdcpp");
    let media_dir = std::env::temp_dir().join(format!("agp-media-sdcpp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&media_dir);

    std::env::set_var("MEDIA_BACKEND", "sdcpp");
    std::env::set_var("MEDIA_API_BASE", &sd);
    std::env::set_var("MEDIA_DATA_DIR", &media_dir);

    let state = Arc::new(agent_platform_server::AppState::new(&db, Some(MASTER.to_string())));
    agent_platform_server::db::ensure_schema(&state.any).await.unwrap();
    drop(state);

    let origin = start_server(&db, Some(MASTER)).await;
    let http = reqwest::Client::new();

    // -- nothing listening, and no model flags: a named error ---------------
    //
    // The state a fresh sdcpp install is actually in. It must be a sentence
    // the user can act on, and it must NOT quietly start a download: with no
    // `MEDIA_SDCPP_ARGS` there is nothing to launch even once the binary is
    // fetched, so fetching first would spend 39 MB to arrive at the same error.
    {
        let dead = dead_loopback_base().await;
        std::env::set_var("MEDIA_API_BASE", &dead);
        let response = authed(http.post(format!("{origin}/api/v1/media/generate")))
            .json(&json!({ "kind": "image", "prompt": "a red apple" }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 502, "an unlaunchable backend is a 502, not a hang");
        let body: Value = response.json().await.unwrap();
        let rendered = body.to_string();
        assert!(
            rendered.contains("media_backend_unconfigured"),
            "the error must name the missing configuration, not just 'unreachable': {rendered}"
        );
        assert!(
            rendered.contains("MEDIA_SDCPP_ARGS"),
            "the error must name the variable to set: {rendered}"
        );
        assert!(SUBMITTED.lock().unwrap().is_none(), "nothing should have been submitted");
        std::env::set_var("MEDIA_API_BASE", &sd);
    }

    // -- the probe ----------------------------------------------------------
    let status: Value = authed(http.get(format!("{origin}/api/v1/media/status")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["reachable"], json!(true));
    assert_eq!(status["backend"], json!("sdcpp"), "the selector must route to this adapter");
    assert_eq!(
        status["image_model"],
        json!("z_image_turbo_bf16"),
        "sd-server's one loaded model is the image model, stem preferred over file name"
    );
    assert_eq!(
        status["modes"],
        json!(["img_gen", "vid_gen"]),
        "what the loaded model can be asked for must reach the desktop"
    );

    // -- requirements: nothing to fetch, because the model is already bound --
    let requirements: Value = authed(http.get(format!("{origin}/api/v1/media/requirements")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(requirements["items"], json!([]), "sd-server has no shopping list");

    // -- generate -----------------------------------------------------------
    let created: Value = authed(http.post(format!("{origin}/api/v1/media/generate")))
        .json(&json!({ "kind": "image", "prompt": "a red apple", "width": 1000, "height": 768 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // The same column-order regression `media_routes.rs` exists for, on the
    // path that does not build a graph.
    assert_eq!(created["kind"], json!("image"), "the row's columns must not be shifted");
    assert_eq!(created["prompt"], json!("a red apple"));
    assert_eq!(created["status"], json!("running"));
    assert_eq!(created["width"], json!(992), "1000 snaps down to a multiple of 16");
    assert_eq!(created["length"], json!(0), "an image carries no frame count");
    let id = created["id"].as_i64().unwrap();

    // What sd-server was handed.
    let sent = SUBMITTED.lock().unwrap().clone().expect("a request was submitted");
    assert_eq!(sent["prompt"], json!("a red apple"));
    assert_eq!(sent["width"], json!(992));
    assert_eq!(sent["height"], json!(768));
    assert!(sent["seed"].is_i64(), "a seed is always sent: {sent}");
    // The deliberate omission (ADR 0011): pinning steps or CFG here would be
    // wrong for whichever model class the server did not load.
    assert!(sent.get("sample_params").is_none(), "sampling is the model's default to pick");
    assert!(sent.get("video_frames").is_none(), "an image job carries no frame count");

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
        json!(format!("{id}_output.png")),
        "the saved name is the job id and the backend's format"
    );
    assert!(POLLS.load(Ordering::Relaxed) >= 2, "the `generating` poll must have been tolerated");

    // -- the bytes came back through the file route unchanged ---------------
    let bytes = authed(http.get(format!("{origin}/api/v1/media/jobs/{id}/file")))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), PNG, "base64 in the poll body must decode to the original bytes");

    let _ = std::fs::remove_dir_all(&media_dir);
}
