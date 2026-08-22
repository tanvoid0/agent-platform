//! `/api/v1/media/*` — local image and video generation (ADR 0009, ADR 0011).
//!
//! **Two backends, one domain.** `MEDIA_BACKEND` selects between them and
//! `MEDIA_API_BASE` says where it listens:
//!
//! - `comfy` (default) — ComfyUI's node-graph API, the original backend
//!   (ADR 0009). The server owns two checked-in workflow templates
//!   (`media_templates/`), fills in the prompt, size and seed, submits the
//!   graph to `POST /prompt`, and polls `GET /history/{id}`.
//! - `sdcpp` — stable-diffusion.cpp's `sd-server` (ADR 0011), in
//!   [`crate::media_sdcpp`]. One MIT-licensed native binary, no Python; flat
//!   parameters instead of a graph, so there is no template to break when a
//!   node is renamed upstream.
//!
//! Either way it is **an external local process reached over loopback**, the
//! way Ollama is for chat. The seam is deliberately thin: an adapter submits
//! and answers [`Poll`], and everything else here — the row, the waiter, the
//! deadline, prompt enhancement, the file route — is shared. The finished file
//! is copied into the server's own `media/` data dir, because a backend's
//! output directory is its own business and may be cleaned behind our back.
//!
//! **A generation is a job, not a request.** Diffusion runs seconds-to-minutes
//! (video: minutes), so `POST /generate` answers immediately with a
//! `media_jobs` row and a background task does the waiting. The desktop polls
//! `GET /jobs` — same shape as model-ops build jobs, without the subprocess.
//!
//! **Unconfigured is a first-class state** (the ADR 0008 lesson):
//! `GET /status` reports `reachable: false` with the base URL it tried, and
//! the Studio screen renders an install pointer rather than an error. Routes
//! that need ComfyUI answer a named 502 only when actually asked to generate.
//!
//! **Templates are data, and user-overridable.** A file at
//! `<media dir>/templates/text_to_image.json` or `text_to_video.json` (a
//! ComfyUI "Export (API)" graph with `__AGP_*__` placeholders) replaces the
//! built-in — the escape hatch for a ComfyUI or model-family update that
//! renames a node, and the way a user runs a different model without waiting
//! for this crate. The video template targets ComfyUI's stock Wan 2.2 5B
//! text-to-video workflow and hard-codes that family's file names; the image
//! template resolves `__AGP_MODEL__` against whatever checkpoints ComfyUI
//! reports installed, preferring the known text-to-image families.
//!
//! **Master-key only** (ADR 0009 "Tenancy"): media jobs are the desktop's own
//! surface, like `.env` admin. A workspace token gets a 403, not a filtered
//! view. E.V. needs no new tool for any of this — `GET`s come free through
//! `assistant_tools::api_get`, and `POST /generate` is a write that parks
//! behind the existing `api_write` confirm card.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::FromRow;

use crate::auth::Principal;
use crate::db;
use crate::error::{ApiError, PathId};
use crate::wire::{sql_now, sql_time};
use crate::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/media/status", get(status))
        .route("/api/v1/media/generate", post(generate))
        .route("/api/v1/media/suggest", get(suggest))
        .route("/api/v1/media/requirements", get(requirements))
        .route("/api/v1/media/models", get(models))
        .route("/api/v1/media/models/{model_id}/install", post(install_model))
        .route("/api/v1/media/jobs", get(list_jobs))
        .route("/api/v1/media/jobs/{job_id}", get(get_job))
        .route("/api/v1/media/jobs/{job_id}/file", get(job_file))
}

const NOT_MASTER: &str = "Media generation is managed with the master key.";

/// Where finished images and videos are kept, and where a user's own workflow
/// templates go (`templates/` inside it). `MEDIA_DATA_DIR`, else
/// `CONFIG_DIR/media` — the same shape as [`crate::model_ops`]'s `data_dir`,
/// so the desktop points both at its own data folder with one env var each.
pub fn media_dir() -> PathBuf {
    crate::env_opt("MEDIA_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::llm_config::config_dir().join("media"))
}

/// Which local generator `MEDIA_API_BASE` points at (ADR 0011).
///
/// Two adapters, one domain: everything below the wire — the job row, the
/// waiter, the file route, prompt enhancement — is shared, and only submit,
/// poll and probe differ. Keeping ComfyUI is the point rather than an
/// oversight: it carries an ecosystem sd.cpp does not, and a user who already
/// has it running should not have to give it up for this change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaBackend {
    /// ComfyUI's node-graph API (ADR 0009). Workflow templates apply.
    Comfy,
    /// stable-diffusion.cpp's `sd-server` (ADR 0011). No templates, no Python.
    Sdcpp,
}

impl MediaBackend {
    /// The name this backend answers to in `MEDIA_BACKEND`, and what
    /// `GET /status` reports so the desktop can label what it is talking to.
    pub fn as_str(self) -> &'static str {
        match self {
            MediaBackend::Comfy => "comfy",
            MediaBackend::Sdcpp => "sdcpp",
        }
    }
}

/// `MEDIA_BACKEND` = `comfy` (default) | `sdcpp`.
///
/// **ComfyUI stays the default** until sd.cpp's video output has been compared
/// against it on real hardware — the switch is a decision to make on rendered
/// frames, not on a spec sheet. An unrecognised value falls back rather than
/// refusing to start: a typo here should cost a working generator, not the
/// whole server.
pub fn media_backend() -> MediaBackend {
    match crate::llm_config::from_env_or_dotenv("MEDIA_BACKEND").trim().to_ascii_lowercase().as_str()
    {
        "sdcpp" | "sd-cpp" | "sd_cpp" | "stable-diffusion.cpp" => MediaBackend::Sdcpp,
        "" | "comfy" | "comfyui" => MediaBackend::Comfy,
        other => {
            logd!("[media] unknown MEDIA_BACKEND {other:?}; using comfy");
            MediaBackend::Comfy
        }
    }
}

