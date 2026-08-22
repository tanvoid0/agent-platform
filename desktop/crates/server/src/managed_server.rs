//! The servers `agent-platformd` runs on the user's behalf.
//!
//! `sd-server` ([ADR 0011](../../../../docs/adr/0011-stable-diffusion-cpp-media-backend.md))
//! and `llama-server` ([ADR 0012](../../../../docs/adr/0012-managed-llama-server.md))
//! are the same sentence with different nouns: fetch a pinned upstream release,
//! spawn it on loopback, watch it come up, put its stderr in our log ring, stop
//! it when it goes idle, and reap it when this process dies. That is
//! *mechanism*, and it lives here once.
//!
//! *Policy* stays in the two callers, because that is where they genuinely
//! differ: which release, which flags, what counts as healthy, and what makes a
//! running server the wrong one. [`crate::media_sdcpp_process`] restarts on a
//! modality change; [`crate::llm_llama_process`] restarts on a model change;
//! neither shape belongs to the other.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use serde_json::Value;

use crate::AppState;

// ---------------------------------------------------------------------------
// Install: a pinned release, fetched and unpacked
// ---------------------------------------------------------------------------

/// A pinned GitHub release, and the one executable we want out of it.
pub(crate) struct Release {
    /// `owner/repo` on GitHub.
    pub repo: &'static str,
    /// The exact tag. **Never a floating `latest`** — both upstreams cut
    /// releases most days and neither carries semver, so tracking the tip means
    /// a model that worked yesterday failing today on a machine nobody is
    /// watching. Moving a pin is a deliberate act with a test run attached.
    pub tag: &'static str,
    /// What to call this thing in a sentence a user reads.
    pub label: &'static str,
    /// File name of the executable inside the archive, extension included.
    pub exe: &'static str,
    /// Substring of the wanted asset's file name.
    pub asset: String,
    /// Where it unpacks. Tagged by release by convention, so changing a pin
    /// fetches beside the old copy rather than half-overwriting it.
    pub dir: PathBuf,
}

/// What [`Release::install`] is doing, for the caller's stage machine.
pub(crate) enum Progress {
    Downloading { received: u64, total: u64 },
    Extracting,
}

impl Release {
    /// The unpacked executable, if this release is already on disk.
    pub fn installed(&self) -> Option<PathBuf> {
        find_binary(&self.dir, self.exe)
    }

    /// The unpacked executable, fetching the release first if it is missing.
    pub async fn install(
        &self,
        state: &AppState,
        progress: &(dyn Fn(Progress) + Sync),
    ) -> Result<PathBuf, String> {
        if let Some(found) = self.installed() {
            return Ok(found);
        }
        let (url, size, name) = self.asset(state).await?;
        let archive = self.dir.join(name);
        download(state, &url, &archive, size, self.label, progress).await?;

        progress(Progress::Extracting);
        extract(&archive, &self.dir)
            .await
            .map_err(|e| format!("The {} archive could not be unpacked{e}", self.label))?;
        let _ = tokio::fs::remove_file(&archive).await;

        self.installed().ok_or_else(|| {
            format!("The {} download unpacked, but no {} was found inside it.", self.tag, self.exe)
        })
    }

    /// `(download URL, size in bytes, file name)` for the pinned tag's asset.
    ///
    /// Asked of the GitHub API rather than assembled from the tag. Asset names
    /// embed things the tag does not — a short commit sha, a CUDA version —
    /// and deriving those by string surgery is the kind of guess that one day
    /// fetches a 404 or, worse, the wrong build.
    async fn asset(&self, state: &AppState) -> Result<(String, u64, String), String> {
        let url = format!("https://api.github.com/repos/{}/releases/tags/{}", self.repo, self.tag);
        let body: Value = state
            .http
            .get(&url)
            // GitHub rejects an API request with no User-Agent outright.
            .header("User-Agent", "agent-platformd")
            .header("Accept", "application/vnd.github+json")
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("The {} release list could not be fetched: {e}", self.label))?
            .json()
            .await
            .map_err(|e| format!("The {} release list was not JSON: {e}", self.label))?;

        let assets = body.get("assets").and_then(Value::as_array).ok_or_else(|| {
            format!("Release {} reported no assets. It may have been withdrawn.", self.tag)
        })?;
        assets
            .iter()
            .find(|a| a.get("name").and_then(Value::as_str).is_some_and(|n| n.contains(&self.asset)))
            .and_then(|a| {
                Some((
                    a.get("browser_download_url").and_then(Value::as_str)?.to_string(),
                    a.get("size").and_then(Value::as_u64).unwrap_or(0),
                    a.get("name").and_then(Value::as_str)?.to_string(),
                ))
            })
            .ok_or_else(|| {
                format!("Release {} has no asset matching {:?}.", self.tag, self.asset)
            })
    }

}

