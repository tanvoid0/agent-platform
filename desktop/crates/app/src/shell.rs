//! Server sidecar management, ported from the Tauri shell (`desktop/src-tauri/src/lib.rs`).
//!
//! The server is the unmodified Python app; everything desktop-specific (loopback
//! bind, per-install key, per-user data dirs, fixed port) is passed as environment.
//! Differences from the Tauri version: the port is fixed (Ollama-style background
//! server — external clients need a stable address) and there is no CORS env at
//! all (a native client has no origin).

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const DEFAULT_PORT: u16 = 18410;

/// Same directory the Tauri shell used (`app_config_dir` == `app_data_dir` on
/// Windows for this identifier), so existing installs keep their key and DB.
pub const APP_DIR: &str = "com.tanvoid0.agentplatform";

/// Lines of server output kept for the Logs view.
const LOG_CAPACITY: usize = 4000;

#[cfg(windows)]
const RUNTIME_PYTHON: &str = "python.exe";
#[cfg(not(windows))]
const RUNTIME_PYTHON: &str = "bin/python3";

/// Appearance preference. `System` follows the OS setting, re-read on each poll
/// so a mid-session OS switch is picked up without a restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    /// Glyph for the compact sidebar toggle.
    pub fn icon(&self) -> crate::ui::Icon {
        match self {
            ThemeMode::System => crate::ui::Icon::Monitor,
            ThemeMode::Light => crate::ui::Icon::Sun,
            ThemeMode::Dark => crate::ui::Icon::Moon,
        }
    }

    /// Next mode in the System → Light → Dark cycle.
    pub fn next(&self) -> ThemeMode {
        match self {
            ThemeMode::System => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::System,
        }
    }

    /// Resolve to a concrete iced theme, consulting the OS for `System`.
    /// Unknown/unsupported OS reports fall back to dark.
    pub fn resolve(&self) -> iced::Theme {
        let dark = match self {
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
            ThemeMode::System => system_is_dark(),
        };
        if dark {
            crate::ui::theme::dark_theme()
        } else {
            crate::ui::theme::light_theme()
        }
    }
}

/// OS dark-mode probe with a short TTL cache. `resolve()` runs on every render
/// (60/s while the HUD animates) and `dark_light::detect()` hits the registry,
/// so the raw call is cached; 2s still picks up an OS theme switch promptly.
fn system_is_dark() -> bool {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    if let Some((at, dark)) = *cache {
        if at.elapsed() < Duration::from_secs(2) {
            return dark;
        }
    }
    let dark = !matches!(dark_light::detect(), Ok(dark_light::Mode::Light));
    *cache = Some((Instant::now(), dark));
    dark
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub port: u16,
    pub start_minimized: bool,
    pub theme: ThemeMode,
    /// Chat screen's provider/model override, kept across restarts.
    /// Empty = the server's default.
    pub chat_provider: String,
    pub chat_model: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            start_minimized: false,
            theme: ThemeMode::default(),
            chat_provider: String::new(),
            chat_model: String::new(),
        }
    }
}

impl Settings {
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join("settings.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join("settings.json"), serde_json::to_string_pretty(self).unwrap())
    }
}

pub struct LogRing {
    lines: VecDeque<(u64, String)>,
    next_seq: u64,
}

pub struct LogChunk {
    pub lines: Vec<String>,
    pub next: u64,
    pub dropped: u64,
}

impl LogRing {
    pub fn new() -> Self {
        Self { lines: VecDeque::new(), next_seq: 0 }
    }

    pub fn push(&mut self, line: String) {
        self.lines.push_back((self.next_seq, line));
        self.next_seq += 1;
        while self.lines.len() > LOG_CAPACITY {
            self.lines.pop_front();
        }
    }

    /// Lines at or after `after`; `dropped` is non-zero once the ring wrapped
    /// past what the caller last saw.
    pub fn since(&self, after: u64) -> LogChunk {
        let lines = self
            .lines
            .iter()
            .filter(|(seq, _)| *seq >= after)
            .map(|(_, line)| line.clone())
            .collect();
        let oldest = self.lines.front().map(|(seq, _)| *seq).unwrap_or(after);
        LogChunk { lines, next: self.next_seq, dropped: oldest.saturating_sub(after) }
    }
}

