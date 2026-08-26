//! Downloading, launching and reaping `llama-server` — provider `local`
//! ([ADR 0012](../../../../docs/adr/0012-managed-llama-server.md)).
//!
//! Provider `local` used to mean llama.cpp *linked into* whichever binary was
//! built with `--features local-llm` — off by default, so a shipped daemon had
//! no local model at all and "local" in practice meant an Ollama the user
//! installed themselves. This module makes it mean what `sd-server` already
//! means for images: a pinned upstream binary that `agent-platformd` fetches,
//! starts, watches, logs and stops on the user's behalf. The mechanism is
//! shared with the media backend — see [`crate::managed_server`] — and only the
//! policy below is llama-specific.
//!
//! **Configuration is the two keys that already existed.** `LOCAL_MODEL_PATH`
//! and `LOCAL_N_CTX` are what the desktop's in-process engine reads, and a user
//! who has set them keeps their setup: same file, same context, different
//! process. Unset is [`Stage::Unconfigured`] — a named state with an actionable
//! sentence, never a silent download that arrives at the same error 35 MB
//! later.
//!
//! **Nothing is managed unless `LOCAL_API_BASE` is loopback.** A remote base is
//! someone else's llama-server: probe it, never spawn, download or kill it.
//!
//! **Idle shutdown gives the VRAM back.** A resident 9 GB model is 9 GB the
//! image backend cannot have, on the same card. `LOCAL_LLM_IDLE_SECS` (default
//! 600, `0` disables) stops a server nobody is talking to.
//!
//! ponytail: the two managed servers do not arbitrate VRAM with each other —
//! each only frees its own on its own idle timer. Wire a mutual stop if
//! generating an image while a chat model is resident turns out to OOM in
//! practice rather than in theory.
//!
//! **A slow local turn is usually a full card, and the default log level does
//! not say so.** b10549 fits its own parameters to free VRAM and quietly keeps
//! the layers it cannot offload on the CPU — measured here at 0.54 tok/s with a
//! 9 GB model against 7 GB free, with nothing in the ring to explain it. The
//! lines that explain it (`device_info`, `common_params_fit_impl: projected to
//! use N MiB`) need `-lv 4`, which also emits ~200 lines of GGUF metadata per
//! load. ponytail: not on by default for that reason — add `-lv 4` to
//! `LOCAL_LLM_ARGS` when a turn is inexplicably slow, and filter the drain in
//! [`crate::managed_server::spawn`] if it should ever be always-on.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::http::StatusCode;

use crate::error::ApiError;
use crate::managed_server::{self, Progress, Release, Tail};
use crate::AppState;

/// The release this server launches, pinned by tag.
///
/// llama.cpp has **no semver** — releases are `bNNNN` and land most days, with
/// prereleases in between. Tracking the tip would mean a model that worked
/// yesterday failing today because an upstream flag was renamed, on a machine
/// nobody is watching. Moving this constant is a deliberate act with a test run
/// attached.
const PINNED_RELEASE: &str = "b10549";

/// Every layer on the GPU, the same constant the in-process engine used: the
/// ADR 0006 spike measured 123 tok/s that way against 11 on CPU, so a partial
/// offload is not worth offering as a setting yet.
const N_GPU_LAYERS: &str = "999";

/// How long to wait for `llama-server` to answer after spawning. Generous
/// because this covers reading a multi-gigabyte model off disk into VRAM, which
/// on a cold cache is minutes rather than seconds.
const START_TIMEOUT: Duration = Duration::from_secs(300);

/// Which release asset to fetch, by substring of its file name.
///
/// **Vulkan by default where there is a Vulkan build.** The CUDA builds are
/// faster on NVIDIA but arrive as 147–251 MB plus a *separate* 391 MB cudart
/// zip, where the Vulkan build is 35 MB and needs nothing beside it — and it
/// runs on AMD and Intel too. macOS has no Vulkan asset and its native build is
/// Metal anyway. `LOCAL_LLM_VARIANT` overrides for someone who wants the CUDA
/// build and will fetch its runtime themselves.
fn asset_pattern() -> String {
    let explicit = crate::llm_config::from_env_or_dotenv("LOCAL_LLM_VARIANT");
    if !explicit.trim().is_empty() {
        return explicit.trim().to_string();
    }
    if cfg!(target_os = "windows") {
        "bin-win-vulkan-x64".to_string()
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "bin-macos-arm64".to_string()
        } else {
            "bin-macos-x64".to_string()
        }
    } else if cfg!(target_arch = "aarch64") {
        "bin-ubuntu-vulkan-arm64".to_string()
    } else {
        "bin-ubuntu-vulkan-x64".to_string()
    }
}

