//! Status and Logs screens (Phase 2), composed entirely from the shadcn-style
//! `ui` kit — no raw widget styling here.

use crate::ui::{self, space, Tone};
use crate::{App, Message, Screen};
use agent_platform_client::types::ReadinessReport;
use iced::widget::{column, container, row, scrollable};
use iced::{Element, Length};

pub fn view(app: &App) -> Element<'_, Message> {
    let sidebar = container(
        column![
            ui::stack(vec![
                ui::caption("AGENT PLATFORM"),
                ui::nav_item(
                    "Processes",
                    app.screen == Screen::Processes,
                    Message::Nav(Screen::Processes),
                ),
                ui::nav_item(
                    "Projects",
                    app.screen == Screen::Projects,
                    Message::Nav(Screen::Projects),
                ),
                ui::nav_item("Teams", app.screen == Screen::Teams, Message::Nav(Screen::Teams)),
                ui::nav_item(
                    "Model ops",
                    app.screen == Screen::ModelOps,
                    Message::Nav(Screen::ModelOps),
                ),
                ui::nav_item("Chat", app.screen == Screen::Chat, Message::Nav(Screen::Chat)),
                ui::nav_item(
                    "E.V.",
                    app.screen == Screen::Assistant,
                    Message::Nav(Screen::Assistant),
                ),
                ui::nav_item(
                    "Providers",
                    app.screen == Screen::Providers,
                    Message::Nav(Screen::Providers),
                ),
                ui::nav_item("Status", app.screen == Screen::Status, Message::Nav(Screen::Status)),
                ui::nav_item("Logs", app.screen == Screen::Logs, Message::Nav(Screen::Logs)),
            ])
            .width(Length::Fill),
            iced::widget::space::vertical(),
            ui::cluster(vec![
                ui::icon_button(app.settings.theme.icon(), Message::SetTheme(app.settings.theme.next())),
                ui::icon_button("⟳", Message::RestartApp),
            ])
            .width(Length::Fill),
        ]
        .spacing(space::MD),
    )
    .width(200)
    .padding(space::MD)
    .height(Length::Fill)
    .style(ui::theme::sidebar);

    let content = match app.screen {
        Screen::Processes => {
            crate::processes_view::view(&app.processes).map(Message::Processes)
        }
        Screen::Projects => crate::library_view::view(&app.library, crate::library_view::Kind::Projects)
            .map(Message::Library),
        Screen::Teams => crate::library_view::view(&app.library, crate::library_view::Kind::Teams)
            .map(Message::Library),
        Screen::ModelOps => crate::modelops_view::view(&app.modelops).map(Message::ModelOps),
        Screen::Chat => crate::chat_view::view(&app.chat).map(Message::Chat),
        Screen::Assistant => crate::assistant_view::view(&app.assistant).map(Message::Assistant),
        Screen::Providers => crate::providers_view::view(&app.providers).map(Message::Providers),
        Screen::Status => status_view(app),
        Screen::Logs => logs_view(app),
    };

    row![
        sidebar,
        ui::separator_vertical(),
        container(content).width(Length::Fill).height(Length::Fill),
    ]
    .into()
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
                        ui::button_secondary("Re-check port", Message::RestartServer),
                        ui::button_ghost(
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
    blocks.push(api_card(app));

    if let Some(status) = &app.status {
        blocks.push(
            ui::cluster(vec![
                ui::stat("Active processes", status.processes.active.to_string()),
                ui::stat("Total processes", status.processes.total.to_string()),
                ui::stat("Uptime", format!("{:.0}s", status.uptime_seconds)),
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
        blocks.push(ui::empty_state("Waiting for the server. See Logs for startup output."));
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
        .then(|| ui::button_outline("Restart", Message::RestartServer));

    let mut rows = vec![
        ui::field("State", ui::badge(label, tone)),
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
                    if app.key_revealed { "Hide key" } else { "Show key" },
                    Message::ToggleKeyRevealed,
                ),
                ui::button_secondary(
                    copied("key", "Copy key"),
                    Message::Copy("key", app.shell.key.clone()),
                ),
                ui::button_secondary(
                    copied("origin", "Copy URL"),
                    Message::Copy("origin", origin.clone()),
                ),
                ui::button_secondary(copied("curl", "Copy curl"), Message::Copy("curl", curl)),
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
                    ui::badge(
                        if check.ok { "ok" } else { "fail" },
                        if check.ok { Tone::Success } else { Tone::Danger },
                    ),
                    ui::body(check.name.clone()),
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
                ui::button_ghost("Reveal", Message::RevealPath(p.to_string())),
            ]),
        ),
    }
}

// ---------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------

fn logs_view(app: &App) -> Element<'_, Message> {
    let toolbar = ui::cluster(vec![
        container(ui::input("Filter lines…", &app.logs.filter, Message::LogFilterChanged))
            .width(320)
            .into(),
        ui::button_secondary(
            if app.logs.paused { "Resume" } else { "Pause" },
            Message::ToggleLogsPaused,
        ),
        ui::button_ghost("Clear", Message::ClearLogs),
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
        ui::empty_state(if app.logs.lines.is_empty() {
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

    ui::page(
        "Logs",
        Some(ui::muted(
            "Server output, including startup and migrations — visible before the API answers.",
        )),
        None,
        column![toolbar, ui::code(body)].spacing(space::MD),
    )
}
