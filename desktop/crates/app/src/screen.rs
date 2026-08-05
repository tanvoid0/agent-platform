//! Status and Logs screens (Phase 2), composed entirely from the shadcn-style
//! `ui` kit — no raw widget styling here.

use crate::ui::{self, space, Icon, Tone};
use crate::{App, Message, Screen, ServerState, SettingsTab};
use agent_platform_client::types::ReadinessReport;
use iced::widget::{column, container, row, scrollable, Column};
use iced::{Element, Length};

/// The sidebar: two short groups of things you *work in*. Everything you
/// configure or inspect is one entry below them, on its own tabbed page — so
/// the top level is five destinations rather than nine.
const NAV: &[(&str, &[(Screen, Icon, &str)])] = &[
    (
        "WORKSPACE",
        &[
            (Screen::Dashboard, Icon::Gauge, "Dashboard"),
            (Screen::Processes, Icon::Activity, "Processes"),
            (Screen::Projects, Icon::Folder, "Projects"),
            (Screen::Teams, Icon::Users, "Teams"),
            (Screen::Workflows, Icon::Zap, "Workflows"),
            (Screen::Plans, Icon::ListChecks, "Plans"),
            (Screen::Agenda, Icon::Clock, "Agenda"),
        ],
    ),
    // One entry, two tabs: see [`chat_view`].
    ("ASSISTANTS", &[(Screen::Assistant, Icon::Message, "Assistants")]),
];

/// The chat tab strip: the assistant, and what it remembers about you. Text and
/// voice are one screen — the toggle lives in its header, not out here.
const CHAT_TABS: [(Screen, &str); 2] =
    [(Screen::Assistant, crate::assistant::NAME), (Screen::Memory, "Memory")];

pub fn view(app: &App) -> Element<'_, Message> {
    let content = match app.screen {
        Screen::Dashboard => dashboard_view(app),
        Screen::Settings => settings_view(app),
        _ if !app.view_available() => blocked_view(app, screen_title(app.screen)),
        Screen::Processes => crate::processes_view::view(&app.processes, &app.settings.theme.resolve())
            .map(Message::Processes),
        Screen::Projects => {
            crate::library_view::view(&app.library, crate::library_view::Kind::Projects)
                .map(Message::Library)
        }
        Screen::Teams => crate::library_view::view(&app.library, crate::library_view::Kind::Teams)
            .map(Message::Library),
        Screen::Workflows => crate::workflows_view::view(&app.workflows).map(Message::Workflows),
        Screen::Plans => crate::todos_view::view(&app.todos).map(Message::Todos),
        Screen::Agenda => crate::agenda_view::view(&app.agenda).map(Message::Agenda),
        Screen::Assistant | Screen::Memory => chat_view(app),
    };

    let shell = row![
        sidebar(app),
        ui::separator_vertical(),
        container(content).width(Length::Fill).height(Length::Fill),
    ];

    let shell: Element<'_, Message> = match notice(app) {
        Some((text, _)) => {
            ui::toast_layer(shell, ui::toast(text, Tone::Success, Message::NoticeExpired))
        }
        None => shell.into(),
    };

    // Close-button prompt: drawn in-app so quitting looks like the rest of the
    // app rather than like a Windows message box.
    if app.close_prompt.is_none() {
        return shell;
    }
    ui::modal(
        shell,
        ui::confirm_dialog(
            "Close Agent Platform?",
            "The server keeps running in the tray unless you close the app.",
            vec![
                ui::button_ghost(Icon::X, "Cancel", Message::CloseCancelled),
                ui::button_secondary(Icon::Monitor, "Minimize to tray", Message::MinimizeToTray),
                ui::button_destructive(Icon::Stop, "Close", Message::CloseConfirmed),
            ],
        ),
        460.0,
    )
}