/// The backend base URL. Always present — the default is where the selected
/// backend listens out of the box — so "configured" is never the question
/// here; "reachable" is, and `GET /status` answers it.
pub fn media_api_base() -> String {
    let base = crate::llm_config::from_env_or_dotenv("MEDIA_API_BASE");
    if !base.is_empty() {
        return base.trim_end_matches('/').to_string();
    }
    match media_backend() {
        MediaBackend::Comfy => "http://127.0.0.1:8188".to_string(),
        MediaBackend::Sdcpp => crate::media_sdcpp::DEFAULT_BASE.to_string(),
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/media/status — is ComfyUI there, and what can it draw with
// ---------------------------------------------------------------------------

/// A probe, not a health check: one short-timeout GET, no retries. ComfyUI
/// not running is the *expected* state on most installs and must come back
/// fast and calm, not after a retry ladder.
async fn status(
    State(state): State<Arc<AppState>>,
    principal: Principal,
) -> Result<Response, ApiError> {
    principal.require_master_key(NOT_MASTER)?;
    let base = media_api_base();
    let backend = media_backend();

    // `modes` is what the backend can actually be asked for right now. ComfyUI
    // loads models per graph, so it can always attempt both; sd-server binds
    // one model at startup and says which modes that model supports — the
    // difference the Studio screen needs in order to grey out the right toggle
    // instead of failing a video job three minutes in.
    let (reachable, checkpoints, modes) = match backend {
        MediaBackend::Comfy => {
            let reachable = state
                .http
                .get(format!("{base}/system_stats"))
                .timeout(Duration::from_secs(2))
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);
            let checkpoints =
                if reachable { installed_checkpoints(&state, &base).await } else { Vec::new() };
            let modes = if reachable {
                vec!["img_gen".to_string(), "vid_gen".to_string()]
            } else {
                Vec::new()
            };
            (reachable, checkpoints, modes)
        }
        MediaBackend::Sdcpp => match crate::media_sdcpp::capabilities(&state, &base).await {
            Some(caps) => (true, caps.model.into_iter().collect::<Vec<_>>(), caps.modes),
            None => (false, Vec::new(), Vec::new()),
        },
    };

    // What the *managed* sd-server is doing, which is a different question from
    // whether one is answering: "downloading" and "not_installed" are both
    // `reachable: false`, and the screen should say which.
    let (stage, stage_detail) = match backend {
        MediaBackend::Sdcpp => {
            let (stage, detail) = crate::media_sdcpp_process::stage_report();
            (Some(stage), detail)
        }
        MediaBackend::Comfy => (None, None),
    };

    Ok(Json(json!({
        "reachable": reachable,
        "base": base,
        "backend": backend.as_str(),
        "backend_stage": stage,
        "backend_detail": stage_detail,
        "checkpoints": checkpoints,
        "modes": modes,
        "image_model": match backend {
            // sd-server's one loaded model *is* the image model; there is no
            // choice to make and no family to prefer.
            MediaBackend::Sdcpp => checkpoints.first().cloned(),
            MediaBackend::Comfy => choose_checkpoint(&checkpoints),
        },
    }))
    .into_response())
}

/// What `CheckpointLoaderSimple` can load — ComfyUI's `/object_info` reports
/// each node's input options, and the checkpoint list is the first element of
/// `ckpt_name`'s spec. Empty on any failure: a probe helper, not a route.
async fn installed_checkpoints(state: &AppState, base: &str) -> Vec<String> {
    let Ok(response) = state
        .http
        .get(format!("{base}/object_info/CheckpointLoaderSimple"))
        .timeout(Duration::from_secs(5))
        .send()
        .await
    else {
        return Vec::new();
    };
    let Ok(body) = response.json::<Value>().await else { return Vec::new() };
    body.pointer("/CheckpointLoaderSimple/input/required/ckpt_name/0")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

/// The checkpoint the image template gets. Preference order is the known
/// text-to-image families, newest first; a lone unrecognised checkpoint still
/// wins over refusing, because the user installed it on purpose.
fn choose_checkpoint(installed: &[String]) -> Option<String> {
    const PREFER: &[&str] = &["flux", "z-image", "zimage", "sdxl", "sd_xl", "sd3"];
    for family in PREFER {
        if let Some(name) = installed.iter().find(|n| n.to_lowercase().contains(family)) {
            return Some(name.clone());
        }
    }
    installed.first().cloned()
}

// ---------------------------------------------------------------------------
// POST /api/v1/media/generate
// ---------------------------------------------------------------------------

/// Checked against `openapi.json`'s `MediaGenerateRequest` by
/// `scripts/check_openapi_request_drift.py` — a new field lands in both.
#[derive(Debug, Deserialize)]
struct GenerateRequest {
    kind: Option<String>,
    prompt: Option<String>,
    negative: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    /// Video only: frame count at 24 fps. Ignored for images.
    length: Option<i64>,
    /// Expand the prompt through the LLM proxy before generating. Any model
    /// failure degrades to the prompt as typed — the model is an upgrade,
    /// never a dependency (ADR 0009).
    enhance: Option<bool>,
}

/// What a backend is asked to make. Deliberately backend-agnostic: it holds
/// what the *user* chose and nothing about how it will be rendered, which is
/// what lets one row and one waiter serve both adapters.
pub(crate) struct JobSpec {
    pub(crate) kind: &'static str,
    pub(crate) prompt: String,
    pub(crate) negative: String,
    pub(crate) width: i64,
    pub(crate) height: i64,
    pub(crate) length: i64,
    pub(crate) seed: i64,
}

/// One backend poll, in the only three shapes [`watch_job`] cares about.
///
/// `Done` carries the **bytes**, not a URL, because the two backends differ
/// exactly there: ComfyUI reports a filename to fetch from `/view`, sd-server
/// returns base64 in the poll body. Making the adapter hand back bytes keeps
/// that difference inside the adapter, and leaves one place — [`save_output`]
/// — that writes a file.
#[derive(Debug)]
pub(crate) enum Poll {
    Pending,
    Done {
        bytes: Vec<u8>,
        /// The backend's suggested name; the saved name is this prefixed with
        /// the job id.
        file_name: String,
    },
    Failed(String),
}

/// Defaults per kind: images at 1024², video at Wan 2.2 5B's 832×480 and ~2s.
/// Dimensions are clamped, not rejected — a wild value from a model-written
/// call becomes a sane one rather than a 400 the user has to relay.
fn job_spec(req: &GenerateRequest) -> Result<JobSpec, ApiError> {
    let kind = match req.kind.as_deref().map(str::trim).unwrap_or("image") {
        "image" => "image",
        "video" => "video",
        other => {
            return Err(ApiError::bad_request(format!(
                "`kind` must be \"image\" or \"video\", not {other:?}."
            )))
        }
    };
    let prompt = req
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("`prompt` is required."))?
        .to_string();

    let (dw, dh) = if kind == "video" { (832, 480) } else { (1024, 1024) };
    // Multiples of 16: both latent spaces require it, and rounding here beats
    // ComfyUI's error naming a tensor shape.
    let snap = |v: i64, d: i64| (v.clamp(256, 2048) / 16 * 16).max(256).min(d * 2);
    Ok(JobSpec {
        kind,
        prompt,
        negative: req.negative.as_deref().unwrap_or("").trim().to_string(),
        width: snap(req.width.unwrap_or(dw), dw),
        height: snap(req.height.unwrap_or(dh), dh),
        length: if kind == "video" { req.length.unwrap_or(49).clamp(9, 241) } else { 0 },
        seed: random_seed(),
    })
}

/// Non-cryptographic and non-repeating is all a diffusion seed needs.
fn random_seed() -> i64 {
    i64::from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0),
    )
}

