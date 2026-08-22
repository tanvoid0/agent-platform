//! Downloading, launching and reaping `sd-server` (ADR 0011, step 2).
//!
//! [`crate::media_sdcpp`] talks to whatever is listening. This module is what
//! makes something listen — the half that actually removes the "go install an
//! app first" step, because a 39 MB MIT binary is something we can fetch and
//! run on the user's behalf in a way a 3.5 GB GPL Python environment is not.
//!
//! **Model arguments are configuration, not a table here.** `sd-server` binds
//! its model with flags that differ per family — some families are one
//! `-m checkpoint`, others need `--diffusion-model` plus `--vae` plus a text
//! encoder — and upstream adds families weekly. A lookup table in this crate
//! would be a contract with a moving target, which is exactly the treadmill
//! ADR 0011 names as sd.cpp's main risk; here it would be *our* treadmill.
//! So `MEDIA_SDCPP_ARGS` carries them verbatim, and a curated per-family table
//! can populate that variable later without this module changing. Until it is
//! set there is nothing to launch, and that is [`Stage::Unconfigured`] — a
//! first-class state, not an error (the ADR 0008 lesson).
//!
//! **Nothing is managed unless the base is loopback.** `MEDIA_API_BASE`
//! pointing at another machine means someone else owns that process; we probe
//! it and never spawn, download or kill.
//!
//! **Idle shutdown is not a nicety.** A loaded diffusion model holds VRAM for
//! as long as the process lives, and on the 16 GB card this targets that is
//! the same VRAM local chat inference wants. `MEDIA_SDCPP_IDLE_SECS` (default
//! 600, `0` disables) stops a server nobody has used.
//!
//! **Reaping.** `kill_on_drop` covers the ordinary path. It does not cover
//! this process being terminated rather than shut down, which is what the
//! Windows job object below is for — the same mechanism, and the same reason,
//! as the desktop's `shell.rs`. Without it a killed `agent-platformd` leaves a
//! multi-gigabyte GPU process behind.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use serde_json::Value;

use crate::error::ApiError;
use crate::managed_server::{self, Progress, Release, Tail};
use crate::AppState;

/// The release this server launches, pinned by tag.
///
/// stable-diffusion.cpp has **no semver** — releases are `master-<n>-<sha>` and
/// land most weeks. Tracking `latest` would mean a model that worked yesterday
/// failing today because an upstream flag was renamed, on a machine nobody is
/// watching. Moving this constant is a deliberate act with a test run attached.
const PINNED_RELEASE: &str = "master-827-97d2990";

/// Which release asset to fetch, by substring of its file name.
///
/// **Vulkan by default, on every platform that has it.** The CUDA build is
/// faster on NVIDIA but arrives as 336 MB plus a *separate* 563 MB cudart zip,
/// where the Vulkan build is 39 MB and needs nothing beside it — and it runs on
/// AMD and Intel too. `MEDIA_SDCPP_VARIANT` overrides for someone who wants the
/// CUDA build and will fetch its runtime themselves.
fn asset_pattern() -> String {
    let explicit = crate::llm_config::from_env_or_dotenv("MEDIA_SDCPP_VARIANT");
    if !explicit.trim().is_empty() {
        return explicit.trim().to_string();
    }
    if cfg!(target_os = "windows") {
        "bin-win-vulkan-x64".to_string()
    } else if cfg!(target_os = "macos") {
        "bin-Darwin".to_string()
    } else {
        "x86_64-vulkan".to_string()
    }
}

/// The pinned release this backend runs, unpacked under
/// `<media dir>/sdcpp/<release tag>` — tagged by release so changing
/// [`PINNED_RELEASE`] fetches beside the old copy rather than half-overwriting
/// it.
fn release() -> Release {
    Release {
        repo: "leejet/stable-diffusion.cpp",
        tag: PINNED_RELEASE,
        label: "sd-server",
        exe: exe_name(),
        asset: asset_pattern(),
        dir: crate::media::media_dir().join("sdcpp").join(PINNED_RELEASE),
    }
}