/// The transient message of whatever screen is open, with the generation
/// counter the toast timer keys on. Screens keep their own `notice`; this is
/// the one place that turns them into a toast.
pub fn notice(app: &App) -> Option<(String, u64)> {
    match app.screen {
        Screen::Projects | Screen::Teams => app.library.notice.get(),
        Screen::Processes => app.processes.notice.get(),
        Screen::Workflows => app.workflows.notice.get(),
        Screen::Settings => match app.settings_tab {
            SettingsTab::Providers => app.providers.notice.get(),
            SettingsTab::ModelOps => app.modelops.notice.get(),
            _ => None,
        },
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Sidebar
// ---------------------------------------------------------------------------

fn sidebar(app: &App) -> Element<'_, Message> {
    let ready = app.server_ready();
    let mut items: Vec<Element<'_, Message>> = Vec::new();
    for (group, entries) in NAV {
        items.push(ui::nav_group(group));
        for (screen, glyph, label) in *entries {
            items.push(if screen.is_chat() {
                // One entry for the whole chat page; it returns to the last tab
                // open. Never locked — Memory is a local file, so the page has
                // something to show even with the server down (the two tabs that
                // need it are guarded individually, as in Settings).
                ui::nav_item(*glyph, label, app.screen.is_chat(), Message::Nav(app.chat_tab))
            } else if screen.needs_server() && !ready {
                ui::nav_item_locked(*glyph, label)
            } else {
                ui::nav_item(*glyph, label, app.screen == *screen, Message::Nav(*screen))
            });
        }
    }

    container(
        column![
            brand(app),
            // The list scrolls on its own so the brand block and the footer keep
            // their place in a short window.
            scrollable(Column::with_children(items).spacing(2.0)).height(Length::Fill),
            ui::separator(),
            // Settings sits with the window controls, not in the groups above:
            // it is where you go to change the app, not to use it. It never
            // locks — Status and Logs live inside it.
            ui::nav_item(
                Icon::Settings,
                "Settings",
                app.screen == Screen::Settings,
                Message::Nav(Screen::Settings),
            ),
            ui::cluster(vec![
                ui::icon_button(
                    app.settings.theme.icon(),
                    Message::SetTheme(app.settings.theme.next()),
                ),
                ui::icon_button(Icon::Refresh, Message::RestartApp),
            ])
            .width(Length::Fill),
        ]
        .spacing(space::SM),
    )
    .width(208)
    .padding(space::MD)
    .height(Length::Fill)
    .style(ui::theme::sidebar)
    .into()
}

// ---------------------------------------------------------------------------
// Dashboard — E.V. front and center, the platform's vitals around it
// ---------------------------------------------------------------------------

/// The landing page: E.V.'s live HUD with a way in, then stat tiles fed by the
/// same global status poll every other screen uses. Renders with the server
/// down — the server tile is then exactly the thing worth looking at.
fn dashboard_view(app: &App) -> Element<'_, Message> {
    let mode = app.assistant.mode();
    let (mode_label, mode_tone) = match mode {
        crate::assistant::Mode::Idle => ("SYSTEMS NOMINAL", Tone::Success),
        crate::assistant::Mode::Armed => ("MIC LIVE · MONITORING", Tone::Info),
        crate::assistant::Mode::Listening => ("LISTENING", Tone::Danger),
        crate::assistant::Mode::Thinking => ("ANALYZING", Tone::Warning),
        crate::assistant::Mode::Speaking => ("TRANSMITTING", Tone::Info),
    };
    let (srv_label, srv_tone) = server_label(app.server_state());

    let mut blocks: Vec<Element<'_, Message>> = vec![
        // Takes the leftover height so the page does not end halfway down, but
        // stops at a panel's worth: the canvas paints a fixed dark palette in
        // either theme, and an unbounded one becomes a black slab across a
        // light page.
        // Big enough to be the landing page's headline, bounded on purpose: the
        // canvas paints a fixed dark palette in either theme, so a HUD that took
        // the window's leftover height became a black slab across a light page.
        // Whatever is left below is ordinary background, and the page scrolls if
        // the window is too short for the tiles.
        crate::assistant_view::hud(&app.assistant, 420.0).map(Message::Assistant),
        ui::cluster(vec![
            ui::badge(mode_label, mode_tone),
            ui::badge(srv_label, srv_tone),
            ui::spacer(),
            ui::button_default(Icon::Message, "Talk to E.V.", Message::Nav(Screen::Assistant)),
        ])
        .into(),
    ];

    let mut tiles = vec![ui::stat(
        Icon::Sparkles,
        "Memories",
        app.memory.items.len().to_string(),
    )];
    // The five server tiles are always pushed — as "—" until status lands — so the
    // row keeps its shape instead of snapping from one full-width tile to six.
    match &app.status {
        Some(status) => {
            let ok = |r: &ReadinessReport| {
                format!("{}/{}", r.checks.iter().filter(|c| c.ok).count(), r.checks.len())
            };
            tiles.extend([
                ui::stat(Icon::Activity, "Active processes", status.processes.active.to_string()),
                ui::stat(Icon::Scroll, "Total processes", status.processes.total.to_string()),
                ui::stat(Icon::Clock, "Uptime", uptime(status.uptime_seconds)),
                ui::stat(Icon::CheckCircle, "Readiness", ok(&status.readiness)),
                ui::stat(Icon::Plug, "LLM proxy", ok(&status.llm_proxy)),
            ]);
        }
        None => tiles.extend([
            ui::stat(Icon::Activity, "Active processes", "—"),
            ui::stat(Icon::Scroll, "Total processes", "—"),
            ui::stat(Icon::Clock, "Uptime", "—"),
            ui::stat(Icon::CheckCircle, "Readiness", "—"),
            ui::stat(Icon::Plug, "LLM proxy", "—"),
        ]),
    }
    blocks.push(ui::cluster(tiles).into());

    if app.status.is_none() {
        // Say *why* there are no stats — a port conflict and a stopped server are
        // not "any moment now".
        blocks.push(match app.server_state() {
            ServerState::Conflict => ui::empty_state_icon(
                Icon::Alert,
                "Another server owns the port — see Settings → Status.",
            ),
            ServerState::Unreachable => ui::empty_state_icon(
                Icon::Alert,
                "The server is not running — restart it or see Settings → Status.",
            ),
            _ => ui::empty_state_icon(
                Icon::Clock,
                "Platform stats appear as soon as the server answers.",
            ),
        });
    }

    ui::page(
        "Dashboard",
        Some(ui::muted("E.V. and the platform's vitals at a glance.")),
        None,
        ui::stack_lg(blocks),
    )
}

