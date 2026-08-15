//! Native desktop app: iced daemon owning the Python server sidecar.
//!
//! Ollama-style background behavior: the window's close button asks whether to
//! quit or minimize to tray; tray keeps the daemon running with zero windows
//! (server keeps serving on its fixed port); the tray carries a live server
//! status line plus Show / Talk to E.V. / Open logs / Restart server / Restart
//! app / Quit. Quit kills the child we spawned and
//! hard-exits — `iced::exit()` hangs on Windows wgpu teardown (verified in the
//! Phase 0 spike), and the tray icon must be dropped first or it lingers.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agenda;
mod agenda_chat;
mod agenda_chat_view;
mod agenda_view;
mod apidocs;
mod apidocs_view;
mod assistant;
mod assistant_gate;
mod assistant_tools;
mod assistant_view;
mod assistant_voice;
mod bubble_shader;
mod chat;
mod chat_view;
mod coder;
mod coder_browser;
mod coder_files;
mod coder_git;
mod coder_term;
mod coder_notes;
mod coder_tools;
mod coder_view;
mod domain;
mod graph;
mod history;
mod inference;
mod library;
#[cfg(feature = "local-llm")]
mod local_llm;
#[cfg(feature = "local-llm")]
mod local_server;
mod library_view;
mod logs;
mod memory;
mod memory_view;
mod modelops;
mod modelops_view;
mod notify;
mod processes;
mod processes_view;
mod providers;
mod providers_view;
mod screen;
mod search;
mod search_view;
mod shell;
mod stt;
mod ui;
mod todos;
mod todos_view;
mod update_check;
mod workflows;
mod workflows_view;

use agent_platform_client::sse::ChatChunk;
use agent_platform_client::types::SystemStatus;
use agent_platform_client::Client;
use iced::{window, Element, Subscription, Task};
use shell::{HudStyle, Settings, Shell, ThemeMode};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

/// Top-level destinations. Everything the user configures or inspects rather
/// than works in lives behind [`Screen::Settings`], so the sidebar stays five
/// entries long instead of nine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Dashboard,
    Processes,
    Projects,
    Teams,
    Workflows,
    Plans,
    Agenda,
    Coder,
    Search,
    Assistant,
    Memory,
    Logs,
    Settings,
}

impl Screen {
    /// Whether this screen is useless without the API. Settings opens either
    /// way — it is the page holding Status and Logs, which is where the user
    /// finds out why the server is down. Memory is a local file, readable and
    /// editable whether or not anything is running.
    pub fn needs_server(self) -> bool {
        // Dashboard is the landing page and reports server health itself, so it
        // must render against a dead API rather than hide behind the guard.
        !matches!(self, Screen::Settings | Screen::Memory | Screen::Logs | Screen::Dashboard)
    }

    /// The assistant and its memory share one sidebar entry and one tab strip:
    /// the conversation, and what it remembers of them.
    pub fn is_chat(self) -> bool {
        matches!(self, Screen::Assistant | Screen::Memory)
    }
}

/// Tabs within [`Screen::Settings`]. Ordered configure-then-diagnose: the two
/// you change, then the two you read when something is wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Providers,
    ModelOps,
    Appearance,
    Status,
    Api,
}

impl SettingsTab {
    pub const ALL: [SettingsTab; 5] = [
        SettingsTab::Providers,
        SettingsTab::ModelOps,
        SettingsTab::Appearance,
        SettingsTab::Status,
        SettingsTab::Api,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SettingsTab::Providers => "Providers",
            SettingsTab::ModelOps => "Model ops",
            SettingsTab::Appearance => "Appearance",
            SettingsTab::Status => "Status",
            SettingsTab::Api => "API",
        }
    }

    /// Gating is per tab, not per page: Status and Logs are exactly the tabs a
    /// user needs while the server is not answering, and API still has its
    /// connection details and quickstart to show without one.
    pub fn needs_server(self) -> bool {
        matches!(self, SettingsTab::Providers | SettingsTab::ModelOps)
    }
}

/// What the UI may rely on right now. Derived on demand from the poll result and
/// the child's liveness, so there is one answer and it cannot go stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    /// A server is up and answering authenticated requests.
    Ready,
    /// Our child is alive (or we attached) but the API has not answered yet.
    Starting,
    /// Nothing is answering and no process of ours is running.
    Unreachable,
    /// Another server owns the port; we started nothing and cannot use it.
    Conflict,
}

pub struct LogsState {
    pub lines: Vec<String>,
    pub ring_cursor: u64,
    pub api_cursor: i64,
    pub filter: String,
    /// Show this level and above. `None` shows everything, including the lines
    /// that carry no level at all (startup notes, uvicorn's raw output).
    pub level: Option<logs::Level>,
    pub paused: bool,
    pub dropped: u64,
    /// Absolute id of `lines[0]`. Rows are selected by absolute id, so trimming
    /// the front of the buffer cannot silently reassign a selection to a
    /// different line.
    pub base: u64,
    pub selected: std::collections::HashSet<u64>,
}

pub struct App {
    pub shell: Shell,
    pub settings: Settings,
    pub client: Client,
    pub window: Option<window::Id>,
    /// Set while the in-app quit-or-tray prompt is up; holds the window whose
    /// close was intercepted so "Minimize to tray" knows what to hide.
    pub close_prompt: Option<window::Id>,
    /// The floating E.V. panel is up over whatever screen is open. Ctrl+K
    /// toggles it; it renders `assistant`'s own state, so there is one thread
    /// whichever surface you reach it through.
    pub assistant_open: bool,
    /// The notification panel is up over whatever screen is open. The notes
    /// themselves live in [`notify`], not here — they are posted from module
    /// `update`s that have no `App` to write into.
    pub notifications_open: bool,
    /// Whether the window has the OS focus. Work that finishes behind another
    /// app is work the user did not see finish — see [`watching_key`].
    pub focused: bool,
    /// Whether the login entry is currently registered. Read from the registry,
    /// which is where that fact lives — see [`shell::autostart`]. Cached here
    /// because `view` runs per frame and reading it spawns `reg.exe`.
    pub autostart: bool,
    tray: Option<TrayIcon>,
    /// The tray's disabled status line, and the text it currently shows.
    tray_status: Option<MenuItem>,
    tray_status_text: String,
    /// Which plate the tray icon currently carries, so the health poll can spot
    /// an OS theme switch and repaint it.
    tray_light_plate: bool,
    pub screen: Screen,
    /// Which tab the Settings page shows; remembered across visits.
    pub settings_tab: SettingsTab,
    /// Edit buffer for the local model's context size — the settings field is a
    /// `u32`, and a half-typed number is not one.
    pub local_ctx_input: String,
    /// The same, for the port the local model is served on. Empty means off.
    pub local_server_port_input: String,
    /// Which of the two chat tabs the sidebar entry returns to.
    pub chat_tab: Screen,
    pub status: Option<SystemStatus>,
    pub status_error: Option<String>,
    pub child_alive: bool,
    /// Another server owns our port; we started nothing and cannot use it.
    pub port_conflict: bool,
    pub key_revealed: bool,
    pub copied: Option<&'static str>,
    pub logs: LogsState,
    pub processes: processes::State,
    pub library: library::State,
    pub modelops: modelops::State,
    pub assistant: assistant::State,
    /// What both assistants remember about the user, across restarts.
    pub memory: memory::Store,
    /// Past conversations of both assistants, across restarts.
    pub history: history::Store,
    pub providers: providers::State,
    pub workflows: workflows::State,
    pub todos: todos::State,
    pub agenda: agenda::State,
    pub coder: coder::State,
    pub search: search::State,
    pub apidocs: apidocs::State,
    /// Whether a newer build has been published. Only ever filled by the user
    /// pressing the button in Settings → Status — nothing here phones home on
    /// its own.
    pub update_check: update_check::State,
}

impl App {
    /// Single source of truth for "can the UI talk to the API yet". Every gate —
    /// nav locks, screen guards, poll intervals — reads this and nothing else.
    pub fn server_state(&self) -> ServerState {
        if self.port_conflict {
            ServerState::Conflict
        } else if self.status.is_some() {
            ServerState::Ready
        } else if self.child_alive || self.shell.attached {
            ServerState::Starting
        } else {
            ServerState::Unreachable
        }
    }

    pub fn server_ready(&self) -> bool {
        self.server_state() == ServerState::Ready
    }

    /// Whether what the user is currently looking at can actually work. The
    /// single guard: the sidebar, the settings tabs, the content area and the
    /// pollers all ask this one question, so none of them can disagree.
    /// What the user is actually looking at — `None` while the window is
    /// closed to the tray or sitting behind another app.
    fn on_screen(&self) -> Option<(Screen, SettingsTab)> {
        (self.window.is_some() && self.focused).then_some((self.screen, self.settings_tab))
    }