async fn generate(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    principal.require_master_key(NOT_MASTER)?;
    let req: GenerateRequest = crate::wire::parse_body_typed(&body)?;
    let mut spec = job_spec(&req)?;

    // The optional model pass, before anything is stored — what it produces
    // is part of the job's record, so the gallery can show both what was
    // asked and what was drawn from.
    let enhanced = if req.enhance.unwrap_or(false) {
        enhance_prompt(&state, spec.kind, &spec.prompt).await
    } else {
        None
    };
    if let Some(better) = &enhanced {
        spec.prompt = better.clone();
    }

    let base = media_api_base();
    let backend = media_backend();
    // The one place the two backends diverge on the way in. ComfyUI is handed
    // a whole graph; sd-server is handed the spec's fields. Both answer with
    // an id to poll, which is all the row stores.
    let backend_job_id = match backend {
        MediaBackend::Comfy => {
            let graph = build_workflow(&state, &base, &spec).await?;
            submit_workflow(&state, &base, &graph).await?
        }
        MediaBackend::Sdcpp => {
            // The one place a generation may block on a download and a model
            // load. Deliberately here and not in the status probe: a gallery
            // refresh must never start a multi-gigabyte load (ADR 0011).
            crate::media_sdcpp_process::ensure_running(&state, &base, spec.kind).await?;
            crate::media_sdcpp::submit(&state, &base, &spec).await?
        }
    };

    let now = sql_now();
    let original_prompt = req.prompt.as_deref().unwrap_or("").trim();
    let id: i64 = sqlx::query_scalar(&db::sql(
        "INSERT INTO media_jobs (kind, prompt, enhanced_prompt, status, width, height, length, \
         seed, comfy_prompt_id, created_at, updated_at) \
         VALUES (?, ?, ?, 'running', ?, ?, ?, ?, ?, ?, ?) RETURNING CAST(id AS BIGINT)",
        state.backend,
    ))
    .bind(spec.kind)
    .bind(original_prompt)
    .bind(&enhanced)
    .bind(spec.width)
    .bind(spec.height)
    .bind(spec.length)
    .bind(spec.seed)
    // `comfy_prompt_id` predates the second backend and now holds whichever
    // backend's job id — renaming a column costs a migration for a name only
    // this file reads.
    .bind(&backend_job_id)
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.any)
    .await?;

    // The waiter outlives this request on purpose; the row is how anyone —
    // including a restarted app — finds out how it went.
    tokio::spawn(watch_job(state.clone(), id, backend, base, backend_job_id));

    Ok((StatusCode::CREATED, Json(load_job(&state, id).await?)).into_response())
}

// ---------------------------------------------------------------------------
// GET /api/v1/media/requirements — what the video template needs, and where
// ---------------------------------------------------------------------------
//
// The image template picks whatever checkpoint is installed, so it never has
// a shopping list. Video cannot: `text_to_video.json` names three exact files
// by their ComfyUI folder, and a missing one is a 502 out of `/prompt` that
// the user can do nothing about from inside this app — which is what this
// route is for. It answers what is required, what is already there, and the
// directory the desktop may write the rest into.
//
// The files are the Wan 2.2 TI2V 5B set, pinned to the same repackaged repo
// ComfyUI's own template links. They are named here rather than scraped
// because a download button that guesses a URL is a download button that one
// day fetches the wrong ten gigabytes.

/// `(ComfyUI models folder, filename, URL, exact size in bytes)`. The sizes
/// are the real `content-length` of each file, so the confirm step can say
/// what it is about to spend before anything is fetched.
const VIDEO_REQUIREMENTS: [(&str, &str, &str, i64); 3] = [
    (
        "diffusion_models",
        "wan2.2_ti2v_5B_fp16.safetensors",
        "https://huggingface.co/Comfy-Org/Wan_2.2_ComfyUI_Repackaged/resolve/main/split_files/diffusion_models/wan2.2_ti2v_5B_fp16.safetensors",
        9_999_658_848,
    ),
    (
        "vae",
        "wan2.2_vae.safetensors",
        "https://huggingface.co/Comfy-Org/Wan_2.2_ComfyUI_Repackaged/resolve/main/split_files/vae/wan2.2_vae.safetensors",
        1_409_400_960,
    ),
    (
        "text_encoders",
        "umt5_xxl_fp8_e4m3fn_scaled.safetensors",
        "https://huggingface.co/Comfy-Org/Wan_2.2_ComfyUI_Repackaged/resolve/main/split_files/text_encoders/umt5_xxl_fp8_e4m3fn_scaled.safetensors",
        6_735_906_897,
    ),
];

#[derive(Serialize)]
struct Requirement {
    folder: &'static str,
    file_name: &'static str,
    url: &'static str,
    size_bytes: i64,
    installed: bool,
}

#[derive(Serialize)]
struct RequirementsResponse {
    /// ComfyUI's `models/` directory, when it could be established — the
    /// desktop needs somewhere to put a download, and `None` means it must
    /// ask rather than guess.
    models_root: Option<String>,
    items: Vec<Requirement>,
}

async fn requirements(
    State(state): State<Arc<AppState>>,
    principal: Principal,
) -> Result<Response, ApiError> {
    principal.require_master_key(NOT_MASTER)?;
    let base = media_api_base();

    // sd-server has no shopping list, and that is a fact rather than a gap:
    // its model is bound by the flags it was started with, so a *reachable*
    // sd-server is by definition one whose model is already on disk. What it
    // cannot do is loaded — `GET /status`'s `modes` answers that — and what it
    // has not got is unreachability, which `reachable: false` answers.
    if media_backend() == MediaBackend::Sdcpp {
        return Ok(Json(RequirementsResponse { models_root: None, items: Vec::new() })
            .into_response());
    }

    let mut items = Vec::with_capacity(VIDEO_REQUIREMENTS.len());
    for (folder, file_name, url, size_bytes) in VIDEO_REQUIREMENTS {
        let installed = installed_models(&state, &base, folder)
            .await
            .is_some_and(|files| files.iter().any(|f| f == file_name));
        items.push(Requirement { folder, file_name, url, size_bytes, installed });
    }

    Ok(Json(RequirementsResponse { models_root: comfy_models_root(&state, &base).await, items })
        .into_response())
}