fn uptime(seconds: f64) -> String {
    let s = seconds as u64;
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m {}s", s / 60, s % 60),
        _ => format!("{}h {}m", s / 3600, (s % 3600) / 60),
    }
}

// ---------------------------------------------------------------------------
// Settings — one page, four tabs
// ---------------------------------------------------------------------------

/// The four back-of-house screens on one page. Providers and Model ops are what
/// you change; Status and Logs are what you read when a change did not take.
/// Each tab keeps its own server gate, so the page opens even when nothing else
/// does and lands you on the two tabs that still work.
fn settings_view(app: &App) -> Element<'_, Message> {
    let ready = app.server_ready();
    let tabs = ui::segmented(SettingsTab::ALL.map(|tab| {
        (tab.label(), app.settings_tab == tab, Message::NavSettings(tab))
    }));

    let body = if app.settings_tab.needs_server() && !ready {
        blocked_view(app, app.settings_tab.label())
    } else {
        match app.settings_tab {
            SettingsTab::Providers => {
                crate::providers_view::view(&app.providers).map(Message::Providers)
            }
            SettingsTab::ModelOps => {
                crate::modelops_view::view(&app.modelops).map(Message::ModelOps)
            }
            SettingsTab::Status => status_view(app),
            SettingsTab::Logs => logs_view(app),
        }
    };

    tabbed(tabs, body)
}

// ---------------------------------------------------------------------------
// Chat — one page, two tabs
// ---------------------------------------------------------------------------

