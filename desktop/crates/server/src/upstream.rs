//! The Python server, as a child of this process.
//!
//! The desktop shell used to own this child directly; under ADR 0007 it owns
//! `agent-platformd` instead, and the daemon owns Python. The child inherits our
//! whole environment (DB path, workspace root, config dir, master key are all
//! already set by whoever started us) with only its bind address overridden — so
//! there is exactly one place those variables are defined.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::{env_opt, BoxError, Config};

#[cfg(windows)]
const RUNTIME_PYTHON: &str = "python.exe";
#[cfg(not(windows))]
const RUNTIME_PYTHON: &str = "bin/python3";

const HEALTH_TIMEOUT: Duration = Duration::from_secs(90);

pub struct Upstream {
    pub origin: String,
    child: Mutex<Option<Child>>,
    /// Dropped after the child is killed; see `job::KillOnClose`.
    #[cfg(windows)]
    _job: Option<job::KillOnClose>,
}

impl Upstream {
    /// A server we did not spawn: nothing to reap, and no liveness signal beyond
    /// asking it.
    pub fn attached(origin: impl Into<String>) -> Self {
        Self {
            origin: origin.into(),
            child: Mutex::new(None),
            #[cfg(windows)]
            _job: None,
        }
    }

    /// `None` when we do not own the server (attached to someone else's), so the
    /// caller knows to ask it rather than answer for it.
    pub fn child_alive(&self) -> Option<bool> {
        let mut guard = self.child.lock().unwrap();
        let child = guard.as_mut()?;
        Some(matches!(child.try_wait(), Ok(None)))
    }
}

impl Drop for Upstream {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Ties the Python child's lifetime to this process at the OS level.
///
/// `--exit-with-parent` alone is not enough: it watches stdin for EOF, and on
/// Windows that EOF does not arrive when the parent is terminated rather than
/// shut down. The desktop kills its server child on quit, so without this every
/// quit would leave a uvicorn holding the database and an ephemeral port — the
/// exact orphan the shell's attach-if-running logic exists to detect.
#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// Closing the handle — including when the OS closes it on process death —
    /// terminates every process in the job.
    pub struct KillOnClose(HANDLE);

    // The handle is owned solely by this value and only closed in Drop.
    unsafe impl Send for KillOnClose {}
    unsafe impl Sync for KillOnClose {}

    impl Drop for KillOnClose {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    pub fn attach(child: &Child) -> Option<KillOnClose> {
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return None;
            }
            let job = KillOnClose(handle);

            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) != 0
                && AssignProcessToJobObject(job.0, child.as_raw_handle() as HANDLE) != 0;

            ok.then_some(job)
        }
    }
}

/// Attaches to `AGENT_PLATFORM_UPSTREAM` when set, otherwise spawns Python on an
/// ephemeral loopback port and waits for it to answer `/health`.
///
/// `cfg` is the *public* address; the child gets an ephemeral one and is told the
/// public pair separately so `/api/v1/system/status` still reports the address
/// callers actually use.
/// Our own origin as the child should dial it. `AGENT_PLATFORM_HOST` is a bind
/// address: `0.0.0.0` means "every interface", which is not a destination — the
/// child would resolve it to nothing on Windows and to something surprising
/// elsewhere.
fn loopback_origin(cfg: &Config) -> String {
    let host = match cfg.host.as_str() {
        "0.0.0.0" | "::" | "[::]" | "" => "127.0.0.1",
        h => h,
    };
    format!("http://{}:{}", host, cfg.port)
}

