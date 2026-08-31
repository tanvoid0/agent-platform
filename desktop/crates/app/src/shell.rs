//! Server sidecar management, ported from the Tauri shell (`desktop/src-tauri/src/lib.rs`).
//!
//! The child is `agent-platformd` (ADR 0007), which binds our port and *is* the
//! server — it spawned a Python child of its own until the migration finished.
//! Everything desktop-specific (loopback bind, per-install key, per-user data
//! dirs, fixed port) is passed as environment.
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
const DAEMON_EXE: &str = "agent-platformd.exe";
#[cfg(not(windows))]
const DAEMON_EXE: &str = "agent-platformd";

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
pub fn system_is_dark() -> bool {
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

/// Which animation the E.V. canvas draws. Both are fed by the same live audio
/// analysis — this only picks how it is dressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum HudStyle {
    /// A soft orb, drawn on the GPU. Calm enough to sit on the Dashboard all
    /// day, and the only one of the three with a smooth halo.
    #[default]
    Bubble,
    /// The same idea on iced's canvas, for machines where the GPU backend is
    /// unavailable and [`HudStyle::Bubble`] renders nothing.
    BubbleCanvas,
    /// The suit heads-up display: spectrum web, reticle, telemetry.
    Suit,
}

impl HudStyle {
    pub const ALL: [HudStyle; 3] =
        [HudStyle::Bubble, HudStyle::BubbleCanvas, HudStyle::Suit];

    pub fn label(self) -> &'static str {
        match self {
            HudStyle::Bubble => "Bubble",
            HudStyle::BubbleCanvas => "Bubble (canvas)",
            HudStyle::Suit => "Suit HUD",
        }
    }
}

/// How much of the machine the server may spend on model calls (ADR 0010).
///
/// The app only stores and displays this; the meaning lives in the server's
/// `resources::Mode`, and the wire form is the lowercase name. `Auto` is the
/// default because a knob most users never touch only helps the ones who already
/// knew they had a problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ResourceMode {
    Eco,
    Balanced,
    Turbo,
    #[default]
    Auto,
}

impl ResourceMode {
    pub const ALL: [ResourceMode; 4] =
        [ResourceMode::Auto, ResourceMode::Eco, ResourceMode::Balanced, ResourceMode::Turbo];