/// The assistant on one page: `E.V.` is the conversation — plain text, or with
/// the HUD and a voice behind its own toggle — and `Memory` is what it has
/// learned about you. Tabbed rather than two sidebar entries because they are
/// one destination.
///
/// Gating is per tab, as in [`settings_view`]: Memory is a local file and opens
/// whether or not the server is answering.
fn chat_view(app: &App) -> Element<'_, Message> {
    let tabs = ui::segmented(
        CHAT_TABS.map(|(screen, label)| (label, app.screen == screen, Message::Nav(screen))),
    );
    let body = match app.screen {
        _ if app.screen.needs_server() && !app.server_ready() => {
            blocked_view(app, screen_title(app.screen))
        }
        Screen::Memory => crate::memory_view::view(&app.memory).map(Message::Memory),
        _ => with_history(
            app,
            crate::assistant::NAME,
            crate::assistant_view::view(&app.assistant, &app.settings.theme.resolve())
                .map(Message::Assistant),
        ),
    };
    match memory_notice(app) {
        Some(banner) => tabbed(tabs, column![banner, body].spacing(space::SM).into()),
        None => tabbed(tabs, body),
    }
}

/// A chat body with the past-conversations sidebar on its left. Only the two
/// live threads get one — Memory has no thread to save.
fn with_history<'a>(
    app: &'a App,
    source: &str,
    body: Element<'a, Message>,
) -> Element<'a, Message> {
    row![
        history_panel(app, source),
        ui::separator_vertical(),
        container(body).width(Length::Fill).height(Length::Fill),
    ]
    .into()
}

/// The sidebar itself: New chat on top, then this tab's saved conversations,
/// most recently touched first. Clicking a row loads it; the trash forgets it.
fn history_panel<'a>(app: &'a App, source: &str) -> Element<'a, Message> {
    let current = app.history.current(source);
    let rows = app.history.visible(source);
    let mut items: Vec<Element<'_, Message>> = vec![ui::button_secondary(
        Icon::Plus,
        "New chat",
        Message::History(crate::history::Message::New),
    )];
    if rows.is_empty() {
        items.push(ui::caption("Past chats appear here."));
    }
    for c in rows {
        items.push(
            ui::cluster(vec![
                container(ui::nav_item(
                    Icon::Message,
                    &c.title,
                    current == Some(c.id),
                    Message::History(crate::history::Message::Select(c.id)),
                ))
                .width(Length::Fill)
                .into(),
                ui::icon_button(
                    Icon::Trash,
                    Message::History(crate::history::Message::Delete(c.id)),
                ),
            ])
            .into(),
        );
    }
    container(scrollable(Column::with_children(items).spacing(2.0)).height(Length::Fill))
        .width(224)
        .padding(space::SM)
        .height(Length::Fill)
        .style(ui::theme::sidebar)
        .into()
}

/// The "memory updated" banner: shown in the chat the facts came from, right
/// after a harvest saves something, so a wrong guess can be discarded without a
/// trip to the Memory tab.
fn memory_notice(app: &App) -> Option<Element<'_, Message>> {
    let source = match app.screen {
        Screen::Assistant => crate::assistant::NAME,
        _ => return None,
    };
    let notice = app.memory.notice.as_ref().filter(|n| n.source == source)?;

    let mut lines: Vec<Element<'_, Message>> =
        notice.facts.iter().map(|(_, text)| ui::muted(format!("• {text}"))).collect();
    lines.push(
        ui::cluster(vec![
            ui::button_ghost(Icon::Check, "Keep", Message::Memory(crate::memory::Message::NoticeKeep)),
            ui::button_ghost(
                Icon::Trash,
                "Discard",
                Message::Memory(crate::memory::Message::NoticeDiscard),
            ),
            ui::button_ghost(
                Icon::Sparkles,
                "Open Memory",
                Message::Nav(Screen::Memory),
            ),
        ])
        .into(),
    );

    Some(
        container(ui::alert(Tone::Info, "Memory updated", Some(ui::stack(lines).into())))
            .padding(iced::Padding { top: 0.0, right: space::LG, bottom: 0.0, left: space::LG })
            .into(),
    )
}