/// The last few lines sd-server wrote, for quoting when it dies on its way up.
static TAIL: Tail = Tail::new();

fn exe_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "sd-server.exe"
    } else {
        "sd-server"
    }
}

/// An explicit binary, for a user who built or fetched their own. Skips the
/// download entirely; everything else — spawn, health-wait, idle stop — still
/// applies.
fn configured_binary() -> Option<PathBuf> {
    let raw = crate::llm_config::from_env_or_dotenv("MEDIA_SDCPP_BIN");
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

/// The flags to launch with for this Studio kind.
///
/// `MEDIA_SDCPP_ARGS` wins when set — it is the escape hatch that keeps the
/// catalogue a convenience rather than a contract, and it is how a family the
/// catalogue has never heard of gets run. Otherwise an installed catalogue
/// model of the right kind supplies them, which is what makes fetching the
/// weights the only step a user takes.
///
/// Empty means there is nothing to launch, and that is [`Stage::Unconfigured`].
fn model_args(kind: &str) -> Vec<String> {
    let explicit = managed_server::split_args(&crate::llm_config::from_env_or_dotenv("MEDIA_SDCPP_ARGS"));
    if !explicit.is_empty() {
        return explicit;
    }
    catalogue_args(kind).unwrap_or_default()
}

/// How long to wait for `sd-server` to answer after spawning. Generous because
/// this covers reading a multi-gigabyte model off disk into VRAM, which on a
/// cold cache is minutes rather than seconds.
const START_TIMEOUT: Duration = Duration::from_secs(300);

fn idle_timeout() -> Option<Duration> {
    let raw = crate::llm_config::from_env_or_dotenv("MEDIA_SDCPP_IDLE_SECS");
    let secs: u64 = if raw.trim().is_empty() { 600 } else { raw.trim().parse().unwrap_or(600) };
    (secs > 0).then(|| Duration::from_secs(secs))
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// What the managed `sd-server` is doing, as reported by `GET /media/status`.
///
/// Every variant here is a thing the Studio screen can render as a sentence.
/// There is no "unknown".
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Stage {
    /// `MEDIA_API_BASE` points somewhere we do not own. Nothing is managed.
    External,
    /// No `MEDIA_SDCPP_ARGS`, so there is no model to launch with.
    Unconfigured,
    /// Configured, but the binary is not on disk yet.
    NotInstalled,
    Downloading { received: u64, total: u64 },
    Extracting,
    /// Spawned; waiting for it to answer.
    Starting,
    Ready,
    /// Installed and idle — stopped to give the VRAM back.
    Stopped,
    Failed(String),
}

impl Stage {
    fn name(&self) -> &'static str {
        match self {
            Stage::External => "external",
            Stage::Unconfigured => "unconfigured",
            Stage::NotInstalled => "not_installed",
            Stage::Downloading { .. } => "downloading",
            Stage::Extracting => "extracting",
            Stage::Starting => "starting",
            Stage::Ready => "ready",
            Stage::Stopped => "stopped",
            Stage::Failed(_) => "failed",
        }
    }
}

fn stage_cell() -> &'static Mutex<Stage> {
    static STAGE: OnceLock<Mutex<Stage>> = OnceLock::new();
    STAGE.get_or_init(|| Mutex::new(Stage::NotInstalled))
}

fn set_stage(next: Stage) {
    if let Ok(mut slot) = stage_cell().lock() {
        *slot = next;
    }
}

/// The spawned child, and the async gate that serialises everything in this
/// module. Held across the download and the health-wait on purpose: two
/// concurrent `POST /generate`s must not race into two downloads or two
/// servers fighting over one port.
fn gate() -> &'static tokio::sync::Mutex<Option<tokio::process::Child>> {
    static GATE: OnceLock<tokio::sync::Mutex<Option<tokio::process::Child>>> = OnceLock::new();
    GATE.get_or_init(|| tokio::sync::Mutex::new(None))
}