/// Owns the child process and its captured output. Port and key are fixed for
/// the run so external API clients keep a stable address across restarts.
pub struct Shell {
    pub server: Option<Child>,
    pub log: Arc<Mutex<LogRing>>,
    pub python: PathBuf,
    pub script: PathBuf,
    pub port: u16,
    pub key: String,
    pub data_dir: PathBuf,
    /// True when this app found a healthy server already on the port and became
    /// a pure client. An attached server is never killed or restarted by us.
    pub attached: bool,
}

impl Shell {
    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn log_line(&self, line: impl Into<String>) {
        self.log.lock().unwrap().push(line.into());
    }

    /// Starts the server and wires its output into the ring. Replaces any
    /// process we already own. No-op when attached to an external server.
    pub fn start_server(&mut self) {
        if self.attached {
            self.log_line("[shell] attached to an existing server; not spawning");
            return;
        }
        self.stop_server();
        match self.spawn() {
            Ok(mut child) => {
                if let Some(out) = child.stdout.take() {
                    drain_into_log(self.log.clone(), out);
                }
                if let Some(err) = child.stderr.take() {
                    drain_into_log(self.log.clone(), err);
                }
                self.server = Some(child);
            }
            Err(e) => self.log_line(format!("[shell] could not start the server: {e}")),
        }
    }

    pub fn stop_server(&mut self) {
        if let Some(mut child) = self.server.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    pub fn server_running(&mut self) -> bool {
        if self.attached {
            return true; // health poll is the real signal; we own no process
        }
        match self.server.as_mut() {
            None => false,
            Some(child) => match child.try_wait() {
                Ok(Some(_)) => {
                    self.server = None;
                    false
                }
                _ => true,
            },
        }
    }

    fn spawn(&self) -> std::io::Result<Child> {
        std::fs::create_dir_all(&self.data_dir)?;
        let port = self.port;
        let mut cmd = Command::new(&self.python);
        cmd.arg(&self.script)
            .arg("--skip-build")
            .arg("--no-browser")
            .arg("--exit-with-parent")
            .env("AGENT_PLATFORM_HOST", "127.0.0.1")
            .env("AGENT_PLATFORM_PORT", port.to_string())
            .env("AGENT_PLATFORM_MASTER_KEY", &self.key)
            // Chat, agents, coder and assistant reach the embedded LLM proxy over
            // HTTP; the default base assumes :18410 which may not be our port.
            .env("LLM_ORCHESTRATOR_BASE_URL", format!("http://127.0.0.1:{port}/v1"))
            .env("AGENT_PLATFORM_ENV", "development")
            .env("AGENT_PLATFORM_DB_PATH", self.data_dir.join("agent_platform.db"))
            .env("AGENT_PLATFORM_WORKSPACE_ROOT", self.data_dir.join("workspaces"))
            .env("MODEL_OPS_DATA_DIR", self.data_dir.join("model-ops"))
            // BYOK/provider config must land in a user-writable dir, not the install dir.
            .env("CONFIG_DIR", self.data_dir.join("llm"))
            // A developer's .env must not point a desktop install at someone's
            // Postgres. Present-but-empty wins, because load_dotenv does not override.
            .env("DATABASE_URL", "")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW — no console flash
        }
        cmd.spawn()
    }
}

/// Config/data root: `%APPDATA%/com.tanvoid0.agentplatform` (same as the Tauri shell).
pub fn app_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join(APP_DIR)
}

/// Per-install API key, generated once. Loopback is not a security boundary, so
/// the server always runs with auth on; the app hands the key to its own client
/// and shows it (copyable) for external API consumers.
pub fn load_or_create_key(dir: &Path) -> std::io::Result<String> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("master.key");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }
    let mut raw = [0u8; 32];
    getrandom::getrandom(&mut raw)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    let key: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(&path, &key)?;
    Ok(key)
}