/// Tab strip over a body. The strip is chrome, so it sits outside the tab's own
/// `ui::page` scaffold rather than scrolling away with the content.
fn tabbed<'a>(tabs: Element<'a, Message>, body: Element<'a, Message>) -> Element<'a, Message> {
    container(
        column![
            container(tabs).padding(iced::Padding {
                top: space::MD,
                right: space::LG,
                bottom: 0.0,
                left: space::LG,
            }),
            body
        ]
        .spacing(space::SM),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(ui::theme::app_background)
    .into()
}

/// App name plus the one fact that governs everything else on screen: whether
/// the server is up. Putting it here means the user never has to open Status to
/// learn why a screen is locked.
fn brand(app: &App) -> Element<'_, Message> {
    let (label, tone) = server_label(app.server_state());
    // The mark ships on a near-black plate; on a light sidebar that reads as a
    // hole, so the plate follows the active theme.
    let dark = ui::theme::tokens(&app.settings.theme.resolve()).dark;
    let mark = iced::widget::image(crate::logo_handle(dark)).width(24).height(24);
    ui::stack(vec![
        ui::cluster(vec![mark.into(), ui::body("Agent Platform")]).into(),
        ui::badge(label, tone),
    ])
    .into()
}

fn server_label(state: ServerState) -> (&'static str, Tone) {
    match state {
        ServerState::Ready => ("Connected", Tone::Success),
        ServerState::Starting => ("Starting…", Tone::Warning),
        ServerState::Unreachable => ("Offline", Tone::Danger),
        ServerState::Conflict => ("Port in use", Tone::Danger),
    }
}

// ---------------------------------------------------------------------------
// Server guard
// ---------------------------------------------------------------------------

/// Stands in for any screen or settings tab that needs the API while the API is
/// not there. Deliberately not a bare spinner: it names the state and offers the
/// things that can actually help — the logs, a restart, and the status page.
fn blocked_view<'a>(app: &'a App, title: &'a str) -> Element<'a, Message> {
    let (headline, detail, tone) = match app.server_state() {
        ServerState::Conflict => (
            "Another server owns the port",
            "A server that rejects this install's key is already on this port. \
             Nothing was started, so this screen has no data to show.",
            Tone::Danger,
        ),
        ServerState::Unreachable => (
            "The server is not running",
            "Nothing is answering on the port this app owns.",
            Tone::Danger,
        ),
        // Ready never reaches here (the caller checks first); folding it in with
        // Starting keeps this total rather than panicking inside a view.
        _ => (
            "Waiting for the server",
            "The local API is starting. This screen unlocks by itself as soon as \
             it answers — nothing to click.",
            Tone::Warning,
        ),
    };

    let mut actions = vec![ui::button_secondary(
        Icon::Scroll,
        "Open logs",
        Message::NavSettings(SettingsTab::Logs),
    )];
    if !app.shell.attached {
        actions.push(ui::button_outline(
            Icon::Refresh,
            "Restart server",
            Message::RestartServer,
        ));
    }
    actions.push(ui::button_ghost(
        Icon::Gauge,
        "Status",
        Message::NavSettings(SettingsTab::Status),
    ));

    ui::page(
        title,
        Some(ui::muted("Unavailable until the local server is running.")),
        None,
        ui::stack_lg(vec![ui::alert(
            tone,
            headline,
            Some(ui::stack(vec![ui::muted(detail), ui::cluster(actions).into()]).into()),
        )]),
    )
}

/// The sidebar label for a screen, so a guarded page keeps the title the user
/// clicked instead of a generic one.
fn screen_title(screen: Screen) -> &'static str {
    if screen == Screen::Settings {
        return "Settings";
    }
    if screen.is_chat() {
        return CHAT_TABS.iter().find(|(s, _)| *s == screen).map(|(_, l)| *l).unwrap_or("Chat");
    }
    NAV.iter()
        .flat_map(|(_, entries)| entries.iter())
        .find(|(s, _, _)| *s == screen)
        .map(|(_, _, label)| *label)
        .unwrap_or("Agent Platform")
}


// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