/// The arguments the currently-running child was launched with, so a request
/// for a different model can tell that a restart is needed. `None` when nothing
/// of ours is running.
fn running_args_cell() -> &'static Mutex<Option<Vec<String>>> {
    static ARGS: OnceLock<Mutex<Option<Vec<String>>>> = OnceLock::new();
    ARGS.get_or_init(|| Mutex::new(None))
}

fn running_args() -> Option<Vec<String>> {
    running_args_cell().lock().ok().and_then(|a| a.clone())
}

fn set_running_args(args: &[String]) {
    if let Ok(mut slot) = running_args_cell().lock() {
        *slot = Some(args.to_vec());
    }
}

fn last_used() -> &'static Mutex<Instant> {
    static LAST: OnceLock<Mutex<Instant>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(Instant::now()))
}

/// Bumped on every submit and every poll, so a long render counts as use and
/// the idle watchdog cannot kill a server mid-job.
pub(crate) fn note_used() {
    if let Ok(mut slot) = last_used().lock() {
        *slot = Instant::now();
    }
}

/// `(stage name, detail)` for `GET /media/status`. Cheap: a mutex read, no
/// probing, no awaiting — the status route already probes separately.
pub(crate) fn stage_report() -> (&'static str, Option<String>) {
    let stage = stage_cell().lock().map(|s| s.clone()).unwrap_or(Stage::NotInstalled);
    // `NotInstalled` is this cell's initial value, so a restart reports it even
    // when the binary is sitting on disk from a previous run. Correct it here
    // rather than probing at startup: installed-and-not-running is `Stopped`,
    // which is the sentence the screen should show.
    let stage = match stage {
        Stage::NotInstalled if configured_binary().is_some() || release().installed().is_some() => Stage::Stopped,
        other => other,
    };
    let detail = match &stage {
        Stage::Downloading { received, total } if *total > 0 => {
            Some(format!("{}% of {}", received * 100 / total, managed_server::human(*total)))
        }
        Stage::Downloading { received, .. } => Some(managed_server::human(*received)),
        Stage::Unconfigured => Some(
            "Set MEDIA_SDCPP_ARGS to the model flags sd-server should load with.".to_string(),
        ),
        Stage::Failed(why) => Some(why.clone()),
        _ => None,
    };
    (stage.name(), detail)
}

// ---------------------------------------------------------------------------
// The entry point
// ---------------------------------------------------------------------------

