//! Native desktop app: iced daemon owning the Python server sidecar.
//!
//! Ollama-style background behavior: closing the window hides the app (daemon
//! keeps running with zero windows, server keeps serving on its fixed port);
//! the tray offers Show / Restart / Quit. Quit kills the child we spawned and
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
mod modelops;
mod modelops_view;
mod notify;
mod processes;
mod processes_view;
mod providers;
mod providers_view;
mod screen;
mod shell;
mod ui;

use agent_platform_client::types::SystemStatus;
use agent_platform_client::Client;
use iced::{window, Element, Subscription, Task};
use shell::{Settings, Shell, ThemeMode};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Processes,
    Projects,
    Teams,
    ModelOps,
    Chat,
    Assistant,
    Providers,
    Status,
    Logs,
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
    tray: Option<TrayIcon>,
    pub screen: Screen,
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
    pub providers: providers::State,
}

#[derive(Debug, Clone)]
pub enum Message {
    Tray(String),
    WindowOpened(window::Id),
    WindowClosed(window::Id),
    Nav(Screen),
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
    Providers(providers::Message),
}

fn open_window() -> Task<Message> {
    let (_id, task) = window::open(window::Settings {
        size: iced::Size::new(1100.0, 760.0),
        min_size: Some(iced::Size::new(820.0, 560.0)),
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

    // Ollama-style: if one of OUR servers is already on the port (a second app
    // instance, or a dev run), attach as a pure client instead of spawning.
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

    let app = App {
        shell: sh,
        settings,
        client,
        window: None,
        tray,
        screen: Screen::Processes,
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
        chat: chat::State::default(),
        assistant: assistant::State::new(),
        providers: providers::State::default(),
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
        Message::WindowClosed(id) => {
            if app.window == Some(id) {
                app.window = None;
            }
            Task::none()
        }
        Message::Nav(screen) => {
            app.screen = screen;
            app.copied = None;
            match screen {
                Screen::Logs => Task::done(Message::LogsTick),
                Screen::Processes => Task::done(Message::Processes(processes::Message::ListTick)),
                Screen::Projects | Screen::Teams => Task::done(Message::Library(library::Message::Refresh)),
                Screen::ModelOps => Task::done(Message::ModelOps(modelops::Message::Refresh)),
                Screen::Providers => Task::done(Message::Providers(providers::Message::Refresh)),
                Screen::Status | Screen::Chat | Screen::Assistant => Task::none(),
            }
        }
        Message::StatusTick => {
            app.child_alive = app.shell.server_running();
            fetch_status(&app.client)
        }
        Message::StatusFetched(result) => {
            match result {
                Ok(status) => {
                    app.status = Some(status);
                    app.status_error = None;
                }
                Err(e) => app.status_error = Some(e),
            }
            Task::none()
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
        Message::Chat(msg) => chat::update(&mut app.chat, &app.client, msg).map(Message::Chat),
        Message::Assistant(msg) => {
            assistant::update(&mut app.assistant, &app.client, msg).map(Message::Assistant)
        }
        Message::Providers(msg) => {
            providers::update(&mut app.providers, &app.client, msg).map(Message::Providers)
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
        iced::time::every(std::time::Duration::from_secs(5)).map(|_| Message::StatusTick),
    ];
    if app.window.is_some() && app.screen == Screen::Logs && !app.logs.paused {
        subs.push(iced::time::every(std::time::Duration::from_secs(1)).map(|_| Message::LogsTick));
    }
    // Polling only runs while the window is open; a hidden app is a server host,
    // not a UI, and must not keep hitting the API.
    if app.window.is_some() && app.screen == Screen::Processes {
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
    if app.window.is_some() && app.screen == Screen::Assistant {
        subs.push(
            iced::time::every(std::time::Duration::from_millis(50))
                .map(|_| Message::Assistant(assistant::Message::Tick)),
        );
    }
    if app.window.is_some() && app.screen == Screen::ModelOps && app.modelops.job_running() {
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
        .run()
}