/// Locate the Python runtime and entrypoint: repo checkout first in debug builds
/// (so editing the server takes effect), bundled payload next to the exe otherwise.
pub fn resolve_server() -> Option<(PathBuf, PathBuf)> {
    if cfg!(debug_assertions) {
        if let Some(found) = repo_server() {
            return Some(found);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(root) = exe.parent().map(|p| p.join("server")) {
            let python = root.join("runtime").join(RUNTIME_PYTHON);
            let script = root.join("scripts").join("start.py");
            if python.is_file() && script.is_file() {
                return Some((python, script));
            }
        }
    }
    repo_server()
}

fn repo_server() -> Option<(PathBuf, PathBuf)> {
    // crates/app -> crates -> desktop -> repo root
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?.parent()?;
    let script = repo.join("scripts").join("start.py");
    if !script.is_file() {
        return None;
    }
    let python = if cfg!(windows) { "python" } else { "python3" };
    Some((PathBuf::from(python), script))
}

/// Drains a child pipe into the ring on its own thread. Not optional: a child
/// whose output nobody reads blocks once the OS pipe buffer fills.
fn drain_into_log<R: Read + Send + 'static>(log: Arc<Mutex<LogRing>>, stream: R) {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            log.lock().unwrap().push(line);
        }
    });
}

/// What is answering on our port, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortOwner {
    /// Nothing listening (or not an HTTP server we recognize) — safe to spawn.
    Free,
    /// An agent-platform server that accepts our install key: attach to it.
    Ours,
    /// Something else is on the port — another install, a Docker port-forward,
    /// an unrelated service. Spawning would fail to bind; attaching would talk
    /// to a stranger's data with a key it rejects.
    Foreign,
}

/// One HTTP/1.0 request over a raw socket, no HTTP dependency. Returns the
/// status line's 3-digit code.
fn probe(port: u16, path: &str, bearer: Option<&str>) -> Option<u16> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(500)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(3000)));
    let auth = bearer
        .map(|k| format!("Authorization: Bearer {k}\r\n"))
        .unwrap_or_default();
    let req =
        format!("GET {path} HTTP/1.0\r\nHost: 127.0.0.1\r\n{auth}Connection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).ok()?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).ok()?;
    let head = String::from_utf8_lossy(&response[..response.len().min(64)]).to_string();
    head.split_whitespace().nth(1)?.parse().ok()
}

/// `/health` is unauthenticated, and uvicorn accepts connections before the app
/// finishes starting — so a real request is the only honest readiness signal.
pub fn health_ok(port: u16) -> bool {
    probe(port, "/health", None) == Some(200)
}

/// Decide whether the server already on the port is one we may adopt. A healthy
/// server that rejects our key is NOT ours: attaching would point the UI at
/// another install (or a Docker forward) that our key cannot authenticate to.
pub fn port_owner(port: u16, key: &str) -> PortOwner {
    if !health_ok(port) {
        return PortOwner::Free;
    }
    match probe(port, "/api/v1/system/status", Some(key)) {
        Some(200) => PortOwner::Ours,
        _ => PortOwner::Foreign,
    }
}

/// Launch a fresh copy of this executable. The caller must have stopped the
/// server child first — the new process probes the port on startup and would
/// otherwise attach to a sidecar that is about to die with us.
pub fn spawn_replacement() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    Command::new(exe).spawn().map(|_| ())
}

/// Path of the pid file recording the instance that currently owns the app.
pub fn pid_file(dir: &Path) -> PathBuf {
    dir.join("app.pid")
}

/// One app at a time. Kills whatever earlier instance recorded itself in
/// `app.pid` and waits for it (and the sidecar that dies with it) to let go of
/// the port before returning. Debug and release builds share the directory, so
/// a dev run replaces an installed one and vice versa. Returns a line to log
/// when an instance was actually replaced.
pub fn claim_single_instance(dir: &Path, port: u16) -> Option<String> {
    let path = pid_file(dir);
    let mut note = None;
    if let Some(pid) =
        std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u32>().ok())
    {
        if pid != std::process::id() && is_our_process(pid) {
            kill_process(pid);
            // Its sidecar exits with it, but not instantly — spawning or probing
            // before the port is free would bind-fail or attach to a corpse.
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while std::time::Instant::now() < deadline
                && (is_our_process(pid) || health_ok(port))
            {
                std::thread::sleep(Duration::from_millis(200));
            }
            note = Some(format!("[shell] replaced the running instance (pid {pid})"));
        }
    }
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(&path, std::process::id().to_string());
    note
}