/// Make sure something is answering at `base` before a job is submitted.
///
/// Cheap and idempotent on the common path: one 2 s probe, and if a server is
/// already up this returns immediately. Otherwise it may download, unpack and
/// launch — which is why the caller is `media.rs`'s submit and not the status
/// probe. A status poll must never start a multi-gigabyte model load.
pub(crate) async fn ensure_running(
    state: &AppState,
    base: &str,
    kind: &str,
) -> Result<(), ApiError> {
    let Some(port) = managed_server::loopback_port(base) else {
        set_stage(Stage::External);
        return Ok(());
    };

    // Serialise before probing: two callers arriving together must not both
    // decide the server is missing and both spawn one.
    let mut child_slot = gate().lock().await;

    // Probe before deciding anything. **A server that is already answering
    // needs no launch arguments** — requiring them first was a bug the route
    // test caught: it turned a perfectly working backend into a 502 because
    // nothing had told us which model to start, when nothing needed starting.
    let reachable = crate::media_sdcpp::capabilities(state, base).await.is_some();
    let args = model_args(kind);

    if reachable {
        // A server we did not start is never restarted and never questioned:
        // it is somebody else's process that happens to be on our port.
        let ours = child_slot.is_some();
        // **One model per process, so a kind change is a restart** — an image
        // server cannot answer `vid_gen`, and learning that from sd-server
        // costs a submit and a failed job where comparing two argument lists
        // costs nothing. With no args to compare against, keep what is running:
        // a live server beats an error about configuration we do not need.
        let same_model = args.is_empty() || running_args().as_deref() == Some(&args[..]);
        if !ours || same_model {
            set_stage(Stage::Ready);
            note_used();
            start_idle_watchdog();
            return Ok(());
        }
        logd!("[media] restarting sd-server for a {kind} model");
    } else if args.is_empty() {
        // Nothing listening and nothing to launch: the state a fresh install is
        // in, and a sentence rather than a spinner.
        set_stage(Stage::Unconfigured);
        return Err(ApiError::coded(
            StatusCode::BAD_GATEWAY,
            "media_backend_unconfigured",
            format!(
                "No sd-server is running and there is no {kind} model to launch one with.                  Install one through GET /api/v1/media/models, or set MEDIA_SDCPP_ARGS to                  the flags sd-server should load a model with."
            ),
        ));
    }

    // A child we are still holding, that is not answering — or is answering
    // with the wrong model loaded — is one we replace.
    if let Some(mut old) = child_slot.take() {
        let _ = old.kill().await;
        // sd-server has to release the port before the replacement binds it.
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    let binary = match configured_binary() {
        Some(path) => path,
        None => ensure_installed(state).await?,
    };

    // The address is ours to choose, so it is appended rather than being part
    // of the model arguments a user or the catalogue supplies.
    let mut launch = args.clone();
    launch.extend(["--listen-ip".to_string(), "127.0.0.1".to_string()]);
    launch.extend(["--listen-port".to_string(), port.to_string()]);

    set_stage(Stage::Starting);
    let mut child = managed_server::spawn(&binary, &launch, "sd-server", &TAIL).map_err(|e| {
        let why = format!("sd-server could not be started: {e}");
        set_stage(Stage::Failed(why.clone()));
        ApiError::coded(StatusCode::BAD_GATEWAY, "media_backend_unreachable", why)
    })?;

    // Waited on *before* being stored, so a child that dies on the way up is
    // dropped here — `kill_on_drop` reaps it — rather than being parked in the
    // slot for the next caller to find and mistake for a running server.
    let wait = managed_server::health_wait(&mut child, START_TIMEOUT, "sd-server", &TAIL, || {
        async { crate::media_sdcpp::capabilities(state, base).await.is_some() }
    })
    .await;
    if let Err(why) = wait {
        set_stage(Stage::Failed(why.clone()));
        return Err(ApiError::coded(StatusCode::BAD_GATEWAY, "media_backend_unreachable", why));
    }
    set_running_args(&args);
    *child_slot = Some(child);

    set_stage(Stage::Ready);
    note_used();
    start_idle_watchdog();
    Ok(())
}

// ---------------------------------------------------------------------------
// Install: fetch the pinned release and unpack it
// ---------------------------------------------------------------------------

/// The unpacked `sd-server` binary, fetching the pinned release first if it is
/// not already on disk.
async fn ensure_installed(state: &AppState) -> Result<PathBuf, ApiError> {
    let release = release();
    if release.installed().is_none() {
        set_stage(Stage::NotInstalled);
    }
    release
        .install(state, &|progress| {
            set_stage(match progress {
                Progress::Downloading { received, total } => Stage::Downloading { received, total },
                Progress::Extracting => Stage::Extracting,
            })
        })
        .await
        .map_err(|why| {
            set_stage(Stage::Failed(why.clone()));
            ApiError::coded(StatusCode::BAD_GATEWAY, "media_backend_install_failed", why)
        })
}

// ---------------------------------------------------------------------------
// Spawn, wait, reap
// ---------------------------------------------------------------------------

/// Stops a server nobody has used, to give the VRAM back. One task for the life
/// of the process, started after the first successful launch.
fn start_idle_watchdog() {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let Some(limit) = idle_timeout() else { continue };
            let idle = last_used().lock().map(|t| t.elapsed()).unwrap_or_default();
            if idle < limit {
                continue;
            }
            let mut slot = gate().lock().await;
            // Re-checked under the lock: a job may have started while this task
            // was waiting for it, and killing a server mid-render would fail a
            // job that was about to succeed.
            let idle = last_used().lock().map(|t| t.elapsed()).unwrap_or_default();
            if idle < limit {
                continue;
            }
            if let Some(mut child) = slot.take() {
                let _ = child.kill().await;
                if let Ok(mut slot) = running_args_cell().lock() {
                    *slot = None;
                }
                set_stage(Stage::Stopped);
                logd!("[media] stopped sd-server after {}s idle", idle.as_secs());
            }
        }
    });
}