    /// The wire value. Must match the server's `resources::Mode::as_str`.
    pub fn as_str(self) -> &'static str {
        match self {
            ResourceMode::Eco => "eco",
            ResourceMode::Balanced => "balanced",
            ResourceMode::Turbo => "turbo",
            ResourceMode::Auto => "auto",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ResourceMode::Eco => "Eco",
            ResourceMode::Balanced => "Balanced",
            ResourceMode::Turbo => "Turbo",
            ResourceMode::Auto => "Auto",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub port: u16,
    pub start_minimized: bool,
    pub theme: ThemeMode,
    /// Which E.V. animation the canvas draws.
    pub hud_style: HudStyle,
    /// How fast E.V. reads a reply aloud, as percent of the voice's normal
    /// pace. See `assistant::VOICE_RATES`.
    #[serde(default = "default_voice_rate")]
    pub voice_rate: i32,
    /// What the assistant is called, everywhere the user sees it. Empty = the
    /// default ("E.V."). Display only: the `source` filed on its chats and
    /// memories stays `assistant::NAME`.
    #[serde(default)]
    pub assistant_name: String,
    /// How the wake word may be spelled, comma-separated, because whisper
    /// writes a spoken name a dozen ways ("eva", "ava", "evie"). Empty = the
    /// name itself, or the built-in list while the name is the default one.
    #[serde(default)]
    pub wake_names: String,
    /// TTS voice id — `en-US-AriaNeural` for Edge, or whatever a trained model
    /// is called on the `SPEECH_API_BASE` backend. Empty = each engine's own
    /// default.
    #[serde(default)]
    pub voice_name: String,
    /// Keep the mic open across the whole app, waiting to hear its name. Off by
    /// default and never turned on by anything but the user: it is the one
    /// setting that opens a microphone they did not just click a button for.
    #[serde(default)]
    pub wake_word: bool,
    /// Show a confirm card before E.V. runs a shell command. On by default, and
    /// `default = true` rather than `#[serde(default)]` on purpose: a settings
    /// file written before this existed must come back with the terminal
    /// guarded, not silently open.
    #[serde(default = "default_true")]
    pub confirm_commands: bool,
    /// The app-wide provider/model default, kept across restarts. Empty = the
    /// server's default. Set from the Chat header, and what every screen
    /// without a picker of its own opens a new conversation on — the embedded
    /// chats on Processes, and the Coder screen when its own pair is unset.
    /// A conversation already under way keeps the pair it started on: E.V.'s in
    /// `chats.json`, Coder's on the server's thread row, the embedded chats' in
    /// memory for as long as the thread lives.
    pub chat_provider: String,
    pub chat_model: String,
    /// GGUF to answer chat with in-process (ADR 0006). Empty — the default —
    /// sends every turn to the server, as before. Only read when the binary was
    /// built with the `local-llm` feature.
    #[serde(default)]
    pub local_model_path: String,
    /// KV-cache context for that model. The knob the Ollama path does not
    /// expose: too large and the cache spills to CPU, too small and a long
    /// thread stops fitting.
    #[serde(default = "default_local_n_ctx")]
    pub local_n_ctx: u32,
    /// Port for the OpenAI-compatible endpoint in front of that model, for the
    /// Python server's own agents. 0 — the default — leaves it off; nothing
    /// listens and the model only answers this app's chat.
    #[serde(default)]
    pub local_server_port: u16,
    /// Folder the Coder screen opens on. Persisted because it is the one thing
    /// that screen cannot work without, and re-picking it every launch is the
    /// difference between a tool and a demo.
    #[serde(default)]
    pub coder_workspace: String,
    /// Coder screen's provider/model override, kept across restarts. Empty
    /// means the server's default — which is `llama3` and cannot hold a tool
    /// loop, so this is the setting that decides whether the screen works.
    #[serde(default)]
    pub coder_provider: String,
    #[serde(default)]
    pub coder_model: String,
    /// Ask the Coder agent for a plan before each turn's tool loop. On by
    /// default: it costs one extra call and is the largest quality difference
    /// available on the local models this screen mostly runs.
    ///
    /// Superseded by [`Self::coder_plan_mode`], and still written so a file
    /// shared with an older build keeps its off/on setting. Read only when the
    /// mode is absent.
    #[serde(default = "default_true")]
    pub coder_plan: bool,
    /// The three-state form of the above (`off` / `inline` / `gate`). Absent in
    /// a settings file written before the gate existed, which is what
    /// `coder_plan` is still there to answer.
    #[serde(default)]
    pub coder_plan_mode: Option<crate::coder::PlanMode>,
    /// Open each file the agent writes as it writes it. Off by default: it moves
    /// the dock under the user mid-turn, which is what they asked for when they
    /// turned it on and a surprise when they did not.
    #[serde(default)]
    pub coder_follow: bool,
    /// How much of `run_command` the Coder agent may do unasked — see
    /// [`crate::coder::Autonomy`]. `Off` by default, which is the tier that
    /// refuses commands outright.
    #[serde(default)]
    pub coder_autonomy: crate::coder::Autonomy,
    /// Command prefixes approved for good, keyed by workspace root. Written by
    /// the approval card's **Always allow** and only read in
    /// [`crate::coder::Autonomy::Allowlist`].
    ///
    /// Per workspace on purpose: `cargo test` in a repo you own is not the same
    /// permission as `cargo test` in one you cloned this morning.
    #[serde(default)]
    pub coder_allowlist: std::collections::BTreeMap<String, Vec<String>>,
    /// How hard the server is allowed to work (ADR 0010). Pushed to the server
    /// on change and on every reconnect — it is not an env var, because a
    /// setting that needs a restart to take effect is one users toggle once and
    /// never trust again.
    #[serde(default)]
    pub resource_mode: ResourceMode,
    /// Last cloud origin typed in Settings → Account. Not the session — that
    /// lives in `cloud.session.json` so a refresh token never reaches this file.
    #[serde(default)]
    pub cloud_url: String,
}

fn default_true() -> bool {
    true
}

fn default_local_n_ctx() -> u32 {
    8192
}

fn default_voice_rate() -> i32 {
    crate::assistant_voice::DEFAULT_VOICE_RATE
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            start_minimized: false,
            theme: ThemeMode::default(),
            hud_style: HudStyle::default(),
            voice_rate: default_voice_rate(),
            assistant_name: String::new(),
            wake_names: String::new(),
            voice_name: String::new(),
            wake_word: false,
            confirm_commands: true,
            chat_provider: String::new(),
            chat_model: String::new(),
            local_model_path: String::new(),
            local_n_ctx: default_local_n_ctx(),
            local_server_port: 0,
            coder_workspace: String::new(),
            coder_provider: String::new(),
            coder_model: String::new(),
            coder_plan: true,
            coder_plan_mode: None,
            coder_follow: false,
            coder_autonomy: crate::coder::Autonomy::default(),
            coder_allowlist: std::collections::BTreeMap::new(),
            resource_mode: ResourceMode::default(),
            cloud_url: String::new(),
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
        write_atomic(&dir.join("settings.json"), &serde_json::to_string_pretty(self).unwrap())
    }
}