pub async fn start(cfg: &Config) -> Result<Upstream, BoxError> {
    if let Some(origin) = &cfg.upstream {
        let origin = origin.trim_end_matches('/').to_string();
        wait_healthy(&origin, None).await?;
        return Ok(Upstream::attached(origin));
    }

    let (python, entry) = resolve_python().ok_or(
        "no Python server found: set AGENT_PLATFORM_PYTHON and AGENT_PLATFORM_PY_ENTRY, \
         or AGENT_PLATFORM_UPSTREAM to attach to one that is already running",
    )?;

    let port = free_port()?;
    let origin = format!("http://127.0.0.1:{port}");

    let mut cmd = Command::new(&python);
    cmd.arg(&entry)
        .arg("--skip-build")
        .arg("--no-browser")
        .arg("--exit-with-parent")
        .env("AGENT_PLATFORM_HOST", "127.0.0.1")
        .env("AGENT_PLATFORM_PORT", port.to_string())
        .env("AGENT_PLATFORM_PUBLIC_HOST", &cfg.host)
        .env("AGENT_PLATFORM_PUBLIC_PORT", cfg.port.to_string())
        // Chat, agents, coder and assistant reach the LLM proxy over HTTP. The
        // cutover (ADR 0007): point them at *us*, and switch the child's own `/v1`
        // router off, so there is one implementation of those nine routes rather
        // than two that can drift. One loopback hop is the price. Reverting is
        // these two lines — the package stays imported in the child either way,
        // because eight modules outside it use `llm_proxy.core` in process.
        .env("LLM_ORCHESTRATOR_BASE_URL", format!("{}/v1", loopback_origin(cfg)))
        .env("AGENT_PLATFORM_V1_ROUTER", "0")
        // The workflow engine and its scheduler are ours now. Two pollers on one
        // `workflows` table would each fire every due workflow, so the child's
        // loop is switched off rather than left racing this one.
        .env("AGENT_PLATFORM_WORKFLOW_SCHEDULER", "0")
        // Same reasoning, same shape: the DAG executor is ours, so the child must
        // not also requeue what the last shutdown stranded. Both servers running
        // recovery means every interrupted process gets planned twice.
        .env("AGENT_PLATFORM_RESUME_ON_STARTUP", "0")
        // Piped, and the handle is then left untouched inside `Child`: `--exit-with-parent`
        // watches fd 0 for EOF, so a null stdin makes the server exit(0) the instant it
        // starts, and closing the pipe is what makes it die with us if we are killed.
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    eprintln!("[agent-platformd] starting python upstream on {origin}");
    let child = cmd.spawn()?;
    #[cfg(windows)]
    let job = {
        let attached = job::attach(&child);
        if attached.is_none() {
            eprintln!("[agent-platformd] warning: could not put the python child in a job object; \
                       a hard kill of this process will leave it running");
        }
        attached
    };
    let up = Upstream {
        origin: origin.clone(),
        child: Mutex::new(Some(child)),
        #[cfg(windows)]
        _job: job,
    };
    wait_healthy(&origin, Some(&up)).await?;
    Ok(up)
}

async fn wait_healthy(origin: &str, up: Option<&Upstream>) -> Result<(), BoxError> {
    let http = reqwest::Client::new();
    let deadline = Instant::now() + HEALTH_TIMEOUT;
    loop {
        if up.and_then(Upstream::child_alive) == Some(false) {
            return Err("python server exited during startup".into());
        }
        if let Ok(resp) = http.get(format!("{origin}/health")).send().await {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err(format!("python server did not answer /health at {origin} in time").into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Same search order as the desktop shell's `resolve_server`: explicit env first,
/// then the bundled payload next to the exe, then a repo checkout.
fn resolve_python() -> Option<(PathBuf, PathBuf)> {
    if let (Some(py), Some(entry)) = (env_opt("AGENT_PLATFORM_PYTHON"), env_opt("AGENT_PLATFORM_PY_ENTRY")) {
        return Some((PathBuf::from(py), PathBuf::from(entry)));
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(root) = exe.parent().map(|p| p.join("server")) {
            let python = root.join("runtime").join(RUNTIME_PYTHON);
            let entry = root.join("scripts").join("start.py");
            if python.is_file() && entry.is_file() {
                return Some((python, entry));
            }
        }
    }

    // crates/server -> crates -> desktop -> repo root
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?.parent()?;
    let entry = repo.join("scripts").join("start.py");
    entry.is_file().then(|| {
        let python = if cfg!(windows) { "python" } else { "python3" };
        (PathBuf::from(python), entry)
    })
}

fn free_port() -> std::io::Result<u16> {
    // Classic bind-and-release race, same one every dev server takes. Losing it
    // means the child fails to bind and startup reports it, not a silent misroute.
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}