fn status_view(app: &App) -> Element<'_, Message> {
    let mut blocks: Vec<Element<'_, Message>> = Vec::new();

    if app.port_conflict {
        blocks.push(ui::alert(
            Tone::Warning,
            format!("Port {} is already in use", app.shell.port),
            Some(
                ui::stack(vec![
                    ui::muted(
                        "Another server answers on this port and rejects this install's key \
                         (a second install, or a Docker port-forward). No server was started.",
                    ),
                    ui::cluster(vec![
                        ui::button_secondary(Icon::Refresh, "Re-check port", Message::RestartServer),
                        ui::button_ghost(
                            Icon::FolderOpen,
                            "Open data folder",
                            Message::RevealPath(app.shell.data_dir.display().to_string()),
                        ),
                    ])
                    .into(),
                ])
                .into(),
            ),
        ));
    }

    blocks.push(server_card(app));
    #[cfg(feature = "local-llm")]
    blocks.push(local_llm_card(app));
    blocks.push(api_card(app));

    if let Some(status) = &app.status {
        blocks.push(
            ui::cluster(vec![
                ui::stat(Icon::Activity, "Active processes", status.processes.active.to_string()),
                ui::stat(Icon::Scroll, "Total processes", status.processes.total.to_string()),
                ui::stat(Icon::Clock, "Uptime", uptime(status.uptime_seconds)),
            ])
            .into(),
        );
        blocks.push(ui::section("Readiness", None, checks_view(&status.readiness)));
        blocks.push(ui::section("LLM proxy", None, checks_view(&status.llm_proxy)));
        blocks.push(ui::section(
            "Paths",
            None,
            ui::stack(vec![
                path_field("Database", status.paths.database.as_deref()),
                path_field("Workspaces", status.paths.workspaces.as_deref()),
                path_field("LLM config", status.paths.llm_config_dir.as_deref()),
                path_field("Model-ops data", status.paths.model_ops_data.as_deref()),
                ui::field("Backend", ui::body(status.paths.database_backend.clone())),
            ]),
        ));
    } else if !app.port_conflict {
        blocks.push(ui::empty_state_icon(Icon::Clock, "Waiting for the server. See Logs for startup output."));
    }

    ui::page(
        "Status",
        Some(ui::muted("Server health, API access and local paths.")),
        None,
        ui::stack_lg(blocks),
    )
}

fn server_card(app: &App) -> Element<'_, Message> {
    let (label, tone) = match (&app.status, &app.status_error) {
        (Some(_), _) if app.shell.attached => ("Running (external)", Tone::Success),
        (Some(_), _) => ("Running", Tone::Success),
        (None, Some(_)) if app.child_alive => ("Starting…", Tone::Warning),
        (None, Some(_)) => ("Not answering", Tone::Danger),
        (None, None) => ("Checking…", Tone::Neutral),
    };

    let actions: Option<Element<'_, Message>> = (!app.shell.attached && !app.port_conflict)
        .then(|| ui::button_outline(Icon::Refresh, "Restart", Message::RestartServer));

    let mut rows = vec![
        ui::field("State", ui::badge_icon(ui::tone_icon(tone), label, tone)),
        ui::field("Port", ui::mono(app.shell.port.to_string())),
    ];
    if let Some(status) = &app.status {
        rows.push(ui::field("Environment", ui::body(status.env.clone())));
        rows.push(ui::field("Python", ui::body(status.python.clone())));
        rows.push(ui::field("Platform", ui::muted(status.platform.clone())));
    }
    if let Some(err) = &app.status_error {
        rows.push(ui::caption(err.clone()));
    }

    ui::card_with_header(
        "Server",
        Some(ui::muted("The Python API process this app owns.")),
        actions,
        ui::stack(rows),
    )
}