    pub fn view_available(&self) -> bool {
        let needs_server = match self.screen {
            Screen::Settings => self.settings_tab.needs_server(),
            // Both tabbed pages gate per tab rather than per page: each holds at
            // least one tab that works with the server down, and the tab strip
            // is how the user reaches it.
            s if s.is_chat() => false,
            other => other.needs_server(),
        };
        !needs_server || self.server_ready()
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Tray(String),
    WindowOpened(window::Id),
    /// The window gained or lost the OS focus.
    WindowFocus(bool),
    WindowCloseRequested(window::Id),
    CloseConfirmed,
    MinimizeToTray,
    CloseCancelled,
    /// Esc while a modal is up; routed to whichever modal is open.
    EscapePressed,
    /// Ctrl+K, the overlay's close button, or the sidebar: show or hide the
    /// floating E.V. panel.
    ToggleAssistant,
    /// A toast's time is up (or its close button was pressed).
    NoticeExpired,
    /// The sidebar bell: show or hide the notification panel.
    ToggleNotifications,
    /// A row in that panel: go to the screen the note came from. Arriving there
    /// is what marks it (and the rest of that screen's) seen.
    OpenNote(u64),
    /// Throw one note away without going anywhere.
    DismissNote(u64),
    ClearNotifications,
    WindowClosed(window::Id),
    Nav(Screen),
    NavSettings(SettingsTab),
    StatusTick,
    StatusFetched(Result<SystemStatus, String>),
    LogsTick,
    ApiLogs(Result<(Vec<String>, i64, i64), String>),
    LogFilterChanged(String),
    /// "View logs" on a traced error banner: jump to the Logs screen filtered
    /// to the request that failed.
    TraceLogs(String),
    /// Level filter: show this level and above, `None` for everything.
    SetLogLevel(Option<logs::Level>),
    /// Drop both halves of the filter at once — the way back from a trace jump.
    ClearLogFilter,
    ToggleLogsPaused,
    ClearLogs,
    /// Click a log row: toggle it in the selection.
    ToggleLogLine(u64),
    /// Copy the selection, or every line matching the filter when nothing is
    /// selected — the common case is "give me what I am looking at".
    CopyLogs,
    SelectAllLogs,
    ClearLogSelection,
    ToggleKeyRevealed,
    SetTheme(ThemeMode),
    SetHudStyle(HudStyle),
    /// How fast E.V. reads aloud, as percent of the voice's normal pace.
    SetVoiceRate(i32),
    /// Wake-word standby on or off. Persisted, and the only thing that opens
    /// the mic without a button press.
    SetWakeWord(bool),
    /// What the assistant is called. Persisted.
    SetAssistantName(String),
    /// Comma-separated spellings the wake word answers to. Persisted.
    SetWakeNames(String),
    /// The TTS voice id. Persisted.
    SetVoiceName(String),
    /// Confirm card before E.V. runs a shell command. Persisted.
    SetConfirmCommands(bool),
    /// Start the app at login. Persisted in the registry, not `settings.json`.
    SetAutostart(bool),
    /// Come up in the tray with no window. Persisted.
    SetStartMinimized(bool),
    PickLocalModel,
    /// The GGUF for in-process inference: `None` is a cancelled picker, and an
    /// empty string clears the setting (back to server-answered turns).
    SetLocalModel(Option<String>),
    SetLocalCtx(String),
    SetLocalServerPort(String),
    UnloadLocalModel,
    Copy(&'static str, String),
    RestartServer,
    RestartApp,
    RevealPath(String),
    /// "Check for updates" in Settings → Status, and its answer.
    CheckForUpdate,
    UpdateChecked(Result<Option<String>, String>),
    Quit,
    Processes(processes::Message),
    Library(library::Message),
    ModelOps(modelops::Message),
    Assistant(assistant::Message),
    Memory(memory::Message),
    History(history::Message),
    Providers(providers::Message),
    Workflows(workflows::Message),
    Todos(todos::Message),
    Agenda(agenda::Message),
    Coder(coder::Message),
    Search(search::Message),
    ApiDocs(apidocs::Message),
}

/// One frame of the app icon as RGBA, picked by its edge in pixels.
///
/// `icon.ico` is the same file `build.rs` embeds in the exe, but that resource
/// only reaches Explorer and the taskbar: winit leaves the window class icon
/// unset, so without this the title bar shows Windows' default. Every frame in
/// the file is an 8-bit RGBA PNG, which is the only encoding handled here — an
/// ICO holding BMP frames would need the DIB path too.
fn frame_rgba(px: u8) -> Option<(Vec<u8>, u32, u32)> {
    const ICO: &[u8] = include_bytes!("../icon.ico");
    // ICONDIR: 6-byte header, then one 16-byte ICONDIRENTRY per frame, whose
    // first byte is the width (0 meaning 256) and whose last two fields are
    // the frame's byte length and offset.
    let count = u16::from_le_bytes([ICO[4], ICO[5]]) as usize;
    let entry = (0..count).map(|i| 6 + i * 16).find(|&o| ICO[o] == px)?;
    let len = u32::from_le_bytes(ICO[entry + 8..entry + 12].try_into().ok()?) as usize;
    let off = u32::from_le_bytes(ICO[entry + 12..entry + 16].try_into().ok()?) as usize;
    let mut reader = png::Decoder::new(ICO.get(off..off + len)?).read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    buf.truncate(info.buffer_size());
    Some((buf, info.width, info.height))
}

/// The app icon with its plate recolored for the surface it will sit on.
///
/// The asset ships a near-black plate behind a red robot. On a dark surface that
/// plate disappears, so the neutral pixels — the plate and its antialiased edge;
/// the robot is saturated — are inverted into a light plate. Alpha is untouched,
/// so the rounded corners survive.
fn logo_rgba(px: u8, light_plate: bool) -> Option<(Vec<u8>, u32, u32)> {
    let (mut rgba, w, h) = frame_rgba(px)?;
    if light_plate {
        for p in rgba.chunks_exact_mut(4) {
            let (lo, hi) = (p[0].min(p[1]).min(p[2]), p[0].max(p[1]).max(p[2]));
            if hi - lo <= 24 {
                for c in &mut p[..3] {
                    *c = 255 - *c;
                }
            }
        }
    }
    Some((rgba, w, h))
}

/// The sidebar mark, cached per plate so the view does not re-upload a texture
/// every frame. 48px covers the 28pt slot up to a 1.5× scale factor.
pub fn logo_handle(dark_surface: bool) -> iced::widget::image::Handle {
    use std::sync::OnceLock;
    static CACHE: [OnceLock<iced::widget::image::Handle>; 2] = [OnceLock::new(), OnceLock::new()];
    CACHE[dark_surface as usize]
        .get_or_init(|| {
            let (rgba, w, h) = logo_rgba(48, !dark_surface).expect("no 48x48 frame in icon.ico");
            iced::widget::image::Handle::from_rgba(w, h, rgba)
        })
        .clone()
}

fn open_window() -> Task<Message> {
    let (_id, task) = window::open(window::Settings {
        size: iced::Size::new(1440.0, 900.0),
        min_size: Some(iced::Size::new(820.0, 560.0)),
        // Title bar and taskbar are OS chrome: the plate has to contrast with
        // the *system* theme, not with whatever theme the app is set to.
        icon: logo_rgba(32, shell::system_is_dark())
            .and_then(|(rgba, w, h)| window::icon::from_rgba(rgba, w, h).ok()),
        // Close is intercepted: we ask quit-or-tray instead of just closing.
        exit_on_close_request: false,
        ..window::Settings::default()
    });
    task.map(Message::WindowOpened)
}

fn tray_icon_image(light_plate: bool) -> Option<tray_icon::Icon> {
    let (rgba, w, h) = logo_rgba(32, light_plate)?;
    tray_icon::Icon::from_rgba(rgba, w, h).ok()
}

/// The tray menu, plus the disabled status line the health poll keeps current
/// (see [`sync_tray_status`]) — it is the only item whose text changes.
fn build_tray(port: u16) -> Option<(TrayIcon, MenuItem)> {
    let menu = Menu::new();
    let status = MenuItem::with_id("server", &format!("Server: 127.0.0.1:{port}"), false, None);
    menu.append(&status).ok()?;
    let items: [&dyn tray_icon::menu::IsMenuItem; 9] = [
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id("show", "Show Agent Platform", true, None),
        &MenuItem::with_id("assistant", assistant::talk_label(), true, None),
        &MenuItem::with_id("logs", "Open logs", true, None),
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id("restart", "Restart server", true, None),
        &MenuItem::with_id("restart-app", "Restart app", true, None),
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id("quit", "Quit", true, None),
    ];
    for item in items {
        menu.append(item).ok()?;
    }
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Agent Platform")
        // The notification area follows the system theme, so a dark system gets
        // the light plate.
        .with_icon(tray_icon_image(shell::system_is_dark())?)
        .build()
        .ok()?;
    Some((tray, status))
}

/// The tray's status line, rebuilt from the health poll the app already runs —
/// no extra request, and it stays right while the window is closed.
fn tray_status_text(app: &App) -> String {
    let addr = format!("127.0.0.1:{}", app.shell.port);
    match app.server_state() {
        ServerState::Ready => {
            let active = app.status.as_ref().map(|s| s.processes.active).unwrap_or(0);
            format!("Server: running on {addr} · {active} active")
        }
        ServerState::Starting => format!("Server: starting on {addr}"),
        ServerState::Unreachable => format!("Server: stopped ({addr})"),
        ServerState::Conflict => format!("Server: port {} taken", app.shell.port),
    }
}

/// Push the current status into the tray, skipping the no-op writes so an open
/// menu is not repainted every poll.
fn sync_tray_status(app: &mut App) {
    let text = tray_status_text(app);
    if text == app.tray_status_text {
        return;
    }
    if let Some(item) = &app.tray_status {
        item.set_text(&text);
    }
    app.tray_status_text = text;
}

/// Bring the window up for a tray action: open it if the app is running with no
/// windows, otherwise raise the one that is already there.
fn show_window(app: &App) -> Task<Message> {
    match app.window {
        Some(id) => window::gain_focus(id),
        None => open_window(),
    }
}

fn boot() -> (App, Task<Message>) {
    let app_dir = shell::app_dir();
    let settings = Settings::load(&app_dir);
    let port = std::env::var("AGENT_PLATFORM_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(settings.port);

    let log = std::sync::Arc::new(std::sync::Mutex::new(shell::LogRing::new()));
    let daemon = shell::resolve_server().unwrap_or_else(|| {
        log.lock()
            .unwrap()
            .push("[shell] agent-platformd is missing from the install directory".to_string());
        Default::default()
    });

    let key = shell::load_or_create_key(&app_dir).unwrap_or_else(|e| {
        log.lock().unwrap().push(format!("[shell] could not read or create the install key: {e}"));
        String::new()
    });

    // One app at a time, dev or installed: an earlier instance is killed and
    // waited out before we look at the port, so we never probe a dying sidecar.
    let replaced = shell::claim_single_instance(&app_dir, port);

    // Ollama-style: if one of OUR servers is already on the port (a bare
    // `start.py` dev run), attach as a pure client instead of spawning.
    let owner = shell::port_owner(port, &key);
    let attached = owner == shell::PortOwner::Ours;

    let mut sh = Shell {
        server: None,
        log,
        daemon,
        port,
        key: key.clone(),
        data_dir: app_dir.clone(),
        attached,
    };
    if let Some(note) = replaced {
        sh.log_line(note);
    }
    let port_conflict = owner == shell::PortOwner::Foreign;
    match owner {
        shell::PortOwner::Ours => {
            sh.log_line(format!("[shell] found our server on port {port}; attached to it"))
        }
        shell::PortOwner::Foreign => sh.log_line(format!(
            "[shell] port {port} is already used by another server that rejects this \
             install's key. Not starting a server. Set AGENT_PLATFORM_PORT or edit \
             {}/settings.json to pick a free port.",
            app_dir.display()
        )),
        shell::PortOwner::Free => sh.start_server(),
    }

    let client = Client::new(sh.origin(), key);
    // Before the tray is built and before anything renders or speaks: the name
    // is read by every view and by the tray item, the voice by the first
    // sentence synthesized.
    assistant::set_identity(&settings.assistant_name, &settings.wake_names);
    assistant::set_voice(&settings.voice_name);
    let (tray, tray_status) = match build_tray(port) {
        Some((tray, status)) => (Some(tray), Some(status)),
        None => {
            sh.log_line("[shell] tray unavailable".to_string());
            (None, None)
        }
    };

    let minimized =
        settings.start_minimized || std::env::args().any(|a| a == "--minimized");
    let (chat_provider, chat_model) =
        (settings.chat_provider.clone(), settings.chat_model.clone());
    let voice_rate = settings.voice_rate;
    let confirm_commands = settings.confirm_commands;
    let coder_workspace = settings.coder_workspace.clone();
    // Coder keeps its own pair because it needs a model that can hold a tool
    // loop, but an unset one follows the app-wide default rather than dropping
    // to the server's `llama3`, which cannot.
    let (coder_provider, coder_model) = match settings.coder_model.is_empty() {
        true => (chat_provider.clone(), chat_model.clone()),
        false => (settings.coder_provider.clone(), settings.coder_model.clone()),
    };
    let coder_plan = settings.coder_plan;
    let local_n_ctx = settings.local_n_ctx;
    let local_server_port = settings.local_server_port;

    // The OpenAI-compatible endpoint in front of the local model, for the
    // server's own agents. Off unless a port is set, and a port that will not
    // bind is a log line rather than a failed startup — the app's own chat does
    // not need it.
    #[cfg(feature = "local-llm")]
    if settings.local_server_port != 0 {
        match local_server::start(settings.local_server_port) {
            Ok(addr) => sh.log_line(format!("[local-llm] serving OpenAI-compatible on http://{addr}")),
            Err(e) => sh.log_line(format!(
                "[local-llm] could not bind port {}: {e}",
                settings.local_server_port
            )),
        }
    }

    let app = App {
        shell: sh,
        settings,
        client,
        window: None,
        close_prompt: None,
        assistant_open: false,
        notifications_open: false,
        // Corrected by the first focus event; a window that opens is focused,
        // and one that never opens is covered by `window: None`.
        focused: true,
        autostart: shell::autostart::enabled(),
        tray,
        tray_status,
        tray_status_text: String::new(),
        tray_light_plate: shell::system_is_dark(),
        screen: Screen::Dashboard,
        settings_tab: SettingsTab::Providers,
        local_ctx_input: local_n_ctx.to_string(),
        local_server_port_input: match local_server_port {
            0 => String::new(),
            p => p.to_string(),
        },
        chat_tab: Screen::Assistant,
        status: None,
        status_error: None,
        child_alive: false,
        port_conflict,
        key_revealed: false,
        copied: None,
        logs: LogsState {
            lines: Vec::new(),
            ring_cursor: 0,
            api_cursor: 0,
            filter: String::new(),
            level: None,
            paused: false,
            dropped: 0,
            base: 0,
            selected: std::collections::HashSet::new(),
        },
        processes: processes::State::default(),
        library: library::State::default(),
        modelops: modelops::State::default(),
        assistant: assistant::State::with_defaults(
            chat_provider,
            chat_model,
            voice_rate,
            confirm_commands,
        ),
        memory: memory::Store::load(&app_dir),
        history: history::Store::load(&app_dir),
        providers: providers::State::default(),
        workflows: workflows::State::default(),
        todos: todos::State::default(),
        agenda: agenda::State::default(),
        coder: coder::State::restored(&coder_workspace, coder_provider, coder_model, coder_plan),
        search: search::State::default(),
        apidocs: apidocs::State::default(),
        update_check: update_check::State::default(),
    };
    let task = if minimized { Task::none() } else { open_window() };
    let mut bootstrap = vec![
        Task::done(Message::StatusTick),
        processes::load_lists(&app.client).map(Message::Processes),
        Task::done(Message::Processes(processes::Message::ListTick)),
    ];
    // The wake word survives a restart, so the mic has to come back up with it.
    // Routed through the message rather than set on the struct: opening the mic
    // is the half that can fail, and this way a missing device turns the setting
    // back off with an error instead of leaving a state that claims to listen.
    if app.settings.wake_word {
        bootstrap.push(Task::done(Message::SetWakeWord(true)));
    }
    let bootstrap = Task::batch(bootstrap);
    (app, task.chain(bootstrap))
}

/// What the context box accepts: digits only, and few enough of them that no
/// context is a typo away from an allocation nobody has the VRAM for. The empty
/// string is allowed through so the field can be cleared and retyped — it just
/// does not parse, so the stored setting keeps its last good value.
fn ctx_digits(raw: &str) -> String {
    raw.chars().filter(char::is_ascii_digit).take(7).collect()
}

/// Persist `settings.json`, logging rather than failing: every caller is a
/// preference change the user already made in the UI.
fn save_settings(app: &mut App) {
    if let Err(e) = app.settings.save(&app.shell.data_dir) {
        app.shell.log_line(format!("[shell] could not save settings: {e}"));
    }
}

/// The chat provider/model override survives restarts: any change lands in
/// `settings.json` the moment it is made, so a fresh thread on the next launch
/// opens on the same pair. Reopening a saved conversation goes through here too
/// — the thread's own pair becomes the new default.
fn persist_chat_model(app: &mut App) {
    if app.settings.chat_provider != app.assistant.provider
        || app.settings.chat_model != app.assistant.model
    {
        app.settings.chat_provider = app.assistant.provider.clone();
        app.settings.chat_model = app.assistant.model.clone();
        save_settings(app);
    }
}

fn quit(app: &mut App) -> ! {
    if !app.shell.attached {
        app.shell.stop_server();
    }
    drop(app.tray.take()); // remove the tray icon before the hard exit
    std::process::exit(0)
}

/// "View logs" on any traced error banner, from any screen: jump to
/// Settings → Logs pre-filtered to the request that failed. One request logs
/// under the same id on both servers ([`request_id`] on the Rust side,
/// `app/observability.py` on the Python side), so the filter finds the line
/// regardless of which server answered.
fn trace_logs_task(app: &mut App, trace_id: String) -> Task<Message> {
    app.logs.filter = trace_id;
    app.logs.paused = false;
    app.screen = Screen::Logs;
    app.copied = None;
    enter_screen(app)
}

/// The one fetch the current view needs on entry. Skipped entirely while the
/// server is not ready, so a blocked view never fires a request that can only
/// fail — [`Message::StatusFetched`] replays it the moment the server answers.
fn enter_screen(app: &mut App) -> Task<Message> {
    // The Coder preview is a child window, not something iced draws, so it has
    // no z-order against wgpu content: left alone it floats over whatever
    // screen comes next. Leaving Coder takes it off the screen; entering puts
    // it back (see the `Screen::Coder` arm), repositioned for a window that may
    // have been resized while it was hidden.
    if app.screen != Screen::Coder && app.coder.browser_open {
        return Task::batch([
            Task::done(Message::Coder(coder::Message::BrowserHide)),
            enter_screen_inner(app),
        ]);
    }
    enter_screen_inner(app)
}

fn enter_screen_inner(app: &mut App) -> Task<Message> {
    if !app.view_available() {
        return Task::none();
    }
    match app.screen {
        // Status is polled globally; the dashboard has nothing extra to fetch.
        Screen::Dashboard => Task::none(),
        Screen::Processes => Task::done(Message::Processes(processes::Message::ListTick)),
        Screen::Projects | Screen::Teams => {
            Task::done(Message::Library(library::Message::Refresh))
        }
        Screen::Workflows => Task::done(Message::Workflows(workflows::Message::Refresh)),
        Screen::Plans => Task::done(Message::Todos(todos::Message::Refresh)),
        Screen::Agenda => Task::done(Message::Agenda(agenda::Message::Refresh)),
        // History is server-owned now (`/api/v1/search/history`) — another
        // window, or E.V., can have added to it since the last visit.
        Screen::Search => Task::done(Message::Search(search::Message::LoadHistory)),
        // Past sessions every visit — another window, or the CLI, can have
        // added one. The model dropdowns are fetched once; they change only
        // when a provider is configured, which is a different screen.
        Screen::Coder => {
            let mut tasks = vec![
                coder::load_threads(&mut app.coder, &app.client).map(Message::Coder),
                // Checkpoints live in the folder, so another window — or the
                // user's own `git` — can have moved them since the last visit.
                coder::load_checkpoints(&mut app.coder).map(Message::Coder),
            ];
            if app.coder.catalog.is_empty() {
                tasks.push(coder::load_catalog(&app.client).map(Message::Coder));
            }
            // The preview is a child window, not something iced draws, so it
            // has to be put back on screen by hand — and repositioned, since
            // the window may have been resized while it was hidden.
            if app.coder.browser_open {
                tasks.push(Task::done(Message::Coder(coder::Message::BrowserSync)));
            }
            Task::batch(tasks)
        }
        // The dropdowns offer only configured providers, so the catalog is
        // refetched on every entry rather than cached: configuring one in
        // Settings has to make it selectable here without a restart. Chat
        // itself works without it, so a failed load costs nothing but empty
        // pickers.
        Screen::Assistant => assistant::load_catalog(&app.client).map(Message::Assistant),
        Screen::Memory => Task::none(),
        Screen::Logs => Task::done(Message::LogsTick),
        Screen::Settings => match app.settings_tab {
            SettingsTab::ModelOps => Task::done(Message::ModelOps(modelops::Message::Refresh)),
            SettingsTab::Providers => Task::done(Message::Providers(providers::Message::Refresh)),
            // The endpoint list is fetched once and kept: the surface changes
            // when the server is rebuilt, not while it runs. `Reload` on the
            // page is the way to ask again.
            SettingsTab::Api => Task::done(Message::ApiDocs(apidocs::Message::Refresh)),
            SettingsTab::Status | SettingsTab::Appearance => Task::none(),
        },
    }
}

fn fetch_status(client: &Client) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move { client.system_status().await.map_err(|e| e.to_string()) },
        Message::StatusFetched,
    )
}

/// Every surface that runs work in the background, as the key it passes to
/// [`notify::away`] and the destination a note about it navigates to. One table
/// in both directions, so a badge can never point somewhere the note did not
/// come from.
const NOTIFY_KEYS: &[(&str, Screen, Option<SettingsTab>)] = &[
    ("processes", Screen::Processes, None),
    ("coder", Screen::Coder, None),
    ("assistant", Screen::Assistant, None),
    ("workflows", Screen::Workflows, None),
    ("modelops", Screen::Settings, Some(SettingsTab::ModelOps)),
];

/// Which surface's key the user is looking at, for [`notify::away`]. `None`
/// stands for a window that is hidden or behind another app: nothing is being
/// watched there, so everything that finishes gets a toast.
///
/// Screens with no background work of their own share the empty key with that
/// case — it matches nothing, which is exactly right for them too.
fn watching_key(on_screen: Option<(Screen, SettingsTab)>) -> &'static str {
    let Some((screen, tab)) = on_screen else { return "" };
    NOTIFY_KEYS
        .iter()
        .find(|(_, s, t)| *s == screen && t.map_or(true, |want| want == tab))
        .map(|(key, _, _)| *key)
        .unwrap_or("")
}