/// Fetch one file, streamed to a `.part` and renamed on success, so an
/// interrupted download is never mistaken for a complete one on the next run.
///
/// Not only for release archives: the media backend's model catalogue pulls
/// multi-gigabyte weights through here too, which is why `label` is a parameter
/// and the progress callback reports per chunk.
pub(crate) async fn download(
    state: &AppState,
    url: &str,
    dest: &Path,
    size: u64,
    label: &str,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt as _;

    let dir = dest.parent().ok_or("The download directory has no parent")?;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| format!("{} could not be created: {e}", dir.display()))?;

    let part = dest.with_extension("part");
    let response = state
        .http
        .get(url)
        .header("User-Agent", "agent-platformd")
        // No overall timeout: this is tens of megabytes to tens of gigabytes
        // over whatever connection the user has. A stalled *stream* still ends
        // the read.
        .send()
        .await
        .map_err(|e| format!("{label} could not be downloaded: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("The {label} download returned HTTP {}.", response.status()));
    }

    let mut file = tokio::fs::File::create(&part)
        .await
        .map_err(|e| format!("{} could not be opened: {e}", part.display()))?;
    let mut received: u64 = 0;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("The {label} download failed: {e}"))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("The download could not be written: {e}"))?;
        received += chunk.len() as u64;
        progress(Progress::Downloading { received, total: size });
    }
    file.flush().await.map_err(|e| format!("The download could not be flushed: {e}"))?;
    drop(file);

    tokio::fs::rename(&part, dest)
        .await
        .map_err(|e| format!("The download could not be renamed into place: {e}"))
}

/// The unpacker, chosen by archive extension and platform.
///
/// **Windows names `System32\tar.exe` by absolute path, never bare `tar`.**
/// Windows 10+ ships bsdtar there and bsdtar reads zip — but GNU tar, which
/// git-bash and MSYS put on `PATH`, does not. Measured on exactly these release
/// zips: GNU tar 1.35 answers *"This does not look like a tar archive"*, bsdtar
/// 3.8.4 unpacks them. Which one a bare `tar` resolves to is a property of the
/// user's `PATH`, and that is not a thing to gamble an install on.
///
/// Elsewhere a `.zip` goes to `unzip`, the tool that is actually about that
/// format, and a `.tar.gz` — what llama.cpp ships for macOS and Linux — goes to
/// `tar -xzf`, which is what GNU tar is for.
///
/// ponytail: shells out rather than adding the `zip` crate and its inflate
/// stack for one call per install. Swap for the crate if a platform without a
/// usable extractor ever matters.
fn unpack_command(archive: &Path, dir: &Path) -> tokio::process::Command {
    let zipped = archive
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    #[cfg(windows)]
    {
        let _ = zipped; // bsdtar reads both formats.
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        let mut command = tokio::process::Command::new(format!(r"{root}\System32\tar.exe"));
        command.arg("-xf").arg(archive).arg("-C").arg(dir);
        command
    }
    #[cfg(not(windows))]
    {
        if zipped {
            let mut command = tokio::process::Command::new("unzip");
            command.arg("-o").arg("-q").arg(archive).arg("-d").arg(dir);
            command
        } else {
            let mut command = tokio::process::Command::new("tar");
            command.arg("-xzf").arg(archive).arg("-C").arg(dir);
            command
        }
    }
}

/// `Ok`, or the unpacker's own complaint punctuated to sit after a caller's
/// sentence.
async fn extract(archive: &Path, dir: &Path) -> Result<(), String> {
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| format!(": {} could not be created: {e}", dir.display()))?;
    let output = unpack_command(archive, dir)
        .output()
        .await
        .map_err(|e| format!(": the unpacker could not be run: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    Err(if detail.is_empty() { ".".to_string() } else { format!(": {detail}") })
}

/// Both upstreams put the executable a directory or two down in their release
/// archives, so this walks rather than assuming a layout upstream may re-nest.
fn find_binary(dir: &Path, exe: &str) -> Option<PathBuf> {
    fn walk(dir: &Path, exe: &str, depth: u32) -> Option<PathBuf> {
        let direct = dir.join(exe);
        if direct.is_file() {
            return Some(direct);
        }
        if depth == 0 {
            return None;
        }
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            if entry.path().is_dir() {
                if let Some(found) = walk(&entry.path(), exe, depth - 1) {
                    return Some(found);
                }
            }
        }
        None
    }
    walk(dir, exe, 3)
}