fn exe_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "agent-platform".to_string())
}

/// Whether `pid` is a live process running this same executable. The name check
/// matters: pids are recycled, and the file may be days stale.
#[cfg(windows)]
fn is_our_process(pid: u32) -> bool {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new("tasklist");
    cmd.args([
        "/FI",
        &format!("PID eq {pid}"),
        "/FI",
        &format!("IMAGENAME eq {}", exe_name()),
        "/NH",
        "/FO",
        "CSV",
    ])
    .creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    match cmd.output() {
        Ok(out) => String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\"")),
        Err(_) => false,
    }
}

#[cfg(not(windows))]
fn is_our_process(pid: u32) -> bool {
    match Command::new("ps").args(["-p", &pid.to_string(), "-o", "comm="]).output() {
        Ok(out) => {
            let comm = String::from_utf8_lossy(&out.stdout);
            let comm = comm.trim();
            !comm.is_empty() && Path::new(comm).file_name().map(|n| n == exe_name().as_str()) == Some(true)
        }
        Err(_) => false,
    }
}

#[cfg(windows)]
fn kill_process(pid: u32) {
    use std::os::windows::process::CommandExt;
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(0x0800_0000)
        .output(); // captured, so taskkill's chatter stays out of a dev console
}

#[cfg(not(windows))]
fn kill_process(pid: u32) {
    let _ = Command::new("kill").arg(pid.to_string()).output();
}

/// Reveal a path in the platform file manager.
pub fn reveal_path(path: &str) {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = Command::new(program).arg(path).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_ring_cursor_and_wrap() {
        let mut ring = LogRing::new();
        for i in 0..10 {
            ring.push(format!("line{i}"));
        }
        let chunk = ring.since(0);
        assert_eq!(chunk.lines.len(), 10);
        assert_eq!(chunk.next, 10);
        assert_eq!(chunk.dropped, 0);

        let chunk = ring.since(7);
        assert_eq!(chunk.lines, vec!["line7", "line8", "line9"]);

        // Fill past capacity: oldest entries fall off, dropped reports the gap.
        for i in 10..(LOG_CAPACITY as u64 + 20) {
            ring.push(format!("line{i}"));
        }
        let chunk = ring.since(0);
        assert_eq!(chunk.lines.len(), LOG_CAPACITY);
        assert!(chunk.dropped > 0);
    }

    /// A stale pid must not be mistaken for a live instance (nothing to kill, no
    /// wait), and the claim must always end with our own pid in the file.
    #[test]
    fn single_instance_claim_ignores_a_stale_pid() {
        let dir = std::env::temp_dir().join(format!("ap-pid-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(pid_file(&dir), "4294967290").unwrap();

        // Port 0 is never listening, so a hang here would mean we decided the
        // dead pid was alive.
        assert_eq!(claim_single_instance(&dir, 0), None);
        assert_eq!(
            std::fs::read_to_string(pid_file(&dir)).unwrap(),
            std::process::id().to_string()
        );

        // Second claim sees its own pid and leaves this process alone.
        assert_eq!(claim_single_instance(&dir, 0), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn settings_roundtrip_and_defaults() {
        let dir = std::env::temp_dir().join(format!("ap-settings-test-{}", std::process::id()));
        let s = Settings {
            port: 12345,
            start_minimized: true,
            theme: ThemeMode::Light,
            ..Settings::default()
        };
        s.save(&dir).unwrap();
        let loaded = Settings::load(&dir);
        assert_eq!(loaded.port, 12345);
        assert!(loaded.start_minimized);
        assert_eq!(loaded.theme, ThemeMode::Light);
        std::fs::remove_dir_all(&dir).ok();

        let missing = Settings::load(Path::new("Z:/definitely/not/here"));
        assert_eq!(missing.port, DEFAULT_PORT);
    }
}