/// The badge on a sidebar entry: how much happened there while the user was
/// elsewhere, and whether any of it is *waiting* on them rather than done.
pub fn screen_notes(screen: Screen) -> (usize, ui::Tone) {
    let key = NOTIFY_KEYS
        .iter()
        .find(|(_, s, t)| *s == screen && t.is_none())
        .map(|(key, _, _)| *key)
        .unwrap_or("");
    (notify::count(key), note_tone(key))
}

/// Warning when something is blocked on the user, Info when it merely finished.
pub fn note_tone(key: &str) -> ui::Tone {
    if notify::review_waiting(key) {
        ui::Tone::Warning
    } else {
        ui::Tone::Info
    }
}

/// Where a note's key says to go.
fn note_destination(key: &str) -> Option<Message> {
    NOTIFY_KEYS.iter().find(|(k, _, _)| *k == key).map(|(_, screen, tab)| match tab {
        Some(tab) => Message::NavSettings(*tab),
        None => Message::Nav(*screen),
    })
}

/// One line of a finished turn, for its toast: the error if it failed, else the
/// first thing it actually said.
fn preview(error: Option<&str>, text: &str) -> String {
    let line = error.unwrap_or(text).lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    match line.char_indices().nth(140) {
        _ if line.is_empty() => "Finished.".to_string(),
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_string(),
    }
}