/// Replace a file's contents in one step, or not at all.
///
/// **`std::fs::write` truncates first.** Everything this app keeps on disk is a
/// whole file rewritten on every change — `settings.json`, `chats.json`,
/// `memories.json`, `master.key` — so a process that dies between the truncate
/// and the write leaves an empty or half-written file, and every one of those
/// loaders falls back to a default when parsing fails. The user's settings,
/// their chat history or everything the assistant remembered are gone, silently.
///
/// This is not a remote possibility here: quitting is `std::process::exit(0)`
/// (`iced::exit()` hangs on Windows — see `desktop/CLAUDE.md`), which does not
/// wait for anything else holding a file open.
///
/// Write beside the target, flush it to the disk, then rename over. `rename`
/// replaces an existing file on both platforms and is atomic within a volume,
/// which a temp directory would not be — hence the sibling.
pub fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;

    let tmp = path.with_extension("tmp");
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(contents.as_bytes())?;
        // Without this the rename can land before the bytes do, and a power cut
        // leaves a correctly-named file full of zeroes.
        file.sync_all()?;
    }
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Do not leave the temp file behind to be mistaken for a backup.
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
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
    /// `agent-platformd`. It is the whole server — there is no second process
    /// behind it any more.
    pub daemon: PathBuf,
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
                #[cfg(windows)]
                if !job::adopt(&child) {
                    self.log_line(
                        "[shell] warning: the server was not adopted into our job object; \
                         a crash of this app will leave it running"
                            .to_string(),
                    );
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
                // The status used to be dropped here, so a server that refused
                // to start — a bad `DATABASE_URL`, an edited migration, a taken
                // port — left the app showing "not running" and nothing else.
                // The daemon's own reason is already in this ring, drained off
                // its stderr; this line is what says the exit was real and
                // final rather than a slow boot.
                Ok(Some(status)) => {
                    self.log_line(format!("[shell] the server exited ({}), see the lines above for why", describe(status)));
                    self.server = None;
                    false
                }
                Err(e) => {
                    // Cannot reap it: treat as gone rather than claiming a
                    // health we cannot observe.
                    self.log_line(format!("[shell] lost track of the server process ({e})"));
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
        let mut cmd = Command::new(&self.daemon);
        // No args: everything the daemon needs comes through the environment.
        cmd.env("AGENT_PLATFORM_HOST", "127.0.0.1")
            .env("AGENT_PLATFORM_PORT", port.to_string())
            // The per-install key, always set so `load_dotenv` (which only fills
            // gaps) cannot let a developer `.env` decide what the local API
            // trusts. ADR 0013 spawned this open, like Ollama; ADR 0019 keys it,
            // because this server is not an inference endpoint — it runs
            // commands, reads and writes the workspace, and holds the user's
            // BYOK provider keys and cloud session. Empty only when the key file
            // could not be read, which degrades to the old open server rather
            // than to an app that cannot reach its own data.
            .env("AGENT_PLATFORM_MASTER_KEY", &self.key)
            .env("AGENT_PLATFORM_ENV", "development")
            .env("AGENT_PLATFORM_DB_PATH", self.data_dir.join("agent_platform.db"))
            .env("AGENT_PLATFORM_WORKSPACE_ROOT", self.data_dir.join("workspaces"))
            .env("MODEL_OPS_DATA_DIR", self.data_dir.join("model-ops"))
            // Generated images and video, and any workflow template the user
            // overrides (ADR 0009). Same reason as the line above: without it
            // the daemon writes them beside its own install.
            .env("MEDIA_DATA_DIR", self.data_dir.join("media"))
            // BYOK/provider config must land in a user-writable dir, not the install dir.
            .env("CONFIG_DIR", self.data_dir.join("llm"))
            // Hosted-account session the daemon reads as provider `platform`.
            .env(
                "AGENT_PLATFORM_CLOUD_SESSION",
                crate::account::session_path(&self.data_dir),
            )
            // A developer's .env must not point a desktop install at someone's
            // Postgres. Present-but-empty wins, because load_dotenv does not override.
            .env("DATABASE_URL", "");
        // The GGUF the daemon's `local` provider runs, through the
        // `llama-server` it manages itself (ADR 0012). Empty path is omitted so
        // a headless LOCAL_MODEL_PATH in the parent environment still reaches
        // the child.
        let settings = Settings::load(&self.data_dir);
        if !settings.local_model_path.trim().is_empty() {
            cmd.env("LOCAL_MODEL_PATH", settings.local_model_path.trim());
            cmd.env("LOCAL_N_CTX", settings.local_n_ctx.to_string());
        }
        cmd.stdin(Stdio::piped())
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
///
/// `AGENT_PLATFORM_APP_DIR` moves the whole of it — database, `settings.json`,
/// `master.key`, chats, memories. There is no other way to launch this app
/// without it opening the real user's data: `AGENT_PLATFORM_PORT` moves the
/// port only, and setting `%APPDATA%` does nothing because `dirs::config_dir`
/// asks Win32 for the known folder rather than reading the variable.
///
/// Which is why this exists: driving the app to check a change meant driving it
/// over live data, and the run that found this was a run that could not start
/// at all because that live database was corrupt.
pub fn app_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("AGENT_PLATFORM_APP_DIR") {
        return PathBuf::from(dir);
    }
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
    // Atomic too: a half-written key reads as a valid-looking wrong one, and
    // the app then cannot authenticate against the server holding its data.
    write_atomic(&path, &key)?;
    Ok(key)
}

/// The server binary, which always sits beside ours — `target/<profile>` in a
/// dev build, the install dir otherwise.
pub fn resolve_server() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let daemon = exe.parent()?.join(DAEMON_EXE);
    daemon.is_file().then_some(daemon)
}