/// In-process inference, per [ADR 0006](../../../../docs/adr/0006-in-process-rust-core.md).
///
/// Only in a `local-llm` build: without the feature there is no engine to point
/// at a file, and the settings keys are inert. Lives on Status rather than Model
/// ops because it is the one model surface that works with the server down.
///
/// Both keys are read once, at the first local turn — a swap needs a restart,
/// which is what the header button is for. The weights themselves come and go on
/// their own (an idle timeout), and "Free VRAM" is the way to hurry that along
/// before a training job wants the card.
#[cfg(feature = "local-llm")]
fn local_llm_card(app: &App) -> Element<'_, Message> {
    let path = app.settings.local_model_path.trim();
    let (state, tone) = match path {
        "" => ("Off — every turn goes to the server", Tone::Neutral),
        _ if crate::local_llm::loaded() => ("Loaded in VRAM", Tone::Success),
        p if std::path::Path::new(p).is_file() => ("Ready — loads on the next turn", Tone::Success),
        _ => ("File is missing", Tone::Danger),
    };
    let engine = match crate::inference::last_turn_was_local() {
        Some(true) => ui::badge_icon(Icon::Cpu, "Answered in-process", Tone::Success),
        Some(false) => ui::badge_icon(Icon::Plug, "Answered by the server", Tone::Neutral),
        None => ui::muted("No turn yet this run"),
    };

    let mut picker = vec![ui::button_outline(Icon::FolderOpen, "Choose…", Message::PickLocalModel)];
    if !path.is_empty() {
        picker.push(ui::button_ghost(
            Icon::X,
            "Clear",
            Message::SetLocalModel(Some(String::new())),
        ));
    }
    if crate::local_llm::loaded() {
        picker.push(ui::button_ghost(Icon::Zap, "Free VRAM", Message::UnloadLocalModel));
    }

    ui::card_with_header(
        "Local model",
        Some(ui::muted("Answer this app's own chat in-process instead of through the server.")),
        Some(ui::button_outline(Icon::Refresh, "Restart app", Message::RestartApp)),
        ui::stack(vec![
            ui::field("State", ui::badge_icon(ui::tone_icon(tone), state, tone)),
            ui::field(
                "GGUF",
                ui::stack(vec![
                    if path.is_empty() { ui::muted("—") } else { ui::mono(path.to_string()) },
                    ui::cluster(picker).into(),
                ]),
            ),
            ui::field(
                "Context",
                container(ui::input("8192", &app.local_ctx_input, Message::SetLocalCtx))
                    .width(Length::Fixed(140.0)),
            ),
            ui::field("Last turn", engine),
            ui::field(
                "Serve to the server",
                ui::stack(vec![
                    container(ui::input(
                        "off",
                        &app.local_server_port_input,
                        Message::SetLocalServerPort,
                    ))
                    .width(Length::Fixed(140.0))
                    .into(),
                    match app.settings.local_server_port {
                        0 => ui::caption(
                            "A port here also answers the Python side, so server-run agents \
                             can use this model. Empty leaves it off.",
                        ),
                        p => ui::caption(format!(
                            "Point the proxy's OpenAI-compatible provider at \
                             http://127.0.0.1:{p} (LM_STUDIO_API_BASE). Loopback only, no key \
                             — like Ollama and LM Studio.",
                        )),
                    },
                ]),
            ),
            ui::caption(
                "A new model, context or port takes effect when the app restarts. The weights \
                 themselves unload after five idle minutes.",
            ),
        ]),
    )
}

fn api_card(app: &App) -> Element<'_, Message> {
    let key_display = if app.shell.key.is_empty() {
        "(no key)".to_string()
    } else if app.key_revealed {
        app.shell.key.clone()
    } else {
        format!("{}{}", &app.shell.key[..6.min(app.shell.key.len())], "•".repeat(12))
    };
    let origin = app.shell.origin();
    let curl = format!(
        "curl -H \"Authorization: Bearer {}\" {origin}/api/v1/system/status",
        app.shell.key
    );

    let copied = |what: &'static str, label: &'static str| -> &'static str {
        if app.copied == Some(what) {
            "Copied!"
        } else {
            label
        }
    };

    ui::card_with_header(
        "API server",
        Some(ui::muted(
            "Other local apps can use this server while the window is closed.",
        )),
        None,
        ui::stack(vec![
            ui::field("Base URL", ui::mono(origin.clone())),
            ui::field("API key", ui::mono(key_display)),
            ui::cluster(vec![
                ui::button_ghost(
                    if app.key_revealed { Icon::EyeOff } else { Icon::Eye },
                    if app.key_revealed { "Hide key" } else { "Show key" },
                    Message::ToggleKeyRevealed,
                ),
                ui::button_secondary(
                    Icon::Copy,
                    copied("key", "Copy key"),
                    Message::Copy("key", app.shell.key.clone()),
                ),
                ui::button_secondary(
                    Icon::Copy,
                    copied("origin", "Copy URL"),
                    Message::Copy("origin", origin.clone()),
                ),
                ui::button_secondary(Icon::Copy, copied("curl", "Copy curl"), Message::Copy("curl", curl)),
            ])
            .into(),
            // The sample never renders the key; the Copy button carries it.
            ui::code(ui::mono(format!(
                "curl -H \"Authorization: Bearer <key>\" {origin}/api/v1/system/status"
            ))),
        ]),
    )
}