fn update(app: &mut App, message: Message) -> Task<Message> {
    let task = dispatch(app, message);
    // Rewritten after every message rather than in the handful of arms that
    // move the user: one write cannot drift from the screen that is actually
    // on, and five of them can.
    let key = watching_key(app.on_screen());
    notify::watching(key);
    // Arriving on a screen is seeing what happened there. The same one write:
    // a badge that outlives the visit it was about is worse than no badge.
    notify::seen(key);
    task
}

fn dispatch(app: &mut App, message: Message) -> Task<Message> {
    match message {
        Message::Tray(id) => match id.as_str() {
            "show" => show_window(app),
            // The window has to exist before the nav means anything on screen,
            // but the nav itself is state, so the order does not matter. E.V. is
            // the Assistant tab specifically, not whichever chat tab was last
            // open — Memory is the other one.
            "assistant" => Task::batch([
                show_window(app),
                Task::done(Message::Nav(Screen::Assistant)),
            ]),
            "logs" => Task::batch([
                show_window(app),
                Task::done(Message::Nav(Screen::Logs)),
            ]),
            "restart" => update(app, Message::RestartServer),
            "restart-app" => update(app, Message::RestartApp),
            "quit" => quit(app),
            _ => Task::none(),
        },
        Message::WindowOpened(id) => {
            app.window = Some(id);
            Task::none()
        }
        Message::WindowFocus(focused) => {
            app.focused = focused;
            Task::none()
        }
        Message::WindowCloseRequested(id) => {
            // The prompt is drawn in-app (see `screen::view`), so the choice
            // arrives as one of the three messages below.
            app.close_prompt = Some(id);
            Task::none()
        }
        Message::NoticeExpired => {
            // One clear for every screen that owns a `notice`: only one is on
            // screen at a time, and a stale one behind it should go too. Errors
            // are untouched — they stay as inline banners until dismissed.
            app.library.notice.clear();
            app.processes.notice.clear();
            app.modelops.notice.clear();
            app.providers.notice.clear();
            app.workflows.notice.clear();
            Task::none()
        }
        Message::CloseCancelled => {
            app.close_prompt = None;
            Task::none()
        }
        Message::ToggleNotifications => {
            app.notifications_open = !app.notifications_open;
            Task::none()
        }
        Message::OpenNote(id) => {
            app.notifications_open = false;
            // The note is gone either way — clicking it is seeing it — but a key
            // with no destination (a screen since removed) must not navigate.
            match notify::take(id).and_then(note_destination) {
                Some(nav) => update(app, nav),
                None => Task::none(),
            }
        }
        Message::DismissNote(id) => {
            notify::take(id);
            Task::none()
        }
        Message::ClearNotifications => {
            notify::clear();
            app.notifications_open = false;
            Task::none()
        }
        Message::ToggleAssistant => {
            // On a chat screen the panel is suppressed anyway — it would sit
            // beside the same conversation — so the shortcut just goes there.
            // `assistant_open` stays *off*: flipping it here left the panel
            // primed to appear on its own the next time the user navigated
            // somewhere else, which is not what pressing it on this screen
            // asked for.
            if app.screen.is_chat() {
                return update(app, Message::Nav(Screen::Assistant));
            }
            app.assistant_open = !app.assistant_open;
            Task::none()
        }
        Message::EscapePressed => {
            if app.close_prompt.is_some() {
                app.close_prompt = None;
                Task::none()
            } else if app.notifications_open {
                // Above Abort: the panel is what Esc most obviously means while
                // it is covering the screen, and it stops nothing to close it.
                app.notifications_open = false;
                Task::none()
            } else if app.assistant.sending || app.assistant.speaking() {
                // A reply that is still arriving or still being read out loud is
                // the most urgent thing Esc could mean.
                update(app, Message::Assistant(assistant::Message::Abort))
            } else if app.assistant_open {
                // Below Abort on purpose: a turn in flight is the more urgent
                // thing to stop, and the panel is one more Esc away.
                app.assistant_open = false;
                Task::none()
            } else {
                update(
                    app,
                    Message::Library(library::Message::CancelConfirm),
                )
            }
        }
        Message::CloseConfirmed => {
            app.close_prompt = None;
            quit(app)
        }
        Message::MinimizeToTray => match app.close_prompt.take() {
            Some(id) => window::close(id),
            None => Task::none(),
        },
        Message::WindowClosed(id) => {
            if app.window == Some(id) {
                app.window = None;
            }
            Task::none()
        }
        Message::Nav(screen) => {
            app.screen = screen;
            if screen.is_chat() {
                app.chat_tab = screen;
            }
            app.copied = None;
            enter_screen(app)
        }
        Message::NavSettings(tab) => {
            app.screen = Screen::Settings;
            app.settings_tab = tab;
            app.copied = None;
            enter_screen(app)
        }
        Message::StatusTick => {
            app.child_alive = app.shell.server_running();
            // The one timer that always runs, so it is also where an OS theme
            // switch is noticed: the in-app theme re-resolves on every render,
            // but the tray icon is a bitmap that has to be repainted.
            let dark = shell::system_is_dark();
            if dark != app.tray_light_plate {
                app.tray_light_plate = dark;
                if let (Some(tray), Some(icon)) = (&app.tray, tray_icon_image(dark)) {
                    let _ = tray.set_icon(Some(icon));
                }
            }
            fetch_status(&app.client)
        }
        Message::StatusFetched(result) => {
            let was = app.server_state();
            match result {
                Ok(status) => {
                    app.status = Some(status);
                    app.status_error = None;
                }
                Err(e) => {
                    // A failed poll means the API is gone, not merely noisy: drop
                    // the last report so the guard closes instead of showing a
                    // screen backed by data that is no longer being refreshed.
                    app.status = None;
                    app.status_error = Some(e);
                }
            }
            sync_tray_status(app);
            // The screen the user is already looking at was blocked while the
            // server came up; load it now rather than waiting for a re-click.
            if was != ServerState::Ready && app.server_state() == ServerState::Ready {
                enter_screen(app)
            } else {
                Task::none()
            }
        }
        Message::LogsTick => {
            if app.logs.paused {
                return Task::none();
            }
            if app.shell.attached {
                // We own no child pipes; the server's own ring is the source.
                let client = app.client.clone();
                let after = app.logs.api_cursor;
                Task::perform(
                    async move {
                        client
                            .system_logs(after)
                            .await
                            .map(|c| (c.lines, c.next, c.dropped))
                            .map_err(|e| e.to_string())
                    },
                    Message::ApiLogs,
                )
            } else {
                let chunk = app.shell.log.lock().unwrap().since(app.logs.ring_cursor);
                app.logs.ring_cursor = chunk.next;
                app.logs.dropped += chunk.dropped;
                app.logs.lines.extend(chunk.lines);
                trim_log_view(&mut app.logs);
                Task::none()
            }
        }
        Message::ApiLogs(result) => {
            if let Ok((lines, next, dropped)) = result {
                app.logs.api_cursor = next;
                app.logs.dropped += dropped.max(0) as u64;
                app.logs.lines.extend(lines);
                trim_log_view(&mut app.logs);
            }
            Task::none()
        }
        Message::LogFilterChanged(f) => {
            app.logs.filter = f;
            Task::none()
        }
        Message::TraceLogs(trace_id) => trace_logs_task(app, trace_id),
        Message::ClearLogFilter => {
            app.logs.filter.clear();
            app.logs.level = None;
            Task::none()
        }
        Message::SetLogLevel(level) => {
            app.logs.level = level;
            Task::none()
        }
        Message::ToggleLogsPaused => {
            app.logs.paused = !app.logs.paused;
            Task::none()
        }
        Message::ClearLogs => {
            app.logs.base += app.logs.lines.len() as u64;
            app.logs.lines.clear();
            app.logs.selected.clear();
            Task::none()
        }
        Message::ToggleLogLine(id) => {
            if !app.logs.selected.remove(&id) {
                app.logs.selected.insert(id);
            }
            Task::none()
        }
        Message::SelectAllLogs => {
            app.logs.selected = visible_log_ids(&app.logs).collect();
            Task::none()
        }
        Message::ClearLogSelection => {
            app.logs.selected.clear();
            Task::none()
        }
        Message::CopyLogs => {
            let text = copy_text(&app.logs);
            if text.is_empty() {
                return Task::none();
            }
            update(app, Message::Copy("logs", text))
        }
        Message::SetTheme(mode) => {
            app.settings.theme = mode;
            save_settings(app);
            Task::none()
        }
        Message::SetHudStyle(style) => {
            app.settings.hud_style = style;
            save_settings(app);
            Task::none()
        }
        Message::SetConfirmCommands(on) => {
            app.settings.confirm_commands = on;
            app.assistant.confirm_commands = on;
            save_settings(app);
            Task::none()
        }
        Message::SetAutostart(on) => {
            // Re-read rather than assume: the registry is the state, so a failed
            // write must leave the toggle showing what is actually there.
            if let Err(e) = shell::autostart::set(on) {
                app.shell.log_line(format!("[shell] could not change the login entry: {e}"));
            }
            app.autostart = shell::autostart::enabled();
            Task::none()
        }
        Message::SetStartMinimized(on) => {
            app.settings.start_minimized = on;
            save_settings(app);
            Task::none()
        }
        Message::SetWakeWord(on) => {
            // The assistant owns the recorder, so it does the opening and the
            // closing — and opening is the half that can fail.
            let task = update(app, Message::Assistant(assistant::Message::SetStandby(on)));
            // Mirror what actually happened, not what was asked for. A missing
            // or refused microphone leaves `standby` false, and a setting that
            // still said `true` would show "Listening for E.V." next to a mic
            // that is shut — and would try again, failing, on every launch.
            app.settings.wake_word = app.assistant.standby;
            save_settings(app);
            task
        }
        Message::SetAssistantName(name) => {
            app.settings.assistant_name = name;
            // ponytail: the tray item keeps the name it was built with until the
            // app restarts — the menu is created once in `boot` and only the
            // status line is held for updating.
            assistant::set_identity(&app.settings.assistant_name, &app.settings.wake_names);
            save_settings(app);
            Task::none()
        }
        Message::SetWakeNames(spellings) => {
            app.settings.wake_names = spellings;
            assistant::set_identity(&app.settings.assistant_name, &app.settings.wake_names);
            save_settings(app);
            Task::none()
        }
        Message::SetVoiceName(id) => {
            app.settings.voice_name = id;
            // Takes effect on the next sentence synthesized; whatever is already
            // queued keeps the old voice rather than switching mid-answer.
            assistant::set_voice(&app.settings.voice_name);
            save_settings(app);
            Task::none()
        }
        Message::SetVoiceRate(rate) => {
            app.settings.voice_rate = rate;
            // Takes effect on the next sentence synthesized, not the next
            // restart — which is how you can hear what you just picked.
            app.assistant.voice_rate = rate;
            save_settings(app);
            Task::none()
        }
        Message::PickLocalModel => Task::future(async {
            rfd::AsyncFileDialog::new()
                .set_title("Pick a GGUF model")
                .add_filter("GGUF model", &["gguf"])
                .pick_file()
                .await
                .map(|h| h.path().display().to_string())
        })
        .map(Message::SetLocalModel),
        Message::SetLocalModel(None) => Task::none(),
        Message::SetLocalModel(Some(path)) => {
            app.settings.local_model_path = path;
            save_settings(app);
            Task::none()
        }
        Message::SetLocalCtx(raw) => {
            app.local_ctx_input = ctx_digits(&raw);
            if let Ok(n) = app.local_ctx_input.parse::<u32>() {
                if n > 0 {
                    app.settings.local_n_ctx = n;
                    save_settings(app);
                }
            }
            Task::none()
        }
        Message::SetLocalServerPort(raw) => {
            // Five digits caps it at the port space; an empty box is "off",
            // which is what 0 means in the settings file.
            app.local_server_port_input = ctx_digits(&raw).chars().take(5).collect();
            app.settings.local_server_port = app.local_server_port_input.parse().unwrap_or(0);
            save_settings(app);
            Task::none()
        }
        Message::UnloadLocalModel => {
            #[cfg(feature = "local-llm")]
            local_llm::unload();
            Task::none()
        }
        Message::ToggleKeyRevealed => {
            app.key_revealed = !app.key_revealed;
            Task::none()
        }
        Message::Copy(what, text) => {
            app.copied = Some(what);
            iced::clipboard::write(text)
        }
        Message::RestartServer => {
            // Re-check ownership: the conflict may have cleared (or appeared).
            let owner = shell::port_owner(app.shell.port, &app.shell.key);
            app.shell.attached = owner == shell::PortOwner::Ours;
            app.port_conflict = owner == shell::PortOwner::Foreign;
            if app.port_conflict {
                app.shell
                    .log_line(format!("[shell] port {} still owned by another server", app.shell.port));
            } else if !app.shell.attached {
                app.shell.log_line("[shell] restarting the server");
                app.shell.start_server();
            }
            app.child_alive = app.shell.server_running();
            Task::none()
        }
        Message::RestartApp => {
            // Stop our sidecar before launching the replacement, so it does not
            // attach to a server that dies with us; then exit like Quit does.
            if !app.shell.attached {
                app.shell.stop_server();
            }
            // Drop our claim first: the replacement must not spend its startup
            // killing a process that is exiting on the next line anyway.
            let _ = std::fs::remove_file(shell::pid_file(&app.shell.data_dir));
            if let Err(e) = shell::spawn_replacement() {
                app.shell.log_line(format!("[shell] could not relaunch the app: {e}"));
                return Task::none();
            }
            drop(app.tray.take());
            std::process::exit(0)
        }
        Message::RevealPath(path) => {
            shell::reveal_path(&path);
            Task::none()
        }
        Message::CheckForUpdate => {
            if app.update_check.checking {
                return Task::none();
            }
            app.update_check.checking = true;
            app.update_check.error = None;
            // `newer_release` is a blocking call; off the UI thread it goes, or
            // a GitHub that is merely slow freezes the window for ten seconds.
            Task::perform(
                async { tokio::task::spawn_blocking(update_check::newer_release).await },
                |joined| {
                    Message::UpdateChecked(joined.unwrap_or_else(|e| Err(e.to_string())))
                },
            )
        }
        Message::UpdateChecked(result) => {
            app.update_check.checking = false;
            app.update_check.checked = true;
            match result {
                Ok(newer) => app.update_check.newer = newer,
                Err(message) => app.update_check.error = Some(message),
            }
            Task::none()
        }
        Message::Quit => quit(app),
        Message::Processes(processes::Message::TraceLogs(id))
        | Message::Processes(processes::Message::Chat(chat::Message::TraceLogs(id))) => {
            trace_logs_task(app, id)
        }
        Message::Processes(msg) => {
            // Embedded chats have no picker of their own, so they run on the
            // app-wide pair — read here rather than copied at boot, so a change
            // in Chat reaches the next thread opened on any screen.
            app.processes.chat_default =
                (app.settings.chat_provider.clone(), app.settings.chat_model.clone());
            processes::update(&mut app.processes, &app.client, msg).map(Message::Processes)
        }
        Message::Library(library::Message::TraceLogs(id)) => trace_logs_task(app, id),
        Message::Library(msg) => {
            library::update(&mut app.library, &app.client, msg).map(Message::Library)
        }
        Message::ModelOps(modelops::Message::TraceLogs(id)) => trace_logs_task(app, id),
        Message::ModelOps(msg) => {
            modelops::update(&mut app.modelops, &app.client, msg).map(Message::ModelOps)
        }
        // The assistant takes two memory hooks: recall refreshed before every
        // message (so an edit in the dashboard lands on the next turn, not the
        // next restart) and one harvest when a reply completes.
        Message::Assistant(assistant::Message::TraceLogs(id)) => trace_logs_task(app, id),
        Message::Assistant(msg) => {
            let closed = matches!(msg, assistant::Message::Chunk(ChatChunk::Done));
            // Autosave at the moments the thread actually changed shape: the
            // user's turn going in, and the reply closing (or dying).
            let save = closed
                || matches!(
                    msg,
                    assistant::Message::Send | assistant::Message::Chunk(ChatChunk::Failed(_))
                );
            if matches!(msg, assistant::Message::Clear) {
                app.history.close(assistant::NAME);
            }
            // The turn streams on its own task and does not care which screen is
            // open, so the user is free to walk away from it — and gets told
            // when it lands. `sending` going false is the whole turn ending, not
            // a `Done` between tool rounds, which keeps it.
            let was_sending = app.assistant.sending;
            let aborted = matches!(msg, assistant::Message::Abort);
            app.assistant.memory = app.memory.system_block();
            let turn = assistant::update(&mut app.assistant, &app.client, &mut app.memory, msg)
                .map(Message::Assistant);
            if save {
                app.history.autosave(
                    assistant::NAME,
                    &app.assistant.messages,
                    &app.assistant.reasoning,
                    &app.assistant.provider,
                    &app.assistant.model,
                );
            }
            // Stopping it yourself is not news.
            if was_sending && !app.assistant.sending && !aborted {
                notify::away(
                    "assistant",
                    assistant::NAME,
                    &preview(
                        app.assistant.error.as_deref(),
                        app.assistant.messages.last().map_or("", |m| m.content.as_str()),
                    ),
                );
            }
            persist_chat_model(app);
            // `open_screen` parks its answer here rather than navigating from
            // inside the assistant, which has no reach into `App`. Routed
            // through `Message::Nav` like a sidebar click, so the screen it
            // lands on gets its own refresh.
            let go = app.assistant.nav.take().map(|s| Task::done(Message::Nav(s)));
            let mut tasks = vec![turn];
            tasks.extend(go);
            if closed {
                tasks.push(
                    app.memory
                        .harvest(&app.client, &app.assistant.messages, assistant::NAME)
                        .map(Message::Memory),
                );
            }
            Task::batch(tasks)
        }
        Message::Memory(msg) => memory::update(&mut app.memory, msg).map(Message::Memory),
        Message::History(msg) => {
            let source = assistant::NAME;
            // Swapping the thread out from under a streaming reply would append
            // the rest of it to the wrong conversation.
            if app.assistant.sending {
                return Task::none();
            }
            let load = |app: &mut App, messages: Vec<_>, reasoning: Vec<_>| {
                app.assistant.load_thread(messages, reasoning);
            };
            match msg {
                history::Message::New => {
                    app.history.close(source);
                    load(app, Vec::new(), Vec::new());
                }
                history::Message::Select(id) => {
                    if app.history.current(source) == Some(id) {
                        return Task::none();
                    }
                    if let Some(c) = app.history.open(source, id) {
                        // A thread answers on the pair it was answered on. Only
                        // for conversations that recorded one — the ones saved
                        // before that keep whatever is selected now.
                        if let (Some(p), Some(m)) = (c.provider, c.model) {
                            app.assistant.provider = p;
                            app.assistant.model = m;
                            persist_chat_model(app);
                        }
                        load(app, c.messages, c.reasoning);
                    }
                }
                history::Message::Delete(id) => {
                    let was_open = app.history.current(source) == Some(id);
                    app.history.delete(id);
                    if was_open {
                        load(app, Vec::new(), Vec::new());
                    }
                }
            }
            Task::none()
        }
        Message::Providers(providers::Message::TraceLogs(id)) => trace_logs_task(app, id),
        Message::Providers(msg) => {
            providers::update(&mut app.providers, &app.client, msg).map(Message::Providers)
        }
        Message::Todos(todos::Message::TraceLogs(id)) => trace_logs_task(app, id),
        Message::Todos(msg) => todos::update(&mut app.todos, &app.client, msg).map(Message::Todos),
        Message::Agenda(agenda::Message::TraceLogs(id))
        | Message::Agenda(agenda::Message::Chat(agenda_chat::Message::TraceLogs(id))) => {
            trace_logs_task(app, id)
        }
        Message::Agenda(msg) => {
            agenda::update(&mut app.agenda, &app.client, msg).map(Message::Agenda)
        }
        Message::ApiDocs(apidocs::Message::TraceLogs(id)) => trace_logs_task(app, id),
        Message::ApiDocs(msg) => {
            apidocs::update(&mut app.apidocs, &app.client, msg).map(Message::ApiDocs)
        }
        Message::Coder(coder::Message::TraceLogs(id)) => trace_logs_task(app, id),
        Message::Coder(msg) => {
            // The folder and the model outlive the session, so they are settings
            // rather than screen state; everything else the Coder screen holds
            // is not.
            let persist = matches!(
                msg,
                coder::Message::RootPicked(Some(_))
                    | coder::Message::ProviderChanged(_)
                    | coder::Message::ModelChanged(_)
                    | coder::Message::TogglePlan(_)
            );
            let was_sending = app.coder.sending;
            let task = coder::update(&mut app.coder, &app.client, msg).map(Message::Coder);
            // Same as the assistant: the turn outlives the visit to the screen.
            // An approval pause counts as finishing — that one is *waiting* on
            // the user, so it is the toast that matters most.
            if was_sending && !app.coder.sending {
                match &app.coder.pending {
                    Some(p) => notify::review(
                        "coder",
                        "Coder",
                        &format!("Waiting for approval: {}", p.command),
                    ),
                    None => notify::away(
                        "coder",
                        "Coder",
                        &preview(app.coder.error.as_deref(), app.coder.last_reply()),
                    ),
                }
            }
            if persist {
                app.settings.coder_workspace =
                    app.coder.root.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
                app.settings.coder_provider = app.coder.provider.clone();
                app.settings.coder_model = app.coder.model.clone();
                app.settings.coder_plan = app.coder.plan;
                save_settings(app);
            }
            task
        }
        Message::Workflows(workflows::Message::TraceLogs(id)) => trace_logs_task(app, id),
        Message::Workflows(msg) => {
            workflows::update(&mut app.workflows, &app.client, msg).map(Message::Workflows)
        }
        Message::Search(search::Message::TraceLogs(id)) => trace_logs_task(app, id),
        Message::Search(msg) => search::update(&mut app.search, &app.client, msg).map(Message::Search),
    }
}