fn exe_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

fn release() -> Release {
    Release {
        repo: "ggml-org/llama.cpp",
        tag: PINNED_RELEASE,
        label: "llama-server",
        exe: exe_name(),
        asset: asset_pattern(),
        // Tagged by release so changing the pin fetches beside the old copy
        // rather than half-overwriting it.
        dir: crate::llm_config::config_dir().join("llama").join(PINNED_RELEASE),
    }
}

/// An explicit binary, for a user who built or fetched their own. Skips the
/// download entirely; everything else — spawn, health-wait, idle stop — still
/// applies.
fn configured_binary() -> Option<PathBuf> {
    let raw = crate::llm_config::from_env_or_dotenv("LOCAL_LLM_BIN");
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

/// The configured GGUF, if it is on disk. The file check is what keeps
/// `local` out of the provider list on a machine where the path is stale.
pub(crate) fn model_path() -> Option<PathBuf> {
    let raw = crate::llm_config::from_env_or_dotenv("LOCAL_MODEL_PATH");
    let path = PathBuf::from(raw.trim());
    (!raw.trim().is_empty() && path.is_file()).then_some(path)
}

/// What `/v1/models` and the provider catalog call this model: the GGUF's file
/// stem, which is also the alias `llama-server` is launched with, so the name
/// on the wire and the name in the catalog are the same string.
pub(crate) fn model_id() -> Option<String> {
    Some(model_path()?.file_stem()?.to_string_lossy().into_owned())
}

/// Is provider `local` configured at all?
///
/// A loopback base with no model is not: there would be nothing to launch. A
/// remote base is, unconditionally — someone else already runs it.
pub(crate) fn configured() -> bool {
    if managed_server::loopback_port(&crate::llm_config::local_api_base()).is_none() {
        return true;
    }
    model_path().is_some() || !crate::llm_config::from_env_or_dotenv("LOCAL_LLM_ARGS").trim().is_empty()
}

fn n_ctx() -> u32 {
    crate::llm_config::from_env_or_dotenv("LOCAL_N_CTX").trim().parse().unwrap_or(8192)
}

/// The flags to launch with, minus the address (which [`ensure_running`] adds).
///
/// `LOCAL_LLM_ARGS` wins when set — the escape hatch for a flag this does not
/// know about, and the way a second model or a draft model gets configured
/// without this module learning about either. Otherwise the two settings that
/// already existed become the four flags they imply.
///
/// Empty means there is nothing to launch, and that is [`Stage::Unconfigured`].
fn model_args() -> Vec<String> {
    let explicit =
        managed_server::split_args(&crate::llm_config::from_env_or_dotenv("LOCAL_LLM_ARGS"));
    if !explicit.is_empty() {
        return explicit;
    }
    let Some(path) = model_path() else { return Vec::new() };
    let mut args: Vec<String> = vec![
        "-m".into(),
        path.to_string_lossy().into_owned(),
        "-c".into(),
        n_ctx().to_string(),
        "-ngl".into(),
        N_GPU_LAYERS.into(),
        // The chat template out of the GGUF, which is also what turns `tools`
        // in the request into `tool_calls` in the reply. Without it a tool
        // definition is silently ignored and an agent turn comes back as prose.
        "--jinja".into(),
    ];
    if let Some(id) = model_id() {
        args.push("-a".into());
        args.push(id);
    }
    args
}

fn idle_timeout() -> Option<Duration> {
    let raw = crate::llm_config::from_env_or_dotenv("LOCAL_LLM_IDLE_SECS");
    let secs: u64 = if raw.trim().is_empty() { 600 } else { raw.trim().parse().unwrap_or(600) };
    (secs > 0).then(|| Duration::from_secs(secs))
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// What the managed `llama-server` is doing, as reported to the app.
///
/// Every variant is a thing a screen can render as a sentence. There is no
/// "unknown".
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Stage {
    /// `LOCAL_API_BASE` points somewhere we do not own. Nothing is managed.
    External,
    /// No `LOCAL_MODEL_PATH`, so there is no model to launch with.
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

/// The spawned child, and the async gate that serialises everything here. Held
/// across the download and the health-wait on purpose: two concurrent chat
/// turns must not race into two downloads or two servers fighting over a port.
fn gate() -> &'static tokio::sync::Mutex<Option<tokio::process::Child>> {
    static GATE: OnceLock<tokio::sync::Mutex<Option<tokio::process::Child>>> = OnceLock::new();
    GATE.get_or_init(|| tokio::sync::Mutex::new(None))
}

/// The arguments the running child was launched with, so a settings change can
/// tell that a restart is needed. `None` when nothing of ours is running.
fn running_args_cell() -> &'static Mutex<Option<Vec<String>>> {
    static ARGS: OnceLock<Mutex<Option<Vec<String>>>> = OnceLock::new();
    ARGS.get_or_init(|| Mutex::new(None))
}

fn running_args() -> Option<Vec<String>> {
    running_args_cell().lock().ok().and_then(|a| a.clone())
}

fn last_used() -> &'static Mutex<Instant> {
    static LAST: OnceLock<Mutex<Instant>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(Instant::now()))
}

