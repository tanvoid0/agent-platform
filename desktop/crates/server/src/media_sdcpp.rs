//! The `stable-diffusion.cpp` backend for [`crate::media`] (ADR 0011).
//!
//! **What this talks to.** `sd-server`, the HTTP server that ships in
//! stable-diffusion.cpp's own release zips. One MIT-licensed native binary —
//! no Python, no torch, no interpreter — reached over loopback exactly like
//! ComfyUI is. It answers three API families; this module uses the native
//! `/sdcpp/v1/…` one, because that is the only family with **async jobs**, and
//! a job is the shape [`crate::media`] already stores.
//!
//! **One model per process.** `sd-server` binds its model at startup
//! (`--diffusion-model`, `--vae`, `--llm`); there is no load-a-different-model
//! route. `GET /sdcpp/v1/capabilities` reports which modes the loaded model
//! supports (`img_gen`, `vid_gen`), so asking a text-to-image model for a
//! video is a thing this module can answer *before* submitting rather than a
//! failure minutes later. That constraint is not a hardship on the hardware
//! this targets: 16 GB of VRAM cannot hold an image and a video model resident
//! at once anyway.
//!
//! **What we deliberately do not send.** Sampler, scheduler, step count and
//! CFG are all omitted, so `sd-server` applies the defaults for whatever model
//! it loaded. This is not laziness about quality — it is the opposite. A
//! distilled model (Z-Image-Turbo, Flux Klein) wants `txt_cfg` 1.0 and ~8
//! steps, a full one (Flux-dev) wants 3.5 and ~28, and a caller that pins
//! either number gets the other model badly wrong. We send only what the user
//! actually chose: prompt, size, seed, and the frame count.
//!
//! **Wire contract**, from `examples/server/api.md` at the pinned release:
//! submit answers `202` with `{id}`; `GET /sdcpp/v1/jobs/{id}` reports
//! `queued | generating | completed | failed | cancelled`; a finished image is
//! `result.images[].b64_json`, a finished video is a single encoded container
//! in `result.b64_json` with its `mime_type`. Bytes come back **inside the
//! JSON**, which is why there is no separate fetch step here the way ComfyUI's
//! `/view` needs one.

use std::time::Duration;

use axum::http::StatusCode;
use base64::Engine as _;
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::media::{JobSpec, Poll};
use crate::AppState;

/// Where `sd-server` listens with no `--listen-port`, per its own README.
pub const DEFAULT_BASE: &str = "http://127.0.0.1:1234";

/// What the loaded model is and what it can be asked for. `modes` holds
/// `sd-server`'s `supported_modes` — `img_gen`, `vid_gen`, or both.
pub struct Capabilities {
    pub model: Option<String>,
    pub modes: Vec<String>,
}