// ---------------------------------------------------------------------------
// The model catalogue (ADR 0011, step 3)
// ---------------------------------------------------------------------------
//
// **This table fills the launch arguments; it is not a second way to launch.**
// The distinction matters, and is why the launcher above knows nothing about
// model families: if an entry here goes stale, a user still sets
// `MEDIA_SDCPP_ARGS` by hand and everything works. A catalogue is a convenience
// with an escape hatch, which is a different thing from a contract with
// upstream.
//
// Sizes are real `content-length` values, checked against the HuggingFace API
// rather than estimated, because a confirm step that says "6.4 GB" and spends
// twelve is worse than one that says nothing.
//
// Every URL is **ungated**. FLUX.1-schnell's `ae.safetensors` — which
// stable-diffusion.cpp's own Z-Image doc links for the VAE — answers 401
// without a token, so the Comfy-Org repackage of the same file is used instead.
// A catalogue entry that walks the user into an auth wall is not a catalogue
// entry.

/// One file a model needs: the flag it is passed as, where it comes from, and
/// what it costs.
struct ModelFile {
    /// The `sd-server` flag this file is the value for.
    flag: &'static str,
    file_name: &'static str,
    url: &'static str,
    size_bytes: u64,
}

/// A launchable model: the files, plus whatever extra flags its family needs.
struct CatalogueModel {
    id: &'static str,
    label: &'static str,
    /// `image` or `video` — which Studio kind this model serves.
    kind: &'static str,
    note: &'static str,
    files: &'static [ModelFile],
    /// Flags that are properties of the *model*, not of the request. Z-Image
    /// Turbo is distilled and wants `--cfg-scale 1.0`; a full model's 3.5 would
    /// wash it out. This is exactly the per-family knowledge that must not go
    /// into a generation request, which is why it lives beside the weights.
    extra_args: &'static [&'static str],
}

/// Deliberately short: two entries that fit the 16 GB card this targets, one
/// per modality. Not an attempt to mirror everything sd.cpp supports, which is
/// the treadmill this design exists to stay off.
const CATALOGUE: &[CatalogueModel] = &[
    CatalogueModel {
        id: "z-image-turbo",
        label: "Z-Image Turbo",
        kind: "image",
        note: "Distilled: eight steps, and it fits comfortably in 16 GB.",
        files: &[
            ModelFile {
                flag: "--diffusion-model",
                file_name: "z_image_turbo-Q4_K.gguf",
                url: "https://huggingface.co/leejet/Z-Image-Turbo-GGUF/resolve/main/z_image_turbo-Q4_K.gguf",
                size_bytes: 3_864_250_304,
            },
            ModelFile {
                flag: "--vae",
                file_name: "ae.safetensors",
                url: "https://huggingface.co/Comfy-Org/z_image_turbo/resolve/main/split_files/vae/ae.safetensors",
                size_bytes: 335_304_388,
            },
            ModelFile {
                flag: "--llm",
                file_name: "Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
                url: "https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/main/Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
                size_bytes: 2_497_281_120,
            },
        ],
        extra_args: &["--cfg-scale", "1.0", "--steps", "8", "--diffusion-fa", "--offload-to-cpu"],
    },
    CatalogueModel {
        id: "wan-2.2-ti2v-5b",
        label: "Wan 2.2 TI2V 5B",
        kind: "video",
        note: "Minutes per clip on a consumer card. The same files ComfyUI's own template uses.",
        files: &[
            ModelFile {
                flag: "--diffusion-model",
                file_name: "wan2.2_ti2v_5B_fp16.safetensors",
                url: "https://huggingface.co/Comfy-Org/Wan_2.2_ComfyUI_Repackaged/resolve/main/split_files/diffusion_models/wan2.2_ti2v_5B_fp16.safetensors",
                size_bytes: 9_999_658_848,
            },
            ModelFile {
                flag: "--vae",
                file_name: "wan2.2_vae.safetensors",
                url: "https://huggingface.co/Comfy-Org/Wan_2.2_ComfyUI_Repackaged/resolve/main/split_files/vae/wan2.2_vae.safetensors",
                size_bytes: 1_409_400_960,
            },
            ModelFile {
                flag: "--t5xxl",
                file_name: "umt5_xxl_fp8_e4m3fn_scaled.safetensors",
                url: "https://huggingface.co/Comfy-Org/Wan_2.2_ComfyUI_Repackaged/resolve/main/split_files/text_encoders/umt5_xxl_fp8_e4m3fn_scaled.safetensors",
                size_bytes: 6_735_906_897,
            },
        ],
        extra_args: &["--diffusion-fa", "--offload-to-cpu", "--vae-tiling"],
    },
];