// ---------------------------------------------------------------------------
// GET /api/v1/media/models, POST /api/v1/media/models/{id}/install
// ---------------------------------------------------------------------------
//
// The sd.cpp answer to what `/requirements` does for ComfyUI: what is worth
// having, what it costs, and what is already on disk. The difference is that
// ComfyUI owns its models folder and we can only point at it, whereas these
// live in ours — so this pair can actually fetch them, and an installed model
// launches with no further configuration.

async fn models(principal: Principal) -> Result<Response, ApiError> {
    principal.require_master_key(NOT_MASTER)?;
    Ok(Json(crate::media_sdcpp_process::catalogue_report()).into_response())
}

/// Fetches every missing file for one catalogue model. Gigabytes, so it holds
/// the response open the way a job would not — deliberately: the desktop's
/// confirm card already quotes the total from `GET /models`, and a second job
/// table for a download that resumes at file granularity is machinery this does
/// not need yet.
async fn install_model(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(model_id): PathId<String>,
) -> Result<Response, ApiError> {
    principal.require_master_key(NOT_MASTER)?;
    Ok(Json(crate::media_sdcpp_process::install_model(&state, &model_id).await?).into_response())
}

/// `GET /models/{folder}` — ComfyUI's own listing for one model kind. `None`
/// when it cannot be read at all; an empty folder answers `Some([])`, and the
/// difference matters: unknown must not render as "missing" and offer to
/// re-download something already on disk.
async fn installed_models(state: &AppState, base: &str, folder: &str) -> Option<Vec<String>> {
    let response = state
        .http
        .get(format!("{base}/models/{folder}"))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let files: Vec<String> = response.json().await.ok()?;
    Some(files)
}

/// ComfyUI's `models/` directory, worked out from the running instance rather
/// than configured here.
///
/// ComfyUI has no endpoint that reports it, but `/system_stats` echoes the
/// `argv` it was started with, and `--output-directory` sits beside `models`
/// in every layout the desktop build ships. That makes this a guess — so it
/// is only returned once *verified*: the candidate must exist and must hold a
/// subfolder that ComfyUI's own `/models` listing agrees is there. A wrong
/// directory here would download ten gigabytes into a folder nothing reads.
async fn comfy_models_root(state: &AppState, base: &str) -> Option<String> {
    let response =
        state.http.get(format!("{base}/system_stats")).timeout(Duration::from_secs(10)).send().await.ok()?;
    let stats: Value = response.json().await.ok()?;
    let argv = stats.get("system")?.get("argv")?.as_array()?;

    let output_dir = argv
        .iter()
        .position(|a| a.as_str() == Some("--output-directory"))
        .and_then(|i| argv.get(i + 1))
        .and_then(Value::as_str)?;

    let candidate = PathBuf::from(output_dir).parent()?.join("models");
    // Verified, not assumed: a folder ComfyUI reports must also be on disk
    // where we think the root is, or this is the wrong tree.
    let known = VIDEO_REQUIREMENTS[0].0;
    (candidate.join(known).is_dir()).then(|| candidate.display().to_string())
}

// GET /api/v1/media/suggest?kind=image|video — a ready-to-run test prompt
//
// The same local model the `enhance` toggle uses, asked to invent the request
// instead of expanding one. It exists because staring at an empty prompt box
// is the slowest part of checking whether generation still works.
#[derive(Deserialize)]
struct SuggestQuery {
    kind: Option<String>,
}

#[derive(Serialize)]
struct SuggestResponse {
    kind: &'static str,
    prompt: String,
}

async fn suggest(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    axum::extract::Query(q): axum::extract::Query<SuggestQuery>,
) -> Result<Response, ApiError> {
    principal.require_master_key(NOT_MASTER)?;
    let kind = if q.kind.as_deref() == Some("video") { "video" } else { "image" };
    let prompt = suggest_prompt(&state, kind).await.ok_or_else(|| {
        ApiError::coded(
            StatusCode::BAD_GATEWAY,
            "media_suggest_unavailable",
            "No language model answered. Studio's suggestions need a working LLM provider — \
             the prompt box still takes your own words.",
        )
    })?;
    Ok(Json(SuggestResponse { kind, prompt }).into_response())
}

/// Ask the model for one usable prompt. The nonce matters: without something
/// varying per call a local model at this temperature returns the same "lone
/// lighthouse" every time, and a suggest button that repeats itself is a
/// button nobody presses twice.
async fn suggest_prompt(state: &AppState, kind: &str) -> Option<String> {
    state.master_key.as_ref()?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let subject = SUGGEST_SEEDS[nonce as usize % SUGGEST_SEEDS.len()];

    let mut payload = Map::new();
    payload.insert(
        "messages".into(),
        json!([
            { "role": "system", "content": SUGGEST_SYSTEM_PROMPT },
            { "role": "user", "content": format!(
                "Medium: {kind}. Build it around this and nothing else: {subject}." ) },
        ]),
    );
    payload.insert("max_tokens".into(), json!(200));
    payload.insert("temperature".into(), json!(1.0));

    let data = crate::llm::complete_internal(state, payload, crate::resources::Priority::Background).await.ok()?;
    let content =
        data.get("choices")?.as_array()?.first()?.get("message")?.get("content")?.as_str()?;
    let cleaned = content.trim().trim_matches('"').trim();
    (!cleaned.is_empty() && cleaned.len() < 2000).then(|| cleaned.to_string())
}

/// Rotated through so the model starts somewhere different each press. Kept
/// deliberately plain — they are a seed for the model, not the prompt itself.
const SUGGEST_SEEDS: [&str; 12] = [
    "a kitchen at dawn",
    "an abandoned funfair",
    "a creature nobody has named",
    "weather doing something it should not",
    "a machine left running too long",
    "a market street in the rain",
    "something very small, very close up",
    "a room the moment after someone left",
    "an animal wearing the wrong clothes",
    "a landscape on a planet with two suns",
    "a vehicle held together with tape",
    "a library that goes too far down",
];

const SUGGEST_SYSTEM_PROMPT: &str = "You invent one prompt for a local image or video diffusion \
    model, to test that it works. Be concrete and visual: subject, setting, lighting, style, \
    composition, as comma-separated phrases. For a video, include one simple camera or subject \
    motion. Keep it under 40 words. Answer with ONLY the prompt text — no quotes, no preamble, \
    no explanation, no title.";