fn trim_log_view(logs: &mut LogsState) {
    const MAX: usize = 8000;
    if logs.lines.len() > MAX {
        let cut = logs.lines.len() - MAX;
        logs.lines.drain(..cut);
        logs.base += cut as u64;
        // A selected line that scrolled out of the buffer cannot be copied, so
        // it must not keep counting toward "3 selected".
        let base = logs.base;
        logs.selected.retain(|id| *id >= base);
    }
}

impl LogsState {
    /// The one place that decides whether a line is on screen, so "Copy shown",
    /// "Select all" and the rendered tail cannot drift apart.
    ///
    /// ponytail: the level filter parses each line per frame, as `log_entry`
    /// already does for the rendered tail. Parse at ingest if frame time shows.
    pub fn shows(&self, line: &str) -> bool {
        if !self.filter.is_empty()
            && !line.to_lowercase().contains(&self.filter.to_lowercase())
        {
            return false;
        }
        match self.level {
            None => true,
            Some(min) => logs::parse(line).severity().is_some_and(|l| l >= min),
        }
    }

    /// True while anything is hiding lines — what the "N of M" badge is for.
    pub fn filtering(&self) -> bool {
        !self.filter.is_empty() || self.level.is_some()
    }
}

/// Absolute ids of the lines the filter currently shows, oldest first.
fn visible_log_ids(logs: &LogsState) -> impl Iterator<Item = u64> + '_ {
    logs.lines
        .iter()
        .enumerate()
        .filter_map(move |(i, l)| logs.shows(l).then(|| logs.base + i as u64))
}