/// Bumped on every turn, so the idle watchdog cannot stop a server that is
/// answering. A long stream counts as use when it starts; the timeout is
/// minutes and a reply is not.
pub(crate) fn note_used() {
    if let Ok(mut slot) = last_used().lock() {
        *slot = Instant::now();
    }
}

static TAIL: Tail = Tail::new();

/// `(stage name, detail)` for the provider admin row. Cheap: a mutex read, no
/// probing, no awaiting — nothing here may start a multi-gigabyte model load.
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
        Stage::Unconfigured => {
            Some("Set LOCAL_MODEL_PATH to a GGUF file to run a local model.".to_string())
        }
        Stage::Failed(why) => Some(why.clone()),
        _ => None,
    };
    (stage.name(), detail)
}

// ---------------------------------------------------------------------------
// The entry point
// ---------------------------------------------------------------------------

/// Make sure something is answering at `LOCAL_API_BASE` before a chat body is
/// sent there.
///
/// Cheap and idempotent on the common path: one probe, and if a server is
/// already up this returns immediately. Otherwise it may download, unpack and
/// launch — which is why the caller is the chat route's target resolution and
/// not a status poll.
pub(crate) async fn ensure_running(state: &AppState) -> Result<(), ApiError> {
    let base = crate::llm_config::local_api_base();
    let Some(port) = managed_server::loopback_port(&base) else {
        set_stage(Stage::External);
        return Ok(());
    };

    // Serialise before probing: two callers arriving together must not both
    // decide the server is missing and both spawn one.
    let mut child_slot = gate().lock().await;

    // Probe before deciding anything. A server that is already answering needs
    // no launch arguments — requiring them first would turn a perfectly good
    // backend into a 502 because nothing had told us which model to start,
    // when nothing needed starting.
    let reachable = healthy(state, &base).await;
    let args = model_args();

    if reachable {
        // A server we did not start is never restarted and never questioned:
        // it is somebody else's process that happens to be on our port.
        let ours = child_slot.is_some();
        // One model per process, so a model change is a restart. With no args
        // to compare against, keep what is running: a live server beats an
        // error about configuration we do not need.
        let same_model = args.is_empty() || running_args().as_deref() == Some(&args[..]);
        if !ours || same_model {
            set_stage(Stage::Ready);
            note_used();
            start_idle_watchdog();
            return Ok(());
        }
        logd!("[llm] restarting llama-server for a different model");
    } else if args.is_empty() {
        // Nothing listening and nothing to launch: the state a fresh install is
        // in, and a sentence rather than a spinner.
        set_stage(Stage::Unconfigured);
        return Err(ApiError::coded(
            StatusCode::BAD_GATEWAY,
            "local_llm_unconfigured",
            "No llama-server is running and there is no model to launch one with. Set \
             LOCAL_MODEL_PATH to a GGUF file, or LOCAL_LLM_ARGS to the flags llama-server \
             should load one with.",
        ));
    }

    // A child we are still holding, that is not answering — or is answering
    // with the wrong model loaded — is one we replace.
    if let Some(mut old) = child_slot.take() {
        let _ = old.kill().await;
        // It has to release the port before the replacement binds it.
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    let binary = match configured_binary() {
        Some(path) => path,
        None => {
            let release = release();
            if release.installed().is_none() {
                set_stage(Stage::NotInstalled);
            }
            release.install(state, &|progress| {
                set_stage(match progress {
                    Progress::Downloading { received, total } => {
                        Stage::Downloading { received, total }
                    }
                    Progress::Extracting => Stage::Extracting,
                })
            })
            .await
            .map_err(|why| {
                set_stage(Stage::Failed(why.clone()));
                ApiError::coded(StatusCode::BAD_GATEWAY, "local_llm_install_failed", why)
            })?
        }
    };

    let mut launch = args.clone();
    launch.extend(["--host".to_string(), "127.0.0.1".to_string()]);
    launch.extend(["--port".to_string(), port.to_string()]);

    set_stage(Stage::Starting);
    let mut child = managed_server::spawn(&binary, &launch, "llama-server", &TAIL).map_err(|e| {
        let why = format!("llama-server could not be started: {e}");
        set_stage(Stage::Failed(why.clone()));
        ApiError::coded(StatusCode::BAD_GATEWAY, "local_llm_unreachable", why)
    })?;

    // Waited on *before* being stored, so a child that dies on the way up is
    // dropped here — `kill_on_drop` reaps it — rather than being parked in the
    // slot for the next caller to find and mistake for a running server.
    let wait = managed_server::health_wait(
        &mut child,
        START_TIMEOUT,
        "llama-server",
        &TAIL,
        || healthy(state, &base),
    )
    .await;
    if let Err(why) = wait {
        set_stage(Stage::Failed(why.clone()));
        return Err(ApiError::coded(StatusCode::BAD_GATEWAY, "local_llm_unreachable", why));
    }

    if let Ok(mut slot) = running_args_cell().lock() {
        *slot = Some(args);
    }
    *child_slot = Some(child);

    set_stage(Stage::Ready);
    note_used();
    start_idle_watchdog();
    Ok(())
}

/// `GET /health` — llama-server answers 200 only once the model is resident,
/// and 503 `{"status":"loading model"}` while it is still reading weights, so
/// this is exactly the "can it take a turn" question.
async fn healthy(state: &AppState, base: &str) -> bool {
    let url = format!("{}/health", base.trim_end_matches('/'));
    matches!(
        state.http.get(&url).timeout(Duration::from_secs(2)).send().await,
        Ok(r) if r.status().is_success()
    )
}

/// Stops a server nobody is talking to, to give the VRAM back. One task for the
/// life of the process, started after the first successful launch.
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
            // Re-checked under the lock: a turn may have started while this
            // task was waiting for it.
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
                logd!("[llm] stopped llama-server after {}s idle", idle.as_secs());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two settings that already existed become the flags they imply — and
    /// `--jinja` is not optional, because without it a `tools` array comes back
    /// as prose instead of a tool call.
    #[test]
    fn the_launch_flags_come_from_the_model_settings() {
        // Held for the whole test: `LOCAL_MODEL_PATH` pointing at a real file is
        // exactly what makes `local` a *configured* provider, and `llm_config`'s
        // capability routing asserts it is not. See `crate::ENV_LOCK`.
        let _env = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let temp = std::env::temp_dir().join("agp-llama-args-test.gguf");
        std::fs::write(&temp, b"GGUF").unwrap();
        // SAFETY: the lock above is what makes this the only thread writing the
        // environment; these are read through `from_env_or_dotenv`, which does
        // not cache env reads.
        unsafe {
            std::env::set_var("LOCAL_MODEL_PATH", &temp);
            std::env::set_var("LOCAL_N_CTX", "4096");
            std::env::remove_var("LOCAL_LLM_ARGS");
        }
        let args = model_args();
        assert!(args.windows(2).any(|w| w[0] == "-m" && w[1] == temp.to_string_lossy()));
        assert!(args.windows(2).any(|w| w[0] == "-c" && w[1] == "4096"));
        assert!(args.iter().any(|a| a == "--jinja"), "tools need the chat template");
        assert_eq!(model_id().as_deref(), Some("agp-llama-args-test"));

        unsafe {
            std::env::remove_var("LOCAL_MODEL_PATH");
            std::env::remove_var("LOCAL_N_CTX");
        }
        let _ = std::fs::remove_file(&temp);

        // And with nothing configured there is nothing to launch — a named
        // state, not a download that arrives at the same error 35 MB later.
        // Asserted here rather than in its own test because these two write the
        // same environment variable, and two tests that do that race.
        assert!(model_args().is_empty());
    }

    /// The asset pattern has to name a build that exists for this platform, or
    /// the install stops at "no asset matching".
    #[test]
    fn the_asset_pattern_names_a_real_build() {
        unsafe { std::env::remove_var("LOCAL_LLM_VARIANT") };
        let pattern = asset_pattern();
        assert!(pattern.starts_with("bin-"), "{pattern}");
        assert!(release().exe.starts_with("llama-server"));
    }

    /// The stage sentence is what a screen renders, so every variant has to
    /// produce one rather than an empty string.
    #[test]
    fn an_unconfigured_stage_says_what_to_set() {
        set_stage(Stage::Unconfigured);
        let (name, detail) = stage_report();
        assert_eq!(name, "unconfigured");
        assert!(detail.unwrap().contains("LOCAL_MODEL_PATH"));
        set_stage(Stage::NotInstalled);
    }
}