/// One model round-trip turning a plain ask into a diffusion prompt. `None`
/// on any failure at all — no master key, no provider, bad output — and the
/// caller keeps the user's own words.
async fn enhance_prompt(state: &AppState, kind: &str, prompt: &str) -> Option<String> {
    state.master_key.as_ref()?;
    let mut payload = Map::new();
    payload.insert(
        "messages".into(),
        json!([
            { "role": "system", "content": ENHANCE_SYSTEM_PROMPT },
            { "role": "user", "content": format!("Medium: {kind}. Request: {prompt}") },
        ]),
    );
    payload.insert("max_tokens".into(), json!(200));

    let data = crate::llm::complete_internal(state, payload, crate::resources::Priority::Background).await.ok()?;
    let content =
        data.get("choices")?.as_array()?.first()?.get("message")?.get("content")?.as_str()?;
    let cleaned = content.trim().trim_matches('"').trim();
    (!cleaned.is_empty() && cleaned.len() < 2000).then(|| cleaned.to_string())
}

const ENHANCE_SYSTEM_PROMPT: &str = "You turn a short request into one detailed prompt for a \
    local image or video diffusion model. Describe subject, setting, lighting, style and \
    composition in comma-separated phrases. Answer with ONLY the prompt text — no quotes, no \
    preamble, no explanation.";

// ---------------------------------------------------------------------------
// The ComfyUI conversation: template → graph → /prompt → /history → /view
// ---------------------------------------------------------------------------

const IMAGE_TEMPLATE: &str = include_str!("media_templates/text_to_image.json");
const VIDEO_TEMPLATE: &str = include_str!("media_templates/text_to_video.json");

/// Where a user's own exported workflow overrides the built-in — see the
/// module doc. Beside the outputs rather than in the config dir, so one
/// folder holds everything this feature touches on disk.
fn template_override(kind: &str) -> Option<String> {
    let name = if kind == "video" { "text_to_video.json" } else { "text_to_image.json" };
    std::fs::read_to_string(media_dir().join("templates").join(name)).ok()
}

/// Fills a template's `__AGP_*__` placeholders and parses the result. The
/// substitution is textual and quote-aware — `"__AGP_WIDTH__"` becomes a bare
/// number — because ComfyUI wants real JSON numbers and a template that held
/// them as numbers would have nothing to substitute.
fn fill_template(template: &str, spec: &JobSpec, model: Option<&str>) -> Result<Value, ApiError> {
    let mut out = template
        .replace("\"__AGP_PROMPT__\"", &Value::String(spec.prompt.clone()).to_string())
        .replace("\"__AGP_NEGATIVE__\"", &Value::String(spec.negative.clone()).to_string())
        .replace("\"__AGP_SEED__\"", &spec.seed.to_string())
        .replace("\"__AGP_WIDTH__\"", &spec.width.to_string())
        .replace("\"__AGP_HEIGHT__\"", &spec.height.to_string())
        .replace("\"__AGP_LENGTH__\"", &spec.length.to_string());
    if let Some(model) = model {
        out = out.replace("\"__AGP_MODEL__\"", &Value::String(model.to_string()).to_string());
    }
    serde_json::from_str(&out).map_err(|e| {
        ApiError::coded(
            StatusCode::INTERNAL_SERVER_ERROR,
            "media_template_invalid",
            format!("The {} workflow template is not valid JSON after substitution: {e}", spec.kind),
        )
    })
}

async fn build_workflow(state: &AppState, base: &str, spec: &JobSpec) -> Result<Value, ApiError> {
    let overridden = template_override(spec.kind);
    let template = overridden.as_deref().unwrap_or(if spec.kind == "video" {
        VIDEO_TEMPLATE
    } else {
        IMAGE_TEMPLATE
    });

    // Only the image template carries `__AGP_MODEL__`; the video one names
    // its model family outright (module doc). Resolved per request, not
    // cached — the user installs checkpoints while both apps are running.
    let model = if template.contains("__AGP_MODEL__") {
        let installed = installed_checkpoints(state, base).await;
        Some(choose_checkpoint(&installed).ok_or_else(|| {
            ApiError::coded(
                StatusCode::BAD_GATEWAY,
                "media_no_models",
                "ComfyUI is running but has no checkpoints installed. Put a text-to-image \
                 model in ComfyUI/models/checkpoints and try again.",
            )
        })?)
    } else {
        None
    };

    fill_template(template, spec, model.as_deref())
}

/// ComfyUI puts the actionable half of a rejection in `node_errors`, keyed by
/// node id — `error` alone is the useless "Prompt outputs failed validation".
/// Flattened to `ClassType: details; ClassType: details`, which is what names
/// the missing checkpoint the user actually has to go install.
fn node_error_detail(body: &Value) -> Option<String> {
    let nodes = body.get("node_errors")?.as_object()?;
    let lines: Vec<String> = nodes
        .values()
        .flat_map(|node| {
            let class = node.get("class_type").and_then(Value::as_str).unwrap_or("node");
            node.get("errors")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .filter_map(move |e| {
                    let text = e
                        .get("details")
                        .and_then(Value::as_str)
                        .filter(|d| !d.is_empty())
                        .or_else(|| e.get("message").and_then(Value::as_str))?;
                    Some(format!("{class}: {text}"))
                })
                .collect::<Vec<_>>()
        })
        .collect();
    (!lines.is_empty()).then(|| lines.join("; "))
}

/// `POST /prompt`. ComfyUI validates the whole graph before queueing, and its
/// rejection names the node and input that failed — that text goes to the
/// caller verbatim, because "SaveVideo: required input is missing" is the
/// actionable message and anything we paraphrase it into is not.
async fn submit_workflow(state: &AppState, base: &str, graph: &Value) -> Result<String, ApiError> {
    let payload = json!({ "prompt": graph, "client_id": "agent-platformd" });
    let response = state
        .http
        .post(format!("{base}/prompt"))
        .json(&payload)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|_| {
            ApiError::coded(
                StatusCode::BAD_GATEWAY,
                "media_backend_unreachable",
                format!(
                    "No ComfyUI answering at {base}. Start it (or set MEDIA_API_BASE), \
                     then try again."
                ),
            )
        })?;

    let status = response.status();
    let body: Value = response.json().await.unwrap_or_else(|_| json!({}));
    if !status.is_success() {
        let detail = node_error_detail(&body)
            .or_else(|| body.get("error").map(|e| e.to_string()))
            .unwrap_or_else(|| format!("ComfyUI rejected the workflow with HTTP {status}."));
        return Err(ApiError::coded(StatusCode::BAD_GATEWAY, "media_workflow_rejected", detail));
    }
    body.get("prompt_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            ApiError::coded(
                StatusCode::BAD_GATEWAY,
                "media_workflow_rejected",
                "ComfyUI accepted the request but returned no prompt_id.",
            )
        })
}