/// Ties children to this process at the OS level, so a crash cannot leave a
/// server holding the port and the database. The daemon used to do the same to
/// its Python child; this is the one copy left.
#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// Never closed on purpose: the OS closing it when we exit is the whole
    /// mechanism. Stored as `usize` because a raw HANDLE is not `Send`/`Sync`.
    static JOB: OnceLock<usize> = OnceLock::new();

    pub fn adopt(child: &Child) -> bool {
        let handle = *JOB.get_or_init(|| unsafe { create() } as usize);
        handle != 0
            && unsafe {
                AssignProcessToJobObject(handle as HANDLE, child.as_raw_handle() as HANDLE) != 0
            }
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

/// Drains a child pipe into the ring on its own thread. Not optional: a child
/// whose output nobody reads blocks once the OS pipe buffer fills.

/// `exit code 2` / `signal 9`, or a bare status when the platform has neither.
/// `ExitStatus`'s own Display is "exit code: 2" on Windows and "signal: 9
/// (SIGKILL)" on unix; this keeps one shape for the log.
fn describe(status: std::process::ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("exit code {code}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return format!("signal {sig}");
        }
    }
    status.to_string()
}

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

/// Decide whether the server already on the port is one we may adopt.
///
/// A daemon of ours answers status with this install's key (ADR 0019). An open
/// one — a pre-0019 spawn, or a bare `cargo run -p agent-platform-server` — has
/// no master key and answers without a bearer; still treated as ours, so an
/// upgrade does not have to kill the process it finds. Anything else that is
/// healthy but rejects both is foreign (a Docker forward, another product).
pub fn port_owner(port: u16, key: &str) -> PortOwner {
    if !health_ok(port) {
        return PortOwner::Free;
    }
    if probe(port, "/api/v1/system/status", None) == Some(200) {
        return PortOwner::Ours;
    }
    match probe(port, "/api/v1/system/status", Some(key)) {
        Some(200) => PortOwner::Ours,
        _ => PortOwner::Foreign,
    }
}

