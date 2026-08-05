//! Native desktop app: iced daemon owning the Python server sidecar.
//!
//! Ollama-style background behavior: the window's close button asks whether to
//! quit or minimize to tray; tray keeps the daemon running with zero windows
//! (server keeps serving on its fixed port); the tray offers Show / Restart /
//! Quit. Quit kills the child we spawned and
//! hard-exits — `iced::exit()` hangs on Windows wgpu teardown (verified in the
//! Phase 0 spike), and the tray icon must be dropped first or it lingers.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assistant;
mod assistant_view;
mod chat;
mod chat_view;
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
mod shell;
mod stt;
mod ui;
mod todos;
mod todos_view;
mod workflows;
mod workflows_view;

use agent_platform_client::sse::ChatChunk;
use agent_platform_client::types::SystemStatus;
use agent_platform_client::Client;
use iced::{window, Element, Subscription, Task};
use shell::{Settings, Shell, ThemeMode};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
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
    Assistant,
    Memory,
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
        !matches!(self, Screen::Settings | Screen::Memory | Screen::Dashboard)
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
    Status,
    Logs,
}

impl SettingsTab {
    pub const ALL: [SettingsTab; 4] =
        [SettingsTab::Providers, SettingsTab::ModelOps, SettingsTab::Status, SettingsTab::Logs];

    pub fn label(self) -> &'static str {
        match self {
            SettingsTab::Providers => "Providers",
            SettingsTab::ModelOps => "Model ops",
            SettingsTab::Status => "Status",
            SettingsTab::Logs => "Logs",
        }
    }

    /// Gating is per tab, not per page: Status and Logs are exactly the tabs a
    /// user needs while the server is not answering.
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
    pub paused: bool,
    pub dropped: u64,
}