/// How long a job may run before it is declared lost. Video on a mid GPU is
/// legitimately many minutes; an hour means something is wedged.
const JOB_DEADLINE: Duration = Duration::from_secs(60 * 60);
const POLL_EVERY: Duration = Duration::from_secs(2);

/// The background half of a job: poll the backend until it reports the job
/// done, save the output into `media_dir`, and write the row's terminal state.
/// Every exit path updates the row — a job that can end without a status
/// update is a spinner that never stops.
///
/// Backend-agnostic by construction: the adapter answers [`Poll`], and the
/// deadline, the sleep and the row update are the same either way.
async fn watch_job(
    state: Arc<AppState>,
    job_id: i64,
    backend: MediaBackend,
    base: String,
    backend_job_id: String,
) {
    let started = std::time::Instant::now();
    let outcome = loop {
        if started.elapsed() > JOB_DEADLINE {
            break Err(match backend {
                MediaBackend::Comfy => "Timed out after an hour. Check the ComfyUI window.",
                MediaBackend::Sdcpp => "Timed out after an hour. Check the sd-server log.",
            }
            .to_string());
        }
        tokio::time::sleep(POLL_EVERY).await;

        let poll = match backend {
            MediaBackend::Comfy => poll_comfy(&state, &base, &backend_job_id).await,
            MediaBackend::Sdcpp => {
                // A running job counts as use, or the idle watchdog would stop
                // the server out from under a five-minute video render.
                crate::media_sdcpp_process::note_used();
                crate::media_sdcpp::poll(&state, &base, &backend_job_id).await
            }
        };
        match poll {
            Poll::Pending => continue,
            Poll::Failed(error) => break Err(error),
            Poll::Done { bytes, file_name } => {
                break save_output(job_id, &file_name, bytes).await;
            }
        }
    };

    let now = sql_now();
    let result = match &outcome {
        Ok(file_name) => {
            sqlx::query(&db::sql(
                "UPDATE media_jobs SET status = 'completed', file_name = ?, updated_at = ? \
                 WHERE id = ?",
                state.backend,
            ))
            .bind(file_name)
            .bind(&now)
            .bind(job_id)
            .execute(&state.any)
            .await
        }
        Err(error) => {
            sqlx::query(&db::sql(
                "UPDATE media_jobs SET status = 'failed', error = ?, updated_at = ? WHERE id = ?",
                state.backend,
            ))
            .bind(error)
            .bind(&now)
            .bind(job_id)
            .execute(&state.any)
            .await
        }
    };
    if let Err(e) = result {
        logd!("[media] job {job_id} finished but its row could not be updated: {e}");
    }
}

/// ComfyUI's error report for a failed graph lives in `status.messages` as
/// `["execution_error", {node_type, exception_message, …}]` tuples. The
/// exception message is the part a person can act on.
fn history_error(status_obj: &Value) -> String {
    status_obj
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
        .filter(|m| m.first().and_then(Value::as_str) == Some("execution_error"))
        .filter_map(|m| m.get(1))
        .filter_map(|d| {
            let node = d.get("node_type").and_then(Value::as_str).unwrap_or("workflow");
            d.get("exception_message").and_then(Value::as_str).map(|msg| format!("{node}: {msg}"))
        })
        .next()
        .unwrap_or_else(|| "ComfyUI reported an execution error.".to_string())
}

#[derive(Debug, Deserialize)]
struct ComfyOutput {
    filename: String,
    #[serde(default)]
    subfolder: String,
    #[serde(rename = "type", default)]
    kind: String,
}

/// The first saved file in the history entry's outputs, whatever the save
/// node called its list — `images` for `SaveImage`, `videos`/`gifs` for the
/// video savers, and user overrides may use any of them.
fn first_output(entry: &Value) -> Option<ComfyOutput> {
    let outputs = entry.get("outputs")?.as_object()?;
    for node in outputs.values() {
        let node = node.as_object()?;
        for list in node.values() {
            let Some(items) = list.as_array() else { continue };
            for item in items {
                if let Ok(output) = serde_json::from_value::<ComfyOutput>(item.clone()) {
                    if !output.filename.is_empty() && output.kind != "temp" {
                        return Some(output);
                    }
                }
            }
        }
    }
    None
}

/// One `GET /history/{id}`, mapped onto [`Poll`]. A transport failure is
/// `Pending`, not a failure: ComfyUI restarting mid-job is survivable
/// (flapping happens; its queue does not survive, and the deadline is what
/// ends a job that never comes back).
///
/// Completion needs a second request, because ComfyUI reports a *filename*
/// and serves the bytes from `/view` — the asymmetry with sd-server that
/// [`Poll::Done`] carrying bytes exists to hide.
async fn poll_comfy(state: &AppState, base: &str, comfy_id: &str) -> Poll {
    let Ok(response) =
        state.http.get(format!("{base}/history/{comfy_id}")).timeout(Duration::from_secs(10)).send().await
    else {
        return Poll::Pending;
    };
    let Ok(body) = response.json::<Value>().await else { return Poll::Pending };
    let Some(entry) = body.get(comfy_id) else { return Poll::Pending };

    let status_obj = entry.get("status").cloned().unwrap_or_default();
    if status_obj.get("status_str").and_then(Value::as_str) == Some("error") {
        return Poll::Failed(history_error(&status_obj));
    }
    if !status_obj.get("completed").and_then(Value::as_bool).unwrap_or(false) {
        return Poll::Pending;
    }
    let Some(output) = first_output(entry) else {
        return Poll::Failed("The workflow finished but produced no output file.".to_string());
    };
    match fetch_output(state, base, &output).await {
        Ok(bytes) => Poll::Done { bytes, file_name: output.filename },
        Err(error) => Poll::Failed(error),
    }
}