// ---------------------------------------------------------------------------
// Spawn, wait, reap
// ---------------------------------------------------------------------------

/// How many stderr lines to keep for a startup failure message.
const TAIL_LINES: usize = 8;

/// The last few lines a managed server wrote, kept so a process that dies on
/// its way up can be quoted rather than paraphrased.
pub(crate) struct Tail(Mutex<Vec<String>>);

impl Tail {
    pub const fn new() -> Self {
        Self(Mutex::new(Vec::new()))
    }

    fn clear(&self) {
        if let Ok(mut lines) = self.0.lock() {
            lines.clear();
        }
    }

    fn push(&self, line: String) {
        if let Ok(mut lines) = self.0.lock() {
            if lines.len() == TAIL_LINES {
                lines.remove(0);
            }
            lines.push(line);
        }
    }

    fn lines(&self) -> Vec<String> {
        self.0.lock().map(|l| l.clone()).unwrap_or_default()
    }

    /// The diagnostic half of what the server wrote before dying.
    ///
    /// **`[ERROR]` lines win when there are any.** Measured on a bad model
    /// path: the tail is six lines of Vulkan device banner and backend loading,
    /// then the two that matter. Quoting all eight buries the reason in
    /// hardware trivia and puts a GPU model name in front of an error about a
    /// file, which reads like the wrong problem.
    pub fn summarise(&self) -> String {
        let lines = self.lines();
        let errors: Vec<&str> =
            lines.iter().map(String::as_str).filter(|l| l.contains("[ERROR]")).collect();
        let chosen =
            if errors.is_empty() { lines.iter().map(String::as_str).collect() } else { errors };
        chosen.join("; ").trim().to_string()
    }
}

/// Launch a managed server, with its stderr drained into the log ring.
///
/// The drain is not decoration: a child whose stderr pipe nobody reads blocks
/// once the pipe fills, so this is also what keeps a chatty server running. The
/// same lines land in `GET /system/logs` under `prefix`, which is how a model
/// that fails to load says why.
pub(crate) fn spawn(
    binary: &Path,
    args: &[String],
    prefix: &'static str,
    tail: &'static Tail,
) -> std::io::Result<tokio::process::Child> {
    let mut command = tokio::process::Command::new(binary);
    command
        .args(args)
        // Run from the install directory: the release archives put backend
        // shared libraries beside the executable, and a different working
        // directory is how those stop being found.
        .current_dir(binary.parent().unwrap_or_else(|| Path::new(".")))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        // Covers the ordinary path — a dropped handle kills the child. It does
        // *not* cover this process being terminated; the job object below does.
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW — no console flash

    let mut child = command.spawn()?;
    #[cfg(windows)]
    win_job::adopt(&child);

    tail.clear();
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt as _, BufReader};
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                logd!("[{prefix}] {line}");
                tail.push(line);
            }
        });
    }
    Ok(child)
}

/// Poll until it answers, it dies, or `timeout` passes.
///
/// **Watching the child is the whole point of taking it by reference.** A wrong
/// model path kills either of these servers in under a second — measured — so
/// polling an address nobody is listening on for five more minutes before
/// saying so would turn a typo into a coffee break. A dead child ends the wait
/// immediately and its own words become the error.
pub(crate) async fn health_wait<F, Fut>(
    child: &mut tokio::process::Child,
    timeout: Duration,
    label: &str,
    tail: &'static Tail,
    probe: F,
) -> Result<(), String>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let started = Instant::now();
    while started.elapsed() < timeout {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Ok(Some(status)) = child.try_wait() {
            let said = tail.summarise();
            return Err(if said.is_empty() {
                format!("{label} exited immediately ({status}) without saying why.")
            } else {
                format!("{label} exited immediately ({status}): {said}")
            });
        }
        if probe().await {
            return Ok(());
        }
    }
    Err(format!(
        "{label} was started but did not answer within {} seconds. Check GET /system/logs \
         for what it reported.",
        timeout.as_secs()
    ))
}

// ---------------------------------------------------------------------------
// Odds and ends both callers need
// ---------------------------------------------------------------------------

/// Only a loopback base is ours to manage. Returns the port to launch on.
///
/// A remote base belongs to someone else: probe it, never spawn, download or
/// kill it.
pub(crate) fn loopback_port(base: &str) -> Option<u16> {
    let url = url::Url::parse(base).ok()?;
    let host = url.host_str()?;
    if !matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]") {
        return None;
    }
    url.port_or_known_default()
}