/// Where a catalogue model's files live: `<media dir>/models/<model id>/`.
/// Per-model rather than one flat folder, so two models cannot collide on a
/// file name and "is this installed" is a directory listing.
fn model_dir(id: &str) -> PathBuf {
    crate::media::media_dir().join("models").join(id)
}

fn catalogue_model(id: &str) -> Option<&'static CatalogueModel> {
    CATALOGUE.iter().find(|m| m.id == id)
}

/// **Size-checked, not merely present.** A half-written file left by a killed
/// download would otherwise read as installed and fail minutes later at model
/// load, which is the worst place to find out.
fn file_installed(model: &CatalogueModel, file: &ModelFile) -> bool {
    let path = model_dir(model.id).join(file.file_name);
    std::fs::metadata(&path).is_ok_and(|m| m.len() == file.size_bytes)
}

fn model_installed(model: &CatalogueModel) -> bool {
    model.files.iter().all(|f| file_installed(model, f))
}

/// The launch arguments for an installed catalogue model of this kind.
///
/// This is what makes fetching the weights enough: no configuration to write,
/// no setting to persist, no `.env` to edit. The files on disk *are* the state.
fn catalogue_args(kind: &str) -> Option<Vec<String>> {
    let model = CATALOGUE.iter().find(|m| m.kind == kind && model_installed(m))?;
    let dir = model_dir(model.id);
    let mut args = Vec::new();
    for file in model.files {
        args.push(file.flag.to_string());
        args.push(dir.join(file.file_name).to_string_lossy().into_owned());
    }
    args.extend(model.extra_args.iter().map(|a| (*a).to_string()));
    Some(args)
}

/// What `GET /api/v1/media/models` reports: what exists, what it costs, and
/// what is already on disk.
pub(crate) fn catalogue_report() -> Value {
    let models: Vec<Value> = CATALOGUE
        .iter()
        .map(|m| {
            let files: Vec<Value> = m
                .files
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "file_name": f.file_name,
                        "url": f.url,
                        "size_bytes": f.size_bytes,
                        "installed": file_installed(m, f),
                    })
                })
                .collect();
            serde_json::json!({
                "id": m.id,
                "label": m.label,
                "kind": m.kind,
                "note": m.note,
                "installed": model_installed(m),
                "total_bytes": m.files.iter().map(|f| f.size_bytes).sum::<u64>(),
                "directory": model_dir(m.id).to_string_lossy(),
                "files": files,
            })
        })
        .collect();
    serde_json::json!({ "models": models })
}