pub struct App {
    pub shell: Shell,
    pub settings: Settings,
    pub client: Client,
    pub window: Option<window::Id>,
    /// Set while the in-app quit-or-tray prompt is up; holds the window whose
    /// close was intercepted so "Minimize to tray" knows what to hide.
    pub close_prompt: Option<window::Id>,
    tray: Option<TrayIcon>,
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
    WindowCloseRequested(window::Id),
    CloseConfirmed,
    MinimizeToTray,
    CloseCancelled,
    /// Esc while a modal is up; routed to whichever modal is open.
    EscapePressed,
    /// A toast's time is up (or its close button was pressed).
    NoticeExpired,
    WindowClosed(window::Id),
    Nav(Screen),
    NavSettings(SettingsTab),
    StatusTick,
    StatusFetched(Result<SystemStatus, String>),
    LogsTick,
    ApiLogs(Result<(Vec<String>, i64, i64), String>),
    LogFilterChanged(String),
    ToggleLogsPaused,
    ClearLogs,
    ToggleKeyRevealed,
    SetTheme(ThemeMode),
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

fn build_tray(port: u16) -> Option<TrayIcon> {
    let menu = Menu::new();
    let items = [
        MenuItem::with_id("show", "Show Agent Platform", true, None),
        MenuItem::with_id("server", &format!("Server: 127.0.0.1:{port}"), false, None),
        MenuItem::with_id("restart", "Restart server", true, None),
        MenuItem::with_id("quit", "Quit", true, None),
    ];
    for item in &items {
        menu.append(item).ok()?;
    }
    TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Agent Platform")
        // The notification area follows the system theme, so a dark system gets
        // the light plate.
        .with_icon(tray_icon_image(shell::system_is_dark())?)
        .build()
        .ok()
}

fn boot() -> (App, Task<Message>) {
    let app_dir = shell::app_dir();
    let settings = Settings::load(&app_dir);
    let port = std::env::var("AGENT_PLATFORM_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(settings.port);

    let log = std::sync::Arc::new(std::sync::Mutex::new(shell::LogRing::new()));
    let (python, script) = shell::resolve_server().unwrap_or_else(|| {
        log.lock().unwrap().push(
            "[shell] no bundled server payload and no repo checkout found".to_string(),
        );
        (Default::default(), Default::default())
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
        python,
        script,
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
    let tray = build_tray(port);
    if tray.is_none() {
        sh.log_line("[shell] tray unavailable".to_string());
    }

    let minimized =
        settings.start_minimized || std::env::args().any(|a| a == "--minimized");
    let (chat_provider, chat_model) =
        (settings.chat_provider.clone(), settings.chat_model.clone());
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
        tray,
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
            paused: false,
            dropped: 0,
        },
        processes: processes::State::default(),
        library: library::State::default(),
        modelops: modelops::State::default(),
        assistant: assistant::State::with_defaults(chat_provider, chat_model),
        memory: memory::Store::load(&app_dir),
        history: history::Store::load(&app_dir),
        providers: providers::State::default(),
        workflows: workflows::State::default(),
        todos: todos::State::default(),
    };
    let task = if minimized { Task::none() } else { open_window() };
    let bootstrap = Task::batch([
        Task::done(Message::StatusTick),
        processes::load_lists(&app.client).map(Message::Processes),
        Task::done(Message::Processes(processes::Message::ListTick)),
    ]);
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

fn quit(app: &mut App) -> ! {
    if !app.shell.attached {
        app.shell.stop_server();
    }
    drop(app.tray.take()); // remove the tray icon before the hard exit
    std::process::exit(0)
}

/// The one fetch the current view needs on entry. Skipped entirely while the
/// server is not ready, so a blocked view never fires a request that can only
/// fail — [`Message::StatusFetched`] replays it the moment the server answers.
fn enter_screen(app: &App) -> Task<Message> {
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
        // The dropdowns need the provider catalog once; chat itself works
        // without it, so a failed load costs nothing but empty pickers.
        Screen::Assistant if app.assistant.catalog.is_empty() => {
            assistant::load_catalog(&app.client).map(Message::Assistant)
        }
        Screen::Assistant | Screen::Memory => Task::none(),
        Screen::Settings => match app.settings_tab {
            SettingsTab::Logs => Task::done(Message::LogsTick),
            SettingsTab::ModelOps => Task::done(Message::ModelOps(modelops::Message::Refresh)),
            SettingsTab::Providers => Task::done(Message::Providers(providers::Message::Refresh)),
            SettingsTab::Status => Task::none(),
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

fn update(app: &mut App, message: Message) -> Task<Message> {
    match message {
        Message::Tray(id) => match id.as_str() {
            "show" => {
                if app.window.is_none() {
                    open_window()
                } else {
                    Task::none()
                }
            }
            "restart" => update(app, Message::RestartServer),
            "quit" => quit(app),
            _ => Task::none(),
        },
        Message::WindowOpened(id) => {
            app.window = Some(id);
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
        Message::EscapePressed => {
            if app.close_prompt.is_some() {
                app.close_prompt = None;
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
                trim_log_view(&mut app.logs.lines);
                Task::none()
            }
        }
        Message::ApiLogs(result) => {
            if let Ok((lines, next, dropped)) = result {
                app.logs.api_cursor = next;
                app.logs.dropped += dropped.max(0) as u64;
                app.logs.lines.extend(lines);
                trim_log_view(&mut app.logs.lines);
            }
            Task::none()
        }
        Message::LogFilterChanged(f) => {
            app.logs.filter = f;
            Task::none()
        }
        Message::ToggleLogsPaused => {
            app.logs.paused = !app.logs.paused;
            Task::none()
        }
        Message::ClearLogs => {
            app.logs.lines.clear();
            Task::none()
        }
        Message::SetTheme(mode) => {
            app.settings.theme = mode;
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
        Message::Quit => quit(app),
        Message::Processes(msg) => {
            processes::update(&mut app.processes, &app.client, msg).map(Message::Processes)
        }
        Message::Library(msg) => {
            library::update(&mut app.library, &app.client, msg).map(Message::Library)
        }
        Message::ModelOps(msg) => {
            modelops::update(&mut app.modelops, &app.client, msg).map(Message::ModelOps)
        }
        // The assistant takes two memory hooks: recall refreshed before every
        // message (so an edit in the dashboard lands on the next turn, not the
        // next restart) and one harvest when a reply completes.
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
            app.assistant.memory = app.memory.system_block();
            let turn =
                assistant::update(&mut app.assistant, &app.client, msg).map(Message::Assistant);
            if save {
                app.history.autosave(
                    assistant::NAME,
                    &app.assistant.messages,
                    &app.assistant.reasoning,
                );
            }
            // The provider/model override survives restarts: any change lands
            // in settings.json the moment it is made.
            if app.settings.chat_provider != app.assistant.provider
                || app.settings.chat_model != app.assistant.model
            {
                app.settings.chat_provider = app.assistant.provider.clone();
                app.settings.chat_model = app.assistant.model.clone();
                save_settings(app);
            }
            match closed {
                false => turn,
                true => Task::batch([
                    turn,
                    app.memory
                        .harvest(&app.client, &app.assistant.messages, assistant::NAME)
                        .map(Message::Memory),
                ]),
            }
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
        Message::Providers(msg) => {
            providers::update(&mut app.providers, &app.client, msg).map(Message::Providers)
        }
        Message::Todos(msg) => todos::update(&mut app.todos, &app.client, msg).map(Message::Todos),
        Message::Workflows(msg) => {
            workflows::update(&mut app.workflows, &app.client, msg).map(Message::Workflows)
        }
    }
}

fn trim_log_view(lines: &mut Vec<String>) {
    const MAX: usize = 8000;
    if lines.len() > MAX {
        lines.drain(..lines.len() - MAX);
    }
}

fn view(app: &App, _window: window::Id) -> Element<'_, Message> {
    screen::view(app)
}

fn subscription(app: &App) -> Subscription<Message> {
    let mut subs = vec![
        window::close_events().map(Message::WindowClosed),
        window::close_requests().map(Message::WindowCloseRequested),
        // Tray menu events: global receiver, polled.
        Subscription::run(|| {
            iced::stream::channel(16, async |mut out| {
                let rx = MenuEvent::receiver();
                loop {
                    while let Ok(ev) = rx.try_recv() {
                        let _ =
                            futures::SinkExt::send(&mut out, Message::Tray(ev.id.0.clone())).await;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
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

    if live && tab(SettingsTab::Logs) && !app.logs.paused {
        subs.push(iced::time::every(std::time::Duration::from_secs(1)).map(|_| Message::LogsTick));
    }
    if live && app.screen == Screen::Processes {
        subs.push(
            iced::time::every(std::time::Duration::from_secs(3))
                .map(|_| Message::Processes(processes::Message::ListTick)),
        );
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
    let hud_live = (app.screen == Screen::Assistant && app.assistant.voice)
        || app.screen == Screen::Dashboard;
    if live && hud_live {
        subs.push(
            iced::time::every(assistant::TICK)
                .map(|_| Message::Assistant(assistant::Message::Tick)),
        );
    }
    if live && tab(SettingsTab::ModelOps) && app.modelops.job_running() {
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
    // Esc dismisses the in-app modals, as the OS dialogs they replaced did. The
    // filter_map closure has to stay non-capturing, so update() picks the modal.
    if app.close_prompt.is_some() || app.library.confirm.is_some() {
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

    /// The guard is only useful if it leaves a way out. Settings must open with
    /// no server, and at least one of its tabs must work there — that is where
    /// the user finds out what went wrong.
    #[test]
    fn diagnostics_stay_reachable_without_a_server() {
        assert!(!Screen::Settings.needs_server());
        assert!(!Screen::Dashboard.needs_server(), "the landing page must open against a dead API");
        for screen in [
            Screen::Processes,
            Screen::Projects,
            Screen::Teams,
            Screen::Workflows,
            Screen::Plans,
            Screen::Assistant,
        ]
        {
            assert!(screen.needs_server(), "{screen:?} would render against a dead API");
        }

        let usable: Vec<_> =
            SettingsTab::ALL.iter().filter(|t| !t.needs_server()).map(|t| t.label()).collect();
        assert_eq!(usable, vec!["Status", "Logs"]);
    }
}