/// Split on whitespace, honouring double quotes so a path with a space
/// survives — the common case on Windows, and a silent mis-split here would
/// surface as a server rejecting a truncated filename.
pub(crate) fn split_args(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in raw.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut current));
                    any = false;
                }
            }
            c => {
                current.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(current);
    }
    out
}

pub(crate) fn human(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Kill-on-job-close, so a terminated `agent-platformd` does not leave a
/// GPU-resident child behind. Mirrors `desktop/src/shell.rs`; the handle is
/// deliberately never closed, because the OS closing it at exit *is* the
/// mechanism. One job object for every managed server in this process — they
/// all die together, and for the same reason.
#[cfg(windows)]
mod win_job {
    use std::sync::OnceLock;

    use tokio::process::Child;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    static JOB: OnceLock<usize> = OnceLock::new();

    pub fn adopt(child: &Child) -> bool {
        let Some(raw) = child.raw_handle() else { return false };
        let handle = *JOB.get_or_init(|| unsafe { create() } as usize);
        handle != 0 && unsafe { AssignProcessToJobObject(handle as HANDLE, raw as HANDLE) != 0 }
    }

    unsafe fn create() -> HANDLE {
        let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if handle.is_null() {
            return std::ptr::null_mut();
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            handle,
            JobObjectExtendedLimitInformation,
            std::ptr::addr_of!(info).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) != 0;
        if ok {
            handle
        } else {
            std::ptr::null_mut()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only loopback is ours to spawn on, and the port has to come back or the
    /// launch has nowhere to bind.
    #[test]
    fn only_a_loopback_base_is_managed() {
        assert_eq!(loopback_port("http://127.0.0.1:8188"), Some(8188));
        assert_eq!(loopback_port("http://localhost:18412"), Some(18412));
        assert_eq!(loopback_port("http://192.168.1.9:8188"), None);
        assert_eq!(loopback_port("https://images.example.com"), None);
        assert_eq!(loopback_port("not a url"), None);
    }

    /// The bug this guards: a Windows model path has spaces in it more often
    /// than not, and a naive `split_whitespace` hands a server a truncated
    /// filename that it reports as a missing model.
    #[test]
    fn quoted_arguments_survive_splitting() {
        assert_eq!(
            split_args("-m \"C:\\models\\a b.gguf\" -c 8192"),
            vec!["-m", "C:\\models\\a b.gguf", "-c", "8192"]
        );
        assert!(split_args("   ").is_empty());
        assert_eq!(split_args("-m  a.gguf"), vec!["-m", "a.gguf"]);
        // An empty quoted argument is still an argument — dropping it would
        // shift every flag after it onto the wrong value.
        assert_eq!(
            split_args("--negative \"\" --steps 8"),
            vec!["--negative", "", "--steps", "8"]
        );
    }

    /// Measured against a real failed launch: the banner is six lines of GPU
    /// and backend trivia, and the reason is the two `[ERROR]` lines at the
    /// end. Quoting the banner puts a graphics card in front of a file error.
    #[test]
    fn a_failure_summary_prefers_error_lines() {
        static TAIL: Tail = Tail::new();
        for line in [
            "ggml_vulkan: Found 1 Vulkan devices:",
            "ggml_vulkan: 0 = NVIDIA GeForce RTX 5080 (NVIDIA) | uma: 0",
            "load_backend: loaded Vulkan backend from ggml-vulkan.dll",
            "[ERROR] stable-diffusion.cpp:905  - get sd version from file failed: ''",
            "[ERROR] main.cpp:92   - new_sd_ctx_t failed",
        ] {
            TAIL.push(line.into());
        }
        let summary = TAIL.summarise();
        assert!(summary.contains("get sd version from file failed"));
        assert!(summary.contains("new_sd_ctx_t failed"));
        assert!(!summary.contains("RTX 5080"), "the banner must not survive: {summary}");

        // A crash with no `[ERROR]` line at all still has to say something.
        TAIL.clear();
        TAIL.push("something went wrong".into());
        assert_eq!(TAIL.summarise(), "something went wrong");
        TAIL.clear();
        assert_eq!(TAIL.summarise(), "");
    }

    /// The tail is bounded, or a server that logs all night is a leak.
    #[test]
    fn the_tail_keeps_only_the_last_lines() {
        static TAIL: Tail = Tail::new();
        for i in 0..40 {
            TAIL.push(format!("line {i}"));
        }
        let lines = TAIL.lines();
        assert_eq!(lines.len(), TAIL_LINES);
        assert_eq!(lines.last().unwrap(), "line 39");
    }

    #[test]
    fn sizes_read_as_sizes() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(36_700_160), "35.0 MB");
    }
}