/// `GET /view` → the raw bytes. Fetched rather than referenced because
/// ComfyUI's output folder is its own, cleaned on its own schedule; the copy
/// under `media_dir` is the one the file route serves forever.
async fn fetch_output(
    state: &AppState,
    base: &str,
    output: &ComfyOutput,
) -> Result<Vec<u8>, String> {
    let url = format!(
        "{base}/view?filename={}&subfolder={}&type={}",
        urlencode(&output.filename),
        urlencode(&output.subfolder),
        urlencode(if output.kind.is_empty() { "output" } else { &output.kind }),
    );
    let bytes = state
        .http
        .get(&url)
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| format!("The output could not be fetched from ComfyUI: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("The output could not be read from ComfyUI: {e}"))?;
    Ok(bytes.to_vec())
}

/// The one writer of a finished job's file, whichever backend produced the
/// bytes. Returns the saved name, which is what the row stores and what
/// `GET /jobs/{id}/file` looks up.
async fn save_output(job_id: i64, suggested: &str, bytes: Vec<u8>) -> Result<String, String> {
    let file_name = format!("{job_id}_{}", safe_file_name(suggested));
    let dir = media_dir();
    let path = dir.join(&file_name);
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, &bytes)
    })
    .await
    .map_err(|e| format!("The save task failed: {e}"))?
    .map_err(|e| format!("The output could not be saved: {e}"))?;
    Ok(file_name)
}

/// The filename came over HTTP from another process — keep the extension,
/// drop anything path-like. Not a security boundary against ComfyUI (it runs
/// as the same user), just hygiene for a name we then serve back.
fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect();
    let trimmed = cleaned.trim_matches('.');
    if trimmed.is_empty() { "output".to_string() } else { trimmed.to_string() }
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

// ---------------------------------------------------------------------------
// The job rows: list, get, file
// ---------------------------------------------------------------------------

#[derive(Debug, FromRow, Serialize)]
struct MediaJobOut {
    id: i64,
    kind: String,
    prompt: String,
    enhanced_prompt: Option<String>,
    status: String,
    error: Option<String>,
    width: i64,
    height: i64,
    length: i64,
    seed: i64,
    file_name: Option<String>,
    #[serde(serialize_with = "sql_time")]
    created_at: String,
    #[serde(serialize_with = "sql_time")]
    updated_at: String,
}

const JOB_COLUMNS: &str = "CAST(id AS BIGINT) AS id, kind, prompt, enhanced_prompt, status, \
     error, CAST(width AS BIGINT) AS width, CAST(height AS BIGINT) AS height, \
     CAST(length AS BIGINT) AS length, CAST(seed AS BIGINT) AS seed, file_name, \
     CAST(created_at AS TEXT) AS created_at, CAST(updated_at AS TEXT) AS updated_at";

async fn load_job(state: &AppState, id: i64) -> Result<MediaJobOut, ApiError> {
    sqlx::query_as(&db::sql(
        &format!("SELECT {JOB_COLUMNS} FROM media_jobs WHERE id = ?"),
        state.backend,
    ))
    .bind(id)
    .fetch_optional(&state.any)
    .await?
    .ok_or_else(|| ApiError::not_found("Media job not found"))
}

const JOBS_LIMIT: i64 = 100;

async fn list_jobs(
    State(state): State<Arc<AppState>>,
    principal: Principal,
) -> Result<Response, ApiError> {
    principal.require_master_key(NOT_MASTER)?;
    let sql = db::sql(
        &format!("SELECT {JOB_COLUMNS} FROM media_jobs ORDER BY id DESC LIMIT ?"),
        state.backend,
    )
    .into_owned();
    let rows: Vec<MediaJobOut> =
        sqlx::query_as(&sql).bind(JOBS_LIMIT).fetch_all(&state.any).await?;
    Ok(Json(json!({ "jobs": rows })).into_response())
}

async fn get_job(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(job_id): PathId<i64>,
) -> Result<Response, ApiError> {
    principal.require_master_key(NOT_MASTER)?;
    Ok(Json(load_job(&state, job_id).await?).into_response())
}

/// The finished bytes — the server's first raw-binary route (ADR 0009), which
/// is what lets the desktop render a result without knowing where the data
/// dir lives.
async fn job_file(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(job_id): PathId<i64>,
) -> Result<Response, ApiError> {
    principal.require_master_key(NOT_MASTER)?;
    let job = load_job(&state, job_id).await?;
    let Some(file_name) = job.file_name.filter(|_| job.status == "completed") else {
        return Err(ApiError::not_found("This job has no finished file."));
    };
    let path = media_dir().join(&file_name);
    let bytes = tokio::task::spawn_blocking(move || std::fs::read(&path))
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, format!("Read task failed: {e}")))?
        .map_err(|_| ApiError::not_found("The finished file is missing from the media folder."))?;
    Ok(([(axum::http::header::CONTENT_TYPE, content_type(&file_name))], bytes).into_response())
}