fn checks_view(report: &ReadinessReport) -> Element<'_, Message> {
    if report.checks.is_empty() {
        return ui::body(report.status.clone());
    }
    ui::stack(
        report
            .checks
            .iter()
            .map(|check| {
                ui::cluster(vec![
                    ui::badge_icon(
                        if check.ok { Icon::CheckCircle } else { Icon::XCircle },
                        if check.ok { "ok" } else { "fail" },
                        if check.ok { Tone::Success } else { Tone::Danger },
                    ),
                    container(ui::body(check.name.clone())).width(180).into(),
                    ui::muted(check.detail.clone()),
                ])
                .into()
            })
            .collect(),
    )
    .into()
}

fn path_field<'a>(label: &'a str, value: Option<&str>) -> Element<'a, Message> {
    match value {
        None => ui::field(label, ui::muted("—")),
        Some(p) => ui::field(
            label,
            ui::cluster(vec![
                container(ui::mono(p.to_string())).width(Length::Fill).into(),
                ui::button_ghost(Icon::FolderOpen, "Reveal", Message::RevealPath(p.to_string())),
            ]),
        ),
    }
}

// ---------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------

fn logs_view(app: &App) -> Element<'_, Message> {
    let toolbar = ui::cluster(vec![
        container(ui::input_icon(Icon::Search, "Filter lines…", &app.logs.filter, Message::LogFilterChanged))
            .width(320)
            .into(),
        ui::button_secondary(
            if app.logs.paused { Icon::Play } else { Icon::Pause },
            if app.logs.paused { "Resume" } else { "Pause" },
            Message::ToggleLogsPaused,
        ),
        ui::button_ghost(Icon::Trash, "Clear", Message::ClearLogs),
        ui::spacer(),
        ui::badge(
            if app.shell.attached { "server log" } else { "process output" },
            Tone::Neutral,
        ),
    ]);

    let filter = app.logs.filter.to_lowercase();
    let matched: Vec<&String> = app
        .logs
        .lines
        .iter()
        .filter(|l| filter.is_empty() || l.to_lowercase().contains(&filter))
        .collect();
    // Only the tail is rendered: iced lays out every child, and the ring holds
    // thousands of lines.
    let tail = &matched[matched.len().saturating_sub(500)..];

    let body: Element<'_, Message> = if tail.is_empty() {
        ui::empty_state_icon(Icon::Scroll, if app.logs.lines.is_empty() {
            "No output yet."
        } else {
            "No lines match the filter."
        })
    } else {
        let mut lines = column![].spacing(1);
        if app.logs.dropped > 0 {
            lines = lines.push(ui::caption(format!(
                "… {} earlier lines dropped (buffer wrapped)",
                app.logs.dropped
            )));
        }
        for line in tail {
            lines = lines.push(ui::mono((*line).clone()));
        }
        scrollable(lines).height(Length::Fill).anchor_bottom().into()
    };

    // `page_fixed`, not `page`: the log tail scrolls itself. Inside the outer
    // scrollable the two bars stacked, and the right-hand badge sat under the
    // outer one, clipped.
    ui::page_fixed(
        "Logs",
        Some(ui::muted(
            "Server output, including startup and migrations — visible before the API answers.",
        )),
        None,
        column![toolbar, ui::code(body)].spacing(space::MD).height(Length::Fill),
    )
}