/// Bearer the iced client should send. Empty only when the server on the port
/// answers unauthenticated — an open daemon from a pre-[ADR 0019] install, still
/// adopted rather than killed.
///
/// `Free` means we are about to spawn one ourselves, and [`Shell::spawn`] gives
/// it this key.
pub fn client_key(port: u16, install_key: &str, owner: PortOwner) -> String {
    match owner {
        PortOwner::Ours if probe(port, "/api/v1/system/status", None) == Some(200) => String::new(),
        PortOwner::Free | PortOwner::Ours | PortOwner::Foreign => install_key.to_string(),
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

/// The image name of a live process, or `None` when the pid is dead. Asking for
/// the name rather than matching one, because the port takeover needs to *say*
/// what it found before it refuses to kill it.
#[cfg(windows)]
fn process_name(pid: u32) -> Option<String> {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new("tasklist");
    cmd.args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let out = cmd.output().ok()?;
    // `"agent-platformd.exe","1234","Console",…` on a hit; on a miss tasklist
    // prints an INFO sentence instead, which has no leading quote to strip.
    let text = String::from_utf8_lossy(&out.stdout);
    let name = text.lines().next()?.strip_prefix('"')?.split('"').next()?.to_string();
    (!name.is_empty()).then_some(name)
}

#[cfg(not(windows))]
fn process_name(pid: u32) -> Option<String> {
    let out = Command::new("ps").args(["-p", &pid.to_string(), "-o", "comm="]).output().ok()?;
    let comm = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Path::new(&comm)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
}

/// Whether `pid` is a live process running this same executable. The name check
/// matters: pids are recycled, and the file may be days stale.
fn is_our_process(pid: u32) -> bool {
    process_name(pid).as_deref() == Some(exe_name().as_str())
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

/// The pid listening on `port`, from the OS's own socket table.
///
/// Shelling out for the same reason [`process_name`] and `autostart` do: no
/// unsafe, no extra `windows-sys` feature, and it runs when a human presses a
/// button.
#[cfg(windows)]
fn port_pid(port: u16) -> Option<u32> {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new("netstat");
    cmd.args(["-ano", "-p", "TCP"]).creation_flags(0x0800_0000);
    let out = cmd.output().ok()?;
    parse_netstat_pid(&String::from_utf8_lossy(&out.stdout), port)
}

/// The listening row for `port` in `netstat -ano -p TCP` output, as a pid.
///
/// Two things this must not do. It must read the *local* address, the second
/// field: a client of ours connected out to the same port number carries that
/// number in the third, and killing that would kill the wrong process. And it
/// must not read the state word, which is localized — `LISTENING` is absent on
/// a German or Japanese Windows. A listening socket is identified instead by
/// its empty foreign address, which is the same in every locale.
#[cfg(windows)]
fn parse_netstat_pid(output: &str, port: u16) -> Option<u32> {
    let suffix = format!(":{port}");
    output.lines().find_map(|line| {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 5 || !f[0].eq_ignore_ascii_case("TCP") || !f[1].ends_with(&suffix) {
            return None;
        }
        (f[2] == "0.0.0.0:0" || f[2] == "[::]:0" || f[2] == "*:*").then(|| f[4].parse().ok())?
    })
}

#[cfg(not(windows))]
fn port_pid(port: u16) -> Option<u32> {
    let out = Command::new("lsof")
        .args(["-t", "-sTCP:LISTEN", &format!("-itcp:{port}")])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).lines().next()?.trim().parse().ok()
}

/// Stop the server holding `port` — but only when it is one of ours. Returns
/// the pid it stopped, or the reason it stopped nothing.
///
/// The refusal is the feature. [`PortOwner::Foreign`] covers a Docker
/// port-forward and an unrelated service as readily as a stale daemon of ours,
/// and a button that killed whatever answered would be a foot-gun with a
/// helpful label on it. So the image name decides, and anything else is left
/// alone and named in the log.
///
/// `agent-platformd` is the *only* name allowed, deliberately. The app never
/// binds the port itself — the daemon does, always (ADR 0007) — so accepting
/// our own executable's name would widen what this may kill without ever
/// matching a real port holder.
pub fn take_port(port: u16) -> Result<u32, String> {
    let pid = port_pid(port)
        .ok_or_else(|| format!("nothing is listening on port {port} any more"))?;
    let name = process_name(pid)
        .ok_or_else(|| format!("pid {pid} held port {port} and is already gone"))?;
    if name != DAEMON_EXE {
        return Err(format!(
            "port {port} is held by {name} (pid {pid}), which is not an agent-platform server — leaving it alone"
        ));
    }

    kill_process(pid);
    // The same wait `claim_single_instance` does: the socket outlives the
    // process by a moment, and spawning into that window bind-fails.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline && (process_name(pid).is_some() || health_ok(port)) {
        std::thread::sleep(Duration::from_millis(200));
    }
    if health_ok(port) {
        return Err(format!("stopped pid {pid}, but something is still answering on port {port}"));
    }
    Ok(pid)
}

/// Start the app at login, from the per-user `Run` key.
///
/// **Not a Windows service.** This process is the tray UI *and* the server host
/// in one; a service runs in a session with no desktop, so it could not put an
/// icon in the tray or open the window. The `Run` key is what an always-on
/// desktop app (Docker, Ollama, Slack) actually uses: no installer step, no
/// admin rights, and the user can see and remove it from Task Manager's Startup
/// tab.
///
/// **The registry is the state, not `settings.json`.** That Startup tab can
/// disable the entry behind our back, and a copy of the fact in our own file
/// would then be a lie. `reg.exe` rather than the registry API, for the same
/// reason [`is_our_process`] shells out to `tasklist`: no unsafe, no extra
/// `windows-sys` feature, and the call happens twice a session.
#[cfg(windows)]
pub mod autostart {
    use std::process::Command;

    const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "AgentPlatform";

    /// What lands in the key. `--minimized` is the point of the whole thing: at
    /// login the app comes up in the tray with the server running, not as a
    /// window in front of whatever the user was doing. Quoted because the
    /// install path has a space in it (`C:\Program Files\…`) and the flag after
    /// it would otherwise read as part of the path.
    fn command(exe: &std::path::Path) -> String {
        format!("\"{}\" --minimized", exe.display())
    }

    fn reg(args: &[&str]) -> std::io::Result<std::process::Output> {
        use std::os::windows::process::CommandExt;
        Command::new("reg").args(args).creation_flags(0x0800_0000).output() // CREATE_NO_WINDOW
    }

    pub fn enabled() -> bool {
        reg(&["query", RUN_KEY, "/v", VALUE]).is_ok_and(|o| o.status.success())
    }

    pub fn set(on: bool) -> std::io::Result<()> {
        let out = if on {
            let exe = std::env::current_exe()?;
            reg(&["add", RUN_KEY, "/v", VALUE, "/t", "REG_SZ", "/d", &command(&exe), "/f"])?
        } else if enabled() {
            reg(&["delete", RUN_KEY, "/v", VALUE, "/f"])?
        } else {
            return Ok(()); // deleting a value that is not there is an error to reg.exe
        };
        match out.status.success() {
            true => Ok(()),
            false => Err(std::io::Error::other(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            )),
        }
    }

    #[cfg(test)]
    mod tests {
        /// The registry write itself is `reg.exe`'s problem; the quoting is
        /// ours, and an install path with a space in it is the normal case.
        #[test]
        fn the_login_command_quotes_the_path_and_starts_in_the_tray() {
            let line = super::command(std::path::Path::new(
                r"C:\Program Files\Agent Platform\agent-platform.exe",
            ));
            assert_eq!(
                line,
                "\"C:\\Program Files\\Agent Platform\\agent-platform.exe\" --minimized"
            );
        }
    }
}

/// No login entry off Windows — the app only builds there (see plan.md). The
/// stub keeps the Settings toggle compiling rather than `cfg`-ing the screen.
#[cfg(not(windows))]
pub mod autostart {
    pub fn enabled() -> bool {
        false
    }

    pub fn set(_on: bool) -> std::io::Result<()> {
        Err(std::io::Error::other("starting at login is Windows-only"))
    }
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

/// Open a URL in the default browser — the platform launcher [`reveal_path`]
/// already uses takes one just as happily as a path.
pub fn open_url(url: &str) {
    reveal_path(url);
}

/// Start a background program (an LLM backend the user asked us to launch).
/// Errors are returned rather than swallowed: "Launch" that silently does
/// nothing is worse than one that says the command is not installed.
pub fn spawn_detached(program: &str, args: &[&str]) -> std::io::Result<()> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bearer for a server we are about to spawn ourselves. This returned
    /// empty under ADR 0013, when the spawn left the API open; a spawn that
    /// carries the key and a client that sends none is every route 401ing.
    ///
    /// Neither arm here probes the port — only `Ours` does — so this stays
    /// offline and the port number is inert.
    #[test]
    fn a_server_we_are_about_to_spawn_is_reached_with_the_install_key() {
        assert_eq!(client_key(1, "k", PortOwner::Free), "k");
        assert_eq!(client_key(1, "k", PortOwner::Foreign), "k");
    }

    /// The half that protects the user: anything that is not our daemon is
    /// named and left alone. Binds an ephemeral port so it collides with
    /// nothing, and the holder is this test binary — which is exactly the case
    /// that would be fatal if the guard also accepted our own executable name.
    #[test]
    fn take_port_refuses_a_process_that_is_not_our_daemon() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();

        let why = take_port(port).expect_err("took a process that was not our daemon");
        assert!(why.contains(&port.to_string()), "{why}");
        // It says what it found rather than just refusing.
        assert!(why.contains("not an agent-platform server"), "{why}");

        // Still ours, still listening: the refusal did not fire taskkill first
        // and explain afterwards.
        assert_eq!(listener.local_addr().unwrap().port(), port);
    }

    /// The kill path itself, which no amount of parsing proves: a real daemon
    /// of ours on a real port, stopped through `take_port`.
    ///
    /// Ignored by default for the reason the GGUF check is — it needs a built
    /// `agent-platformd` beside the test binary and it binds a port:
    ///
    ///     cargo test -p agent-platform-desktop -- --ignored take_port
    #[test]
    #[ignore]
    fn take_port_stops_a_daemon_of_ours_and_frees_the_port() {
        const PORT: u16 = 18499;
        let exe = std::env::current_exe()
            .unwrap()
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(DAEMON_EXE);
        assert!(exe.is_file(), "build agent-platformd first: {}", exe.display());

        let dir = std::env::temp_dir().join(format!("agp-takeport-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // Every var the daemon would otherwise take from a developer `.env`,
        // which only fills gaps: an unset DATABASE_URL there would put this on
        // a real Postgres instead of a throwaway file.
        let mut child = Command::new(&exe)
            .env("AGENT_PLATFORM_PORT", PORT.to_string())
            .env("AGENT_PLATFORM_MASTER_KEY", "")
            .env("DATABASE_URL", "")
            .env("AGENT_PLATFORM_DB_PATH", dir.join("takeport.db"))
            .env("CONFIG_DIR", &dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !health_ok(PORT) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
        }
        assert!(health_ok(PORT), "the daemon never came up on {PORT}");

        let pid = take_port(PORT).expect("take_port refused a daemon of ours");
        assert_eq!(pid, child.id(), "took the wrong process");
        assert!(!health_ok(PORT), "port {PORT} still answering after the takeover");
        assert!(process_name(pid).is_none(), "pid {pid} outlived the takeover");

        let _ = child.kill();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The takeover kills by pid, so reading the wrong row kills the wrong
    /// process. Both traps are in this fixture: a client of ours *connected to*
    /// 18410 carries that port in its foreign address, and the state word is
    /// not English on a localized Windows.
    #[cfg(windows)]
    #[test]
    fn the_listening_row_is_the_one_the_takeover_reads() {
        let out = "\
  Proto  Local Address          Foreign Address        State           PID
  TCP    127.0.0.1:18410        0.0.0.0:0              LISTENING       4242
  TCP    127.0.0.1:53311        127.0.0.1:18410        ESTABLISHED     9999
  TCP    0.0.0.0:445            0.0.0.0:0              ABH\u{d6}REN        4
";
        assert_eq!(parse_netstat_pid(out, 18410), Some(4242));
        // Still a listener, still found, without the word being read.
        assert_eq!(parse_netstat_pid(out, 445), Some(4));
        assert_eq!(parse_netstat_pid(out, 18411), None);
        // 53311 is a connection, not a listener: nothing there to take.
        assert_eq!(parse_netstat_pid(out, 53311), None);
    }

    /// The point of [`write_atomic`] is that the *old* contents survive a failed
    /// write, where `std::fs::write` would have truncated them first.
    #[test]
    fn a_replaced_file_is_never_seen_half_written() {
        let dir = std::env::temp_dir().join(format!("agp-atomic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        write_atomic(&path, "{\"port\":18410}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"port\":18410}");

        // Replacing an existing file is the case that matters: `rename` has to
        // overwrite rather than fail, which is the one behaviour that differs
        // between platforms.
        write_atomic(&path, "{\"port\":9000}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"port\":9000}");

        // A failed write leaves the previous contents intact, and no `.tmp`
        // beside them to be mistaken for a backup. The directory as a target is
        // the portable way to make the create fail.
        let blocked = dir.join("sub");
        std::fs::create_dir_all(&blocked).unwrap();
        assert!(write_atomic(&blocked, "nope").is_err());
        assert!(!dir.join("sub.tmp").exists(), "the temp file was left behind");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"port\":9000}");

        let _ = std::fs::remove_dir_all(&dir);
    }

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
