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
mod library;
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
    Chat,
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

    /// The assistants share one sidebar entry and one tab strip: the same
    /// conversation surface plain, voiced with a HUD, and what it remembers.
    pub fn is_chat(self) -> bool {
        matches!(self, Screen::Chat | Screen::Assistant | Screen::Memory)
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
    /// A quit-or-tray dialog is already up; further close clicks are ignored.
    close_prompt: bool,
    tray: Option<TrayIcon>,
    pub screen: Screen,
    /// Which tab the Settings page shows; remembered across visits.
    pub settings_tab: SettingsTab,
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
    pub chat: chat::State,
    pub assistant: assistant::State,
    /// What both assistants remember about the user, across restarts.
    pub memory: memory::Store,
    pub providers: providers::State,
    pub workflows: workflows::State,
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
    WindowCloseChoice(window::Id, rfd::MessageDialogResult),
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
    Copy(&'static str, String),
    RestartServer,
    RestartApp,
    RevealPath(String),
    Quit,
    Processes(processes::Message),
    Library(library::Message),
    ModelOps(modelops::Message),
    Chat(chat::Message),
    Assistant(assistant::Message),
    Memory(memory::Message),
    Providers(providers::Message),
    Workflows(workflows::Message),
}

fn open_window() -> Task<Message> {
    let (_id, task) = window::open(window::Settings {
        size: iced::Size::new(1440.0, 900.0),
        min_size: Some(iced::Size::new(820.0, 560.0)),
        // Close is intercepted: we ask quit-or-tray instead of just closing.
        exit_on_close_request: false,
        ..window::Settings::default()
    });
    task.map(Message::WindowOpened)
}

fn tray_icon_image() -> tray_icon::Icon {
    // Solid teal placeholder; real icon lands with packaging (Phase 5).
    let mut rgba = Vec::with_capacity(32 * 32 * 4);
    for _ in 0..(32 * 32) {
        rgba.extend_from_slice(&[0x14, 0xb8, 0xa6, 0xff]);
    }
    tray_icon::Icon::from_rgba(rgba, 32, 32).expect("tray icon")
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
        .with_icon(tray_icon_image())
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

    let app = App {
        shell: sh,
        settings,
        client,
        window: None,
        close_prompt: false,
        tray,
        screen: Screen::Dashboard,
        settings_tab: SettingsTab::Providers,
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
        chat: chat::State::with_defaults(chat_provider, chat_model),
        assistant: assistant::State::new(),
        memory: memory::Store::load(&app_dir),
        providers: providers::State::default(),
        workflows: workflows::State::default(),
    };
    let task = if minimized { Task::none() } else { open_window() };
    let bootstrap = Task::batch([
        Task::done(Message::StatusTick),
        processes::load_lists(&app.client).map(Message::Processes),
        Task::done(Message::Processes(processes::Message::ListTick)),
    ]);
    (app, task.chain(bootstrap))
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
        // The dropdowns need the provider catalog once; chat itself works
        // without it, so a failed load costs nothing but empty pickers.
        Screen::Chat if app.chat.catalog.is_empty() => {
            chat::load_catalog(&app.client).map(Message::Chat)
        }
        Screen::Chat | Screen::Assistant | Screen::Memory => Task::none(),
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
            if app.close_prompt {
                return Task::none();
            }
            app.close_prompt = true;
            let dialog = rfd::AsyncMessageDialog::new()
                .set_title("Agent Platform")
                .set_description("Close the app, or keep it running in the tray?")
                .set_buttons(rfd::MessageButtons::YesNoCancelCustom(
                    "Close".to_string(),
                    "Minimize to tray".to_string(),
                    "Cancel".to_string(),
                ));
            Task::perform(dialog.show(), move |r| Message::WindowCloseChoice(id, r))
        }
        Message::WindowCloseChoice(id, result) => {
            app.close_prompt = false;
            match result {
                rfd::MessageDialogResult::Custom(s) if s == "Close" => quit(app),
                rfd::MessageDialogResult::Custom(s) if s == "Minimize to tray" => {
                    window::close(id)
                }
                // Cancel, Esc, or dialog dismissed: keep the window.
                _ => Task::none(),
            }
        }
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
            if let Err(e) = app.settings.save(&app.shell.data_dir) {
                app.shell.log_line(format!("[shell] could not save settings: {e}"));
            }
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
        // Both assistants take the same two memory hooks: recall refreshed
        // before every message (so an edit in the dashboard lands on the next
        // turn, not the next restart) and one harvest when a reply completes.
        Message::Chat(msg) => {
            let closed = matches!(msg, chat::Message::Chunk(ChatChunk::Done));
            app.chat.system = app.memory.system_block();
            let turn = chat::update(&mut app.chat, &app.client, msg).map(Message::Chat);
            // The provider/model override survives restarts: any change lands
            // in settings.json the moment it is made.
            if app.settings.chat_provider != app.chat.provider
                || app.settings.chat_model != app.chat.model
            {
                app.settings.chat_provider = app.chat.provider.clone();
                app.settings.chat_model = app.chat.model.clone();
                if let Err(e) = app.settings.save(&app.shell.data_dir) {
                    app.shell.log_line(format!("[shell] could not save settings: {e}"));
                }
            }
            match closed {
                false => turn,
                true => Task::batch([
                    turn,
                    app.memory
                        .harvest(&app.client, &app.chat.messages, "Chat")
                        .map(Message::Memory),
                ]),
            }
        }
        Message::Assistant(msg) => {
            let closed = matches!(msg, assistant::Message::Chunk(ChatChunk::Done));
            app.assistant.memory = app.memory.system_block();
            let turn =
                assistant::update(&mut app.assistant, &app.client, msg).map(Message::Assistant);
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
        Message::Providers(msg) => {
            providers::update(&mut app.providers, &app.client, msg).map(Message::Providers)
        }
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
    // The Dashboard embeds E.V.'s live HUD, so it needs the same heartbeat.
    if live && (app.screen == Screen::Assistant || app.screen == Screen::Dashboard) {
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
    Subscription::batch(subs)
}

fn main() -> iced::Result {
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
            Screen::Chat,
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