/// What Copy puts on the clipboard: the selection if there is one, otherwise
/// every line the filter shows.
fn copy_text(logs: &LogsState) -> String {
    let pick: Vec<&String> = logs
        .lines
        .iter()
        .enumerate()
        .filter(|(i, l)| {
            let id = logs.base + *i as u64;
            if logs.selected.is_empty() {
                logs.shows(l)
            } else {
                logs.selected.contains(&id)
            }
        })
        .map(|(_, l)| l)
        .collect();
    pick.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n")
}

fn view(app: &App, _window: window::Id) -> Element<'_, Message> {
    screen::view(app)
}

fn subscription(app: &App) -> Subscription<Message> {
    let mut subs = vec![
        window::close_events().map(Message::WindowClosed),
        window::close_requests().map(Message::WindowCloseRequested),
        // Whether the user is actually in front of the app decides whether
        // finished work gets a toast — see [`watching_key`].
        window::events().filter_map(|(_, event)| match event {
            window::Event::Focused => Some(Message::WindowFocus(true)),
            window::Event::Unfocused => Some(Message::WindowFocus(false)),
            _ => None,
        }),
        // Tray menu events. `muda`'s receiver is a sync crossbeam channel with no
        // async side, so this used to `try_recv` on a 150 ms timer — 6.7 wakeups
        // a second, forever, including while the window is hidden and the app is
        // only a server host. It blocks on the receiver instead: one parked
        // thread out of tokio's blocking pool, and the app sleeps until the user
        // actually picks something out of the tray.
        Subscription::run(|| {
            iced::stream::channel(16, async |mut out| {
                loop {
                    let recv =
                        tokio::task::spawn_blocking(|| MenuEvent::receiver().recv()).await;
                    // The sender is a `muda` static and never drops, so an error
                    // here means the runtime is going down — leave, rather than
                    // spin on a channel that will not produce again.
                    let Ok(Ok(ev)) = recv else { return };
                    let _ = futures::SinkExt::send(&mut out, Message::Tray(ev.id.0)).await;
                }
            })
        }),
        // The health poll doubles as the readiness listener, so it runs fast
        // while the server is coming up (a cold start is the one moment the user
        // is watching the UI wait) and backs off once it is answering. Changing
        // the duration changes the subscription's identity, so iced swaps the
        // timer for us.
        iced::time::every(match app.server_state() {
            ServerState::Starting | ServerState::Unreachable => {
                std::time::Duration::from_millis(750)
            }
            ServerState::Ready | ServerState::Conflict => std::time::Duration::from_secs(5),
        })
        .map(|_| Message::StatusTick),
    ];
    // A blocked view has nothing to refresh: every poll below would be a request
    // that can only fail. Polling also only runs while the window is open — a
    // hidden app is a server host, not a UI.
    let live = app.window.is_some() && app.view_available();
    let tab = |t: SettingsTab| app.screen == Screen::Settings && app.settings_tab == t;

    if live && app.screen == Screen::Logs && !app.logs.paused {
        subs.push(iced::time::every(std::time::Duration::from_secs(1)).map(|_| Message::LogsTick));
    }
    if live && app.screen == Screen::Processes {
        subs.push(
            iced::time::every(std::time::Duration::from_secs(3))
                .map(|_| Message::Processes(processes::Message::ListTick)),
        );
    }
    // The open shell's PTY. Not gated on the screen or the window: the terminal
    // is a live process the user started, and a `cargo build` must keep printing
    // into it while they read the transcript — or while they are on another
    // screen entirely. It ends when they close the drawer, which is what drops
    // the session.
    // The preview's child window is positioned in window coordinates, so a
    // resize moves the hole out from under it until it is told. Only while it
    // is actually on screen: everywhere else it is hidden anyway.
    if app.screen == Screen::Coder && app.coder.browser_open {
        subs.push(
            window::resize_events()
                .map(|_| Message::Coder(coder::Message::BrowserSync)),
        );
    }
    if let Some(session) = app.coder.term.as_ref() {
        subs.push(session.0.subscription().map(|e| Message::Coder(coder::Message::Term(e))));
    }
    // The Coder screen's clock. A turn in flight, or one parked on the approval
    // gate — both are waits the user is sitting through. Otherwise nothing is
    // counting, and a permanent 1s timer would wake the app for no reason.
    if app.coder.sending || app.coder.pending.is_some() {
        subs.push(
            iced::time::every(std::time::Duration::from_secs(1))
                .map(|_| Message::Coder(coder::Message::Tick)),
        );
    }
    // The Coder screen's spinner. Same idea as the clock above but faster and
    // wider: it also runs for a sidebar fetch, which has no seconds worth
    // counting but still needs to say "in progress" as something other than a
    // blank list.
    if app.coder.sending
        || app.coder.pending.is_some()
        || app.coder.threads_loading
        || app.coder.checkpoints_loading
    {
        subs.push(
            iced::time::every(std::time::Duration::from_millis(90))
                .map(|_| Message::Coder(coder::Message::AnimTick)),
        );
    }
    // A run the user walked away from keeps its poll and its stream: leaving the
    // page does not stop the run, and this poll is what notices it go terminal
    // and fires the completion toast. The list above is display only, so it
    // stops with the page.
    let watching_run = live && app.screen == Screen::Processes;
    if watching_run || app.processes.is_live() {
        if app.processes.selected.is_some() {
            subs.push(
                iced::time::every(app.processes.detail_poll_interval())
                    .map(|_| Message::Processes(processes::Message::DetailTick)),
            );
        }
        if let Some(id) = app.processes.selected.filter(|_| app.processes.stream_eligible()) {
            // `run_with` takes a plain fn pointer, so the client is rebuilt from
            // hashable data; the tuple is also the subscription identity, so
            // selecting another run tears the old stream down.
            let data = (id, app.client.base().to_string(), app.client.key().to_string());
            subs.push(Subscription::run_with(data, |(id, base, key)| {
                use futures::StreamExt;
                // Any frame means "state changed"; the payload is not needed
                // because the detail fetch is the source of truth.
                let client = Client::new(base.clone(), key.clone());
                agent_platform_client::sse::process_stream(client, *id)
                    .map(|_| Message::Processes(processes::Message::StreamFrame))
            }));
        }
    }
    // The Dashboard embeds E.V.'s live HUD, so it needs the same heartbeat. On
    // the assistant screen the tick is the HUD, the mic gate and the speech
    // queue — all three are voice mode, so text mode runs at 0 fps.
    // Settings → Appearance previews the picked animation, so it needs the beat
    // too — otherwise you choose between two still frames.
    let hud_live = (app.screen == Screen::Assistant && app.assistant.voice)
        || app.screen == Screen::Dashboard
        || tab(SettingsTab::Appearance);
    if live && hud_live {
        // Frames, not a timer: this fires once per drawn frame at the display's
        // own rate, so the animation lands on the compositor's schedule instead
        // of fighting it. A `time::every(16ms)` on Windows is quantised to the
        // ~15.6 ms system tick, which is what made a 60 fps animation stutter.
        // The analyzer inside still steps at a fixed 60 Hz.
        subs.push(
            iced::window::frames().map(|at| Message::Assistant(assistant::Message::Tick(at))),
        );
    } else if app.assistant.busy() {
        // Walking away does not stop the reply. The tokens arrive on their own
        // task either way, but the speech queue and the audio sink are drained
        // from this beat — so with no canvas to drive it (another screen, or no
        // window at all) it comes off a timer instead, and E.V. finishes the
        // sentence it was in the middle of. `frames()` needs something drawing;
        // this does not, and there is no animation left to stutter.
        subs.push(
            iced::time::every(std::time::Duration::from_millis(16))
                .map(|at| Message::Assistant(assistant::Message::Tick(at))),
        );
    }
    // Build jobs run on the server, so the poll that reports one finished has to
    // outlive the visit to the tab that started it — same reason as the run
    // above. `job_running` is false unless a job is actually in flight.
    if app.modelops.job_running() {
        subs.push(
            iced::time::every(app.modelops.poll_interval())
                .map(|_| Message::ModelOps(modelops::Message::JobTick)),
        );
    }
    // A toast clears itself. Keyed on the text, so a new message restarts the
    // countdown instead of inheriting the old one's remaining time.
    if let Some(keyed) = screen::notice(app) {
        subs.push(Subscription::run_with(keyed, |_| {
            futures::stream::once(async {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                Message::NoticeExpired
            })
        }));
    }
    // Ctrl+K summons E.V. from any screen, and Esc dismisses the in-app modals
    // as the OS dialogs they replaced did. One listener for both: the closure
    // has to stay non-capturing, so update() decides what Esc meant.
    // Two listeners, not one with a flag in it: these closures must stay
    // non-capturing (iced checks at compile time), so the condition lives out
    // here in whether the subscription exists at all.
    subs.push(iced::keyboard::listen().filter_map(|event| {
        matches!(
            event,
            iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Character(ref c),
                modifiers,
                ..
            } if c.as_str() == "k" && modifiers.command()
        )
        .then_some(Message::ToggleAssistant)
    }));
    if app.close_prompt.is_some() || app.library.confirm.is_some() || app.assistant_open {
        subs.push(iced::keyboard::listen().filter_map(|event| {
            matches!(
                event,
                iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                    ..
                }
            )
            .then_some(Message::EscapePressed)
        }));
    }
    Subscription::batch(subs)
}