/// The probe behind `GET /api/v1/media/status`. One short GET, no retries:
/// `sd-server` not running is the *expected* state before the lifecycle work
/// lands, and must come back fast rather than after a retry ladder.
pub(crate) async fn capabilities(state: &AppState, base: &str) -> Option<Capabilities> {
    let body: Value = state
        .http
        .get(format!("{base}/sdcpp/v1/capabilities"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    // `model.name` is the file as loaded; `model.stem` is the friendlier form
    // and what the deprecated top-level mirrors use. Prefer the stem, fall
    // back to the name, and treat neither being there as "no model", not as
    // an unreachable server — a reachable server always has one.
    let model = body
        .pointer("/model/stem")
        .or_else(|| body.pointer("/model/name"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let modes = body
        .get("supported_modes")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Some(Capabilities { model, modes })
}

/// `img_gen` for images, `vid_gen` for video — the two native modes, and the
/// strings `supported_modes` reports.
fn mode_for(kind: &str) -> &'static str {
    if kind == "video" {
        "vid_gen"
    } else {
        "img_gen"
    }
}

/// The container a finished video comes back in. `webm` over `avi`/`webp`
/// because it is what a stock Windows/macOS player opens, and the desktop
/// hands video to the default player rather than decoding it (ADR 0009).
const VIDEO_FORMAT: &str = "webm";

/// Frames per second for `vid_gen`. Pinned to the rate the video models this
/// targets are trained at, and the same rate `media.rs`'s frame-count default
/// is reckoned in — a mismatch here silently changes clip *duration*, not
/// quality, which is the kind of bug nobody reports.
const VIDEO_FPS: i64 = 24;

/// `POST /sdcpp/v1/{img_gen,vid_gen}` → the job id to poll.
///
/// Errors are named rather than paraphrased: an unreachable server and a
/// rejected request are different problems with different fixes, and
/// `sd-server`'s own rejection text says which field it disliked.
pub(crate) async fn submit(state: &AppState, base: &str, spec: &JobSpec) -> Result<String, ApiError> {
    let mode = mode_for(spec.kind);

    let mut payload = json!({
        "prompt": spec.prompt,
        "negative_prompt": spec.negative,
        "width": spec.width,
        "height": spec.height,
        "seed": spec.seed,
    });
    if spec.kind == "video" {
        let obj = payload.as_object_mut().expect("payload is a JSON object");
        obj.insert("video_frames".into(), json!(spec.length));
        obj.insert("fps".into(), json!(VIDEO_FPS));
        obj.insert("output_format".into(), json!(VIDEO_FORMAT));
    }

    let response = state
        .http
        .post(format!("{base}/sdcpp/v1/{mode}"))
        .json(&payload)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|_| {
            ApiError::coded(
                StatusCode::BAD_GATEWAY,
                "media_backend_unreachable",
                format!(
                    "No sd-server answering at {base}. Start it (or set MEDIA_API_BASE), \
                     then try again."
                ),
            )
        })?;

    let status = response.status();
    let body: Value = response.json().await.unwrap_or_else(|_| json!({}));
    if !status.is_success() {
        let detail = error_text(&body)
            .unwrap_or_else(|| format!("sd-server rejected the request with HTTP {status}."));
        return Err(ApiError::coded(StatusCode::BAD_GATEWAY, "media_workflow_rejected", detail));
    }

    body.get("id").and_then(Value::as_str).map(str::to_string).ok_or_else(|| {
        ApiError::coded(
            StatusCode::BAD_GATEWAY,
            "media_workflow_rejected",
            "sd-server accepted the request but returned no job id.",
        )
    })
}

/// `GET /sdcpp/v1/jobs/{id}`, mapped onto [`Poll`].
///
/// A transport failure is [`Poll::Pending`], not a failure: `sd-server`
/// restarting mid-job is survivable, and `media.rs`'s deadline is what ends a
/// job that never comes back. A `404`/`410`, though, is terminal — the server
/// has forgotten this job and no amount of further polling will change that.
pub(crate) async fn poll(state: &AppState, base: &str, job_id: &str) -> Poll {
    let Ok(response) = state
        .http
        .get(format!("{base}/sdcpp/v1/jobs/{job_id}"))
        .timeout(Duration::from_secs(10))
        .send()
        .await
    else {
        return Poll::Pending;
    };

    let status = response.status();
    if status == StatusCode::NOT_FOUND || status == StatusCode::GONE {
        return Poll::Failed(
            "sd-server no longer knows about this job — it was probably restarted.".to_string(),
        );
    }
    let Ok(body) = response.json::<Value>().await else { return Poll::Pending };

    match body.get("status").and_then(Value::as_str).unwrap_or_default() {
        "completed" => decode_result(&body),
        "failed" => Poll::Failed(
            error_text(&body).unwrap_or_else(|| "sd-server reported a failed job.".to_string()),
        ),
        "cancelled" => Poll::Failed("The job was cancelled.".to_string()),
        // queued | generating | anything a newer release adds.
        _ => Poll::Pending,
    }
}

/// The finished bytes. Images arrive as a list even when one was asked for;
/// video arrives as one already-encoded container. Both are base64 inside the
/// JSON body, so there is no second request to make.
fn decode_result(body: &Value) -> Poll {
    let Some(result) = body.get("result") else {
        return Poll::Failed("sd-server reported the job complete but returned no result.".into());
    };

    // Video: one container, its type named by `mime_type`.
    if let Some(b64) = result.get("b64_json").and_then(Value::as_str) {
        let ext = result
            .get("mime_type")
            .and_then(Value::as_str)
            .and_then(|m| m.rsplit('/').next())
            .filter(|e| !e.is_empty())
            .unwrap_or(VIDEO_FORMAT);
        return decode_b64(b64, ext);
    }

    // Image: the first of `result.images`.
    let first = result.get("images").and_then(Value::as_array).and_then(|list| list.first());
    let Some(b64) = first.and_then(|i| i.get("b64_json")).and_then(Value::as_str) else {
        return Poll::Failed("sd-server finished the job but produced no image.".to_string());
    };
    let ext = result.get("output_format").and_then(Value::as_str).filter(|e| !e.is_empty());
    decode_b64(b64, ext.unwrap_or("png"))
}

fn decode_b64(b64: &str, ext: &str) -> Poll {
    match base64::engine::general_purpose::STANDARD.decode(b64) {
        Ok(bytes) if bytes.is_empty() => {
            Poll::Failed("sd-server returned an empty output.".to_string())
        }
        Ok(bytes) => Poll::Done { bytes, file_name: format!("output.{ext}") },
        Err(e) => Poll::Failed(format!("The output from sd-server was not valid base64: {e}")),
    }
}

/// `error` is an object of unspecified shape, and a rejected submission puts
/// its reason in whichever of these a given release chose. Checked in order
/// rather than assumed, because the alternative — showing `{"code":…}` to a
/// person — is what "surface the backend's own words" is supposed to prevent.
fn error_text(body: &Value) -> Option<String> {
    let error = body.get("error")?;
    if let Some(text) = error.as_str() {
        return (!text.is_empty()).then(|| text.to_string());
    }
    for key in ["message", "detail", "reason"] {
        if let Some(text) = error.get(key).and_then(Value::as_str) {
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    Some(error.to_string()).filter(|s| s != "null" && s != "{}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_finished_image_decodes_to_its_bytes() {
        let body = json!({
            "status": "completed",
            "result": { "output_format": "png", "images": [{ "index": 0, "b64_json": "aGk=" }] }
        });
        match decode_result(&body) {
            Poll::Done { bytes, file_name } => {
                assert_eq!(bytes, b"hi");
                assert_eq!(file_name, "output.png");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn a_finished_video_takes_its_extension_from_the_mime_type() {
        let body = json!({
            "status": "completed",
            "result": { "b64_json": "aGk=", "mime_type": "video/webm", "fps": 24 }
        });
        match decode_result(&body) {
            Poll::Done { file_name, .. } => assert_eq!(file_name, "output.webm"),
            other => panic!("expected Done, got {other:?}"),
        }
    }

    /// The regression that matters most: a completed job with nothing in it
    /// must settle as failed, not sit in `Pending` until the hour deadline.
    #[test]
    fn a_completed_job_with_no_output_fails_rather_than_hanging() {
        let body = json!({ "status": "completed", "result": { "images": [] } });
        assert!(matches!(decode_result(&body), Poll::Failed(_)));
        let empty = json!({ "status": "completed" });
        assert!(matches!(decode_result(&empty), Poll::Failed(_)));
    }

    #[test]
    fn bad_base64_is_reported_rather_than_saved() {
        let body = json!({
            "status": "completed",
            "result": { "images": [{ "b64_json": "not base64!!" }] }
        });
        assert!(matches!(decode_result(&body), Poll::Failed(_)));
    }

    #[test]
    fn the_backends_own_error_text_survives() {
        let nested = json!({ "error": { "message": "width must be a multiple of 8" } });
        assert_eq!(error_text(&nested).unwrap(), "width must be a multiple of 8");
        let flat = json!({ "error": "out of memory" });
        assert_eq!(error_text(&flat).unwrap(), "out of memory");
        assert_eq!(error_text(&json!({ "error": null })), None);
    }

    #[test]
    fn video_is_the_only_kind_that_carries_a_frame_count() {
        assert_eq!(mode_for("video"), "vid_gen");
        assert_eq!(mode_for("image"), "img_gen");
    }
}