fn content_type(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

// ---------------------------------------------------------------------------
// Startup recovery
// ---------------------------------------------------------------------------

/// A queued/running row with no process watching it is a spinner that never
/// stops: the watcher is a `tokio::spawn` in this process, so a restart
/// orphans every open job. Same idea as `executor::spawn_startup_recovery`,
/// collapsed to one statement because a media job has no steps to replay.
pub fn spawn_startup_recovery(state: Arc<AppState>) {
    tokio::spawn(async move {
        let result = sqlx::query(&db::sql(
            "UPDATE media_jobs SET status = 'failed', \
             error = 'The server restarted while this job was running.', updated_at = ? \
             WHERE status IN ('queued', 'running')",
            state.backend,
        ))
        .bind(sql_now())
        .execute(&state.any)
        .await;
        match result {
            Ok(done) if done.rows_affected() > 0 => {
                logd!("[media] marked {} orphaned job(s) failed after restart", done.rows_affected());
            }
            Ok(_) => {}
            Err(e) => logd!("[media] startup recovery failed: {e}"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: &'static str) -> JobSpec {
        JobSpec {
            kind,
            prompt: "a red apple on a wooden table".to_string(),
            negative: "blurry".to_string(),
            width: 768,
            height: 512,
            length: 49,
            seed: 42,
        }
    }

    /// The built-in templates must survive their own substitution — a
    /// sentinel typo here would otherwise surface as a runtime 500 on the
    /// first generate of that kind.
    #[test]
    fn built_in_templates_fill_and_parse() {
        let image = fill_template(IMAGE_TEMPLATE, &spec("image"), Some("sd_xl_base_1.0.safetensors"))
            .expect("image template fills");
        assert_eq!(image["6"]["inputs"]["text"], json!("a red apple on a wooden table"));
        assert_eq!(image["7"]["inputs"]["text"], json!("blurry"));
        assert_eq!(image["5"]["inputs"]["width"], json!(768), "width must be a JSON number");
        assert_eq!(image["3"]["inputs"]["seed"], json!(42));
        assert_eq!(image["4"]["inputs"]["ckpt_name"], json!("sd_xl_base_1.0.safetensors"));

        let video = fill_template(VIDEO_TEMPLATE, &spec("video"), None).expect("video template fills");
        assert_eq!(video["55"]["inputs"]["length"], json!(49));
        assert_eq!(video["55"]["inputs"]["width"], json!(768));
        assert_eq!(video["6"]["inputs"]["text"], json!("a red apple on a wooden table"));
    }

    /// The whole point of the rejection path: a missing model must reach the
    /// user by name. The body is a real ComfyUI 400 with the ids trimmed.
    #[test]
    fn node_errors_name_the_missing_model() {
        let body = json!({
            "error": { "type": "prompt_outputs_failed_validation", "details": "" },
            "node_errors": {
                "39": {
                    "class_type": "VAELoader",
                    "errors": [{
                        "type": "value_not_in_list",
                        "message": "Value not in list",
                        "details": "vae_name: 'wan2.2_vae.safetensors' not in ['pixel_space']"
                    }]
                }
            }
        });
        let detail = node_error_detail(&body).expect("node_errors carry a detail");
        assert!(detail.contains("VAELoader"), "{detail}");
        assert!(detail.contains("wan2.2_vae.safetensors"), "{detail}");

        // An empty `details` must fall back to the message, not to "".
        let terse = json!({
            "node_errors": { "1": { "class_type": "KSampler",
                "errors": [{ "message": "Value not in list", "details": "" }] } }
        });
        assert_eq!(node_error_detail(&terse).as_deref(), Some("KSampler: Value not in list"));

        // No node_errors at all → caller falls back to `error`.
        assert_eq!(node_error_detail(&json!({ "error": "boom" })), None);
    }

    /// Every requirement must name a folder ComfyUI actually has and carry a
    /// real size — a confirm step that says "0 bytes" teaches nothing, and a
    /// folder typo downloads into a directory ComfyUI never reads.
    #[test]
    fn video_requirements_are_plausible() {
        // The folder names ComfyUI's own `/models` listing uses.
        const KNOWN: [&str; 3] = ["diffusion_models", "vae", "text_encoders"];
        for (folder, file, url, size) in VIDEO_REQUIREMENTS {
            assert!(KNOWN.contains(&folder), "{folder} is not a ComfyUI model folder");
            assert!(file.ends_with(".safetensors"), "{file} is not a safetensors file");
            assert!(url.ends_with(file), "{url} does not end in the file it claims to fetch");
            assert!(size > 100_000_000, "{file} claims an implausible {size} bytes");
        }
    }

    /// A prompt containing quotes, backslashes or newlines must land as one
    /// correctly-escaped JSON string — the substitution is textual, so this
    /// is the case that would break it.
    #[test]
    fn prompt_with_json_metacharacters_survives_substitution() {
        let mut s = spec("image");
        s.prompt = "a sign saying \"hello\"\nwith a C:\\ path".to_string();
        let graph = fill_template(IMAGE_TEMPLATE, &s, Some("m.safetensors")).expect("fills");
        assert_eq!(graph["6"]["inputs"]["text"], json!("a sign saying \"hello\"\nwith a C:\\ path"));
    }

    #[test]
    fn checkpoint_choice_prefers_known_families_and_falls_back() {
        let installed = vec![
            "anything-v5.safetensors".to_string(),
            "sd_xl_base_1.0.safetensors".to_string(),
            "flux2-klein.safetensors".to_string(),
        ];
        assert_eq!(choose_checkpoint(&installed).as_deref(), Some("flux2-klein.safetensors"));
        let odd = vec!["my-finetune.ckpt".to_string()];
        assert_eq!(choose_checkpoint(&odd).as_deref(), Some("my-finetune.ckpt"));
        assert_eq!(choose_checkpoint(&[]), None);
    }

    #[test]
    fn job_spec_clamps_and_snaps_dimensions() {
        let req = GenerateRequest {
            kind: Some("image".into()),
            prompt: Some("x".into()),
            negative: None,
            width: Some(1000),
            height: Some(99999),
            length: None,
            enhance: None,
        };
        let s = job_spec(&req).unwrap();
        assert_eq!(s.width % 16, 0);
        assert!(s.height <= 2048);
        assert_eq!(s.length, 0, "images carry no frame count");

        let bad = GenerateRequest {
            kind: Some("audio".into()),
            prompt: Some("x".into()),
            negative: None,
            width: None,
            height: None,
            length: None,
            enhance: None,
        };
        assert!(job_spec(&bad).is_err());

        let missing = GenerateRequest {
            kind: None,
            prompt: None,
            negative: None,
            width: None,
            height: None,
            length: None,
            enhance: None,
        };
        assert!(job_spec(&missing).is_err(), "a prompt is required");
    }

    /// The three output-list spellings the save nodes use, plus the temp-file
    /// skip — `first_output` is what decides whether a finished job counts.
    #[test]
    fn first_output_scans_any_save_nodes_list_and_skips_temps() {
        let entry = json!({
            "outputs": {
                "9": { "images": [
                    { "filename": "preview.png", "subfolder": "", "type": "temp" },
                    { "filename": "agent-platform_00001_.png", "subfolder": "", "type": "output" }
                ]}
            }
        });
        let out = first_output(&entry).expect("finds the real output");
        assert_eq!(out.filename, "agent-platform_00001_.png");

        let video = json!({
            "outputs": { "61": { "videos": [
                { "filename": "agent-platform_00001.mp4", "subfolder": "video", "type": "output" }
            ]}}
        });
        assert_eq!(first_output(&video).unwrap().filename, "agent-platform_00001.mp4");
        assert!(first_output(&json!({ "outputs": {} })).is_none());
    }

    #[test]
    fn history_error_prefers_the_execution_message() {
        let status = json!({
            "status_str": "error",
            "messages": [
                ["execution_start", {}],
                ["execution_error", {
                    "node_type": "CheckpointLoaderSimple",
                    "exception_message": "Model not found: missing.safetensors"
                }]
            ]
        });
        assert_eq!(
            history_error(&status),
            "CheckpointLoaderSimple: Model not found: missing.safetensors"
        );
        assert_eq!(history_error(&json!({})), "ComfyUI reported an execution error.");
    }

    #[test]
    fn file_names_are_flattened_and_content_types_known() {
        assert_eq!(safe_file_name("../..\\evil.png"), "_.._evil.png");
        assert_eq!(safe_file_name("out put.mp4"), "out_put.mp4");
        assert_eq!(content_type("a.PNG"), "image/png");
        assert_eq!(content_type("clip.mp4"), "video/mp4");
        assert_eq!(content_type("weird.xyz"), "application/octet-stream");
    }
}