fn main() -> iced::Result {
    // whisper.cpp/GGML log straight to stderr from C; the `set_print_*` params
    // only cover segment output. No log backend feature is on, so this drops them.
    whisper_rs::install_logging_hooks();
    iced::daemon(boot, update, view)
        .title(|_state: &App, _w| "Agent Platform".to_string())
        // System mode re-resolves on every render, so an OS light/dark switch
        // is picked up without a restart.
        .theme(|state: &App, _w| state.settings.theme.resolve())
        .subscription(subscription)
        // lucide glyphs (`ui::Icon`) render as text, so the font has to be loaded.
        .font(ui::icon::FONT_BYTES)
        .run()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The context box writes straight into a `u32` setting, so what it refuses
    /// is the whole validation there is.
    #[test]
    fn the_context_box_takes_digits_and_nothing_else() {
        assert_eq!(ctx_digits("8192"), "8192");
        assert_eq!(ctx_digits("8k tokens"), "8");
        assert_eq!(ctx_digits("-1"), "1");
        // Clearing the field is allowed; it just does not parse, so the stored
        // value stands until a new number is typed.
        assert_eq!(ctx_digits(""), "");
        assert!("".parse::<u32>().is_err());
        // Longer than any real context, and short of overflowing the u32.
        assert_eq!(ctx_digits("123456789").len(), 7);
    }

    /// A silent `None` here means an unbranded title bar and no tray icon, and
    /// neither failure is visible in a build log.
    #[test]
    fn the_app_icon_decodes_out_of_the_ico() {
        for px in [32, 48] {
            let (rgba, w, h) = frame_rgba(px).unwrap_or_else(|| panic!("no {px}px RGBA frame"));
            assert_eq!((w, h), (px as u32, px as u32));
            assert_eq!(rgba.len(), (px as usize).pow(2) * 4);
            assert!(rgba.chunks(4).any(|p| p[3] > 0), "every pixel is transparent");
        }
    }

    /// The recolor has to hit the plate and only the plate: a rule that also
    /// caught the robot would flip the mark's own color with the theme.
    #[test]
    fn the_light_plate_inverts_the_backdrop_but_not_the_robot() {
        let (dark, ..) = logo_rgba(32, false).unwrap();
        let (light, ..) = logo_rgba(32, true).unwrap();
        let count = |rgba: &[u8], want: [u8; 3]| {
            rgba.chunks_exact(4).filter(|p| p[3] == 255 && p[..3] == want).count()
        };
        // (20, 20, 20) is the plate, (255, 78, 62) the robot's body.
        let plate = count(&dark, [20, 20, 20]);
        assert!(plate > 100, "asset changed: no dark plate to recolor");
        assert_eq!(count(&light, [235, 235, 235]), plate, "plate was not inverted");
        assert_eq!(count(&light, [255, 78, 62]), count(&dark, [255, 78, 62]), "robot changed");
    }

    /// The whole notification rule: a toast fires unless the user is looking
    /// straight at the thing that finished. Getting this backwards means either
    /// no toast at all or one for work that is already on screen.
    #[test]
    fn only_the_screen_in_front_of_the_user_suppresses_its_own_toast() {
        let tab = SettingsTab::Providers;
        assert_eq!(watching_key(Some((Screen::Coder, tab))), "coder");
        assert_eq!(watching_key(Some((Screen::Assistant, tab))), "assistant");
        // Same page, other tab: the conversation is not on screen.
        assert_eq!(watching_key(Some((Screen::Memory, tab))), "");
        // Model ops is a tab, so the screen alone is not enough.
        assert_eq!(watching_key(Some((Screen::Settings, tab))), "");
        assert_eq!(watching_key(Some((Screen::Settings, SettingsTab::ModelOps))), "modelops");
        // Hidden, or behind another app: nothing is being watched, so nothing
        // may match — including the screen that is technically still selected.
        assert_eq!(watching_key(None), "");
        for screen in [Screen::Processes, Screen::Coder, Screen::Assistant, Screen::Workflows] {
            assert_ne!(watching_key(Some((screen, tab))), watching_key(None));
        }
    }

    /// The toast body is one line of someone else's markdown, so it has to
    /// survive an empty reply and a multi-byte cut.
    #[test]
    fn a_toast_says_one_line_and_prefers_the_error() {
        assert_eq!(preview(None, "Done.\n\nDetails below."), "Done.");
        assert_eq!(preview(None, "\n\n  indented  \n"), "indented");
        assert_eq!(preview(Some("stream failed"), "half an answer"), "stream failed");
        assert_eq!(preview(None, ""), "Finished.");
        let long = "é".repeat(300);
        assert!(preview(None, &long).ends_with('…'), "a long line is cut");
    }

    /// Selection survives the buffer wrapping: ids are absolute, so trimming
    /// the front must not hand a selection to a different line.
    #[test]
    fn copying_follows_the_selection_not_the_row_position() {
        let mut logs = LogsState {
            lines: (0..3).map(|i| format!("line {i}")).collect(),
            ring_cursor: 0,
            api_cursor: 0,
            filter: String::new(),
            level: None,
            paused: false,
            dropped: 0,
            base: 0,
            selected: [1, 2].into_iter().collect(),
        };
        assert_eq!(copy_text(&logs), "line 1
line 2");

        // The buffer wraps by one: ids stay put, the dropped selection goes.
        logs.lines.remove(0);
        logs.base += 1;
        logs.selected.retain(|id| *id >= logs.base);
        assert_eq!(copy_text(&logs), "line 1
line 2");
        logs.selected.clear();
        logs.filter = "line 2".into();
        assert_eq!(copy_text(&logs), "line 2", "no selection copies what the filter shows");
        assert_eq!(visible_log_ids(&logs).collect::<Vec<_>>(), vec![2]);
    }

    /// The level filter is "this and above", and it must not silently keep the
    /// lines that carry no level — those are the bulk of the buffer.
    #[test]
    fn the_level_filter_keeps_that_level_and_worse() {
        let mut logs = LogsState {
            lines: Vec::new(),
            ring_cursor: 0,
            api_cursor: 0,
            filter: String::new(),
            level: None,
            paused: false,
            dropped: 0,
            base: 0,
            selected: Default::default(),
        };
        let err = r#"{"level": "ERROR", "message": "boom", "trace_id": "abc"}"#;
        let info = r#"{"level": "INFO", "message": "request completed"}"#;
        let bare = "Application startup complete.";

        assert!(logs.shows(err) && logs.shows(info) && logs.shows(bare), "unfiltered shows all");
        assert!(!logs.filtering());

        logs.level = Some(logs::Level::Warn);
        assert!(logs.shows(err), "ERROR is above WARN");
        assert!(!logs.shows(info));
        assert!(!logs.shows(bare), "a line with no level is not a warning");
        assert!(logs.filtering());

        // The line the whole feature exists for: a failed request is logged at
        // INFO. If the level filter read `level` instead of `severity` it would
        // hide a row the screen paints red, which is worse than no filter.
        let five_hundred =
            r#"{"level": "INFO", "message": "request completed", "status_code": 500}"#;
        logs.level = Some(logs::Level::Error);
        assert!(logs.shows(five_hundred), "\"Errors\" must not hide a 5xx");
        assert!(!logs.shows(info));

        // Both halves apply: the text filter still narrows within the level.
        logs.filter = "abc".into();
        assert!(logs.shows(err));
        logs.filter = "nothing".into();
        assert!(!logs.shows(err));
    }

    /// The guard is only useful if it leaves a way out. Settings must open with
    /// no server, and at least one of its tabs must work there — that is where
    /// the user finds out what went wrong.
    #[test]
    fn diagnostics_stay_reachable_without_a_server() {
        assert!(!Screen::Settings.needs_server());
        assert!(!Screen::Logs.needs_server(), "logs must open against a dead API");
        assert!(!Screen::Dashboard.needs_server(), "the landing page must open against a dead API");
        for screen in [
            Screen::Processes,
            Screen::Projects,
            Screen::Teams,
            Screen::Workflows,
            Screen::Plans,
            Screen::Agenda,
            // The Coder screen's tools run here, but the agent loop it answers
            // to is the server's — a dead API means no turn to answer.
            Screen::Coder,
            Screen::Assistant,
        ]
        {
            assert!(screen.needs_server(), "{screen:?} would render against a dead API");
        }

        let usable: Vec<_> =
            SettingsTab::ALL.iter().filter(|t| !t.needs_server()).map(|t| t.label()).collect();
        assert_eq!(usable, vec!["Appearance", "Status", "API"]);
    }
}