/// Fetch every missing file for one catalogue model.
///
/// Runs under the same gate as a launch, so an install cannot race a generation
/// into two downloads of one file. Already-installed files are skipped, which
/// makes a re-run after a failure resume at file granularity rather than
/// starting the whole ten gigabytes again.
pub(crate) async fn install_model(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let model = catalogue_model(id).ok_or_else(|| {
        ApiError::bad_request(format!(
            "No model {id:?}. Known ids: {}.",
            CATALOGUE.iter().map(|m| m.id).collect::<Vec<_>>().join(", ")
        ))
    })?;

    let _guard = gate().lock().await;
    let dir = model_dir(model.id);
    for file in model.files {
        if file_installed(model, file) {
            continue;
        }
        set_stage(Stage::Downloading { received: 0, total: file.size_bytes });
        let dest = dir.join(file.file_name);
        managed_server::download(state, file.url, &dest, file.size_bytes, file.file_name, &|p| {
            if let Progress::Downloading { received, total } = p {
                set_stage(Stage::Downloading { received, total });
            }
        })
        .await
        .map_err(|why| {
            set_stage(Stage::Failed(why.clone()));
            ApiError::coded(StatusCode::BAD_GATEWAY, "media_model_download_failed", why)
        })?;
        logd!("[media] fetched {} for {}", file.file_name, model.id);
    }
    set_stage(Stage::Stopped);
    Ok(serde_json::json!({ "id": model.id, "installed": model_installed(model) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalogue is data, and a typo in data fails at model-load time on a
    /// user's machine rather than here. This is the cheap guard: every entry
    /// well-formed, every id unique, every size real.
    #[test]
    fn every_catalogue_entry_is_well_formed() {
        let mut ids = std::collections::HashSet::new();
        for model in CATALOGUE {
            assert!(ids.insert(model.id), "duplicate catalogue id {}", model.id);
            assert!(
                matches!(model.kind, "image" | "video"),
                "{} has kind {:?}, which no Studio toggle asks for",
                model.id,
                model.kind
            );
            assert!(!model.files.is_empty(), "{} lists no files", model.id);
            for file in model.files {
                assert!(
                    file.flag.starts_with('-'),
                    "{} maps {} to {:?}, which is not a flag",
                    model.id, file.file_name, file.flag
                );
                assert!(
                    file.url.starts_with("https://huggingface.co/") && file.url.contains("/resolve/"),
                    "{} points {} at {:?}, which is not a direct HuggingFace download",
                    model.id, file.file_name, file.url
                );
                assert!(
                    file.url.ends_with(file.file_name),
                    "{}: the saved name {:?} must match what the URL serves ({:?}), or the installed-check compares the wrong file",
                    model.id, file.file_name, file.url
                );
                assert!(
                    file.size_bytes > 1_000_000,
                    "{} has an implausible size for {}",
                    model.id, file.file_name
                );
            }
            // Not a pairing rule: sd-server mixes value flags (`--cfg-scale 1.0`)
            // with boolean ones (`--diffusion-fa`, `--vae-tiling`), so an odd
            // count is normal and asserting otherwise fails on correct data.
            // What is checkable without encoding sd-server's whole CLI here —
            // which is the treadmill this design exists to avoid — is that the
            // list starts with a flag and holds nothing blank.
            if let Some(first) = model.extra_args.first() {
                assert!(
                    first.starts_with('-'),
                    "{}'s extra args start with a value, not a flag: {:?}",
                    model.id, model.extra_args
                );
            }
            assert!(
                model.extra_args.iter().all(|a| !a.trim().is_empty()),
                "{} has a blank extra arg: {:?}",
                model.id, model.extra_args
            );
        }
    }

    /// Both modalities have to be reachable, or one half of Studio has no way
    /// to get a model at all.
    #[test]
    fn the_catalogue_covers_both_kinds() {
        for kind in ["image", "video"] {
            assert!(
                CATALOGUE.iter().any(|m| m.kind == kind),
                "nothing in the catalogue serves {kind}"
            );
        }
    }

    #[test]
    fn every_stage_has_a_name() {
        for stage in [
            Stage::External,
            Stage::Unconfigured,
            Stage::NotInstalled,
            Stage::Downloading { received: 1, total: 2 },
            Stage::Extracting,
            Stage::Starting,
            Stage::Ready,
            Stage::Stopped,
            Stage::Failed("x".into()),
        ] {
            assert!(!stage.name().is_empty());
        }
    }
}
