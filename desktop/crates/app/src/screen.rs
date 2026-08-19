//! Status and Logs screens (Phase 2), composed entirely from the shadcn-style
//! `ui` kit — no raw widget styling here.

use crate::ui::{self, space, Icon, Tone};
use crate::shell::ResourceMode;
use crate::{App, HudStyle, Message, Screen, ServerState, SettingsTab, ThemeMode};
use agent_platform_client::types::ReadinessReport;
use iced::widget::{column, container, row, scrollable, Column};
use iced::{Element, Length, Padding};

/// The sidebar: short groups of things you work in. Settings stays in the
/// footer — it is where you change the app, not where you use it.
const NAV: &[(&str, &[(Screen, Icon, &str)])] = &[
    (
        "WORK",
        &[
            (Screen::Dashboard, Icon::House, "Home"),
            (Screen::Processes, Icon::Activity, "Processes"),
            (Screen::Coder, Icon::Cpu, "Coder"),
        ],
    ),
    (
        "ASSIST",
        &[
            (Screen::Assistant, Icon::Message, "Assistants"),
            (Screen::Studio, Icon::Image, "Studio"),
        ],
    ),
    (
        "LIBRARY",
        &[
            (Screen::Projects, Icon::Folder, "Projects"),
            (Screen::Teams, Icon::Users, "Teams"),
        ],
    ),
    (
        "TOOLS",
        &[
            (Screen::Workflows, Icon::Zap, "Workflows"),
            (Screen::Plans, Icon::ListChecks, "Plans"),
            (Screen::Agenda, Icon::Clock, "Agenda"),
            (Screen::Search, Icon::Search, "Search"),
        ],
    ),
];

/// The chat tab strip: the assistant, and what it remembers about you. Text and
/// voice are one screen — the toggle lives in its header, not out here.
/// A function rather than a const because the first label is the assistant's
/// name, which the user can change.
fn chat_tabs() -> [(Screen, &'static str); 2] {
    [(Screen::Assistant, crate::assistant::name()), (Screen::Memory, "Memory")]
}

pub fn view(app: &App) -> Element<'_, Message> {
    let content = match app.screen {
        Screen::Dashboard => dashboard_view(app),
        Screen::Settings => settings_view(app),
        Screen::Logs => logs_view(app),
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
        Screen::Agenda => crate::agenda_view::view(&app.agenda, &app.settings.theme.resolve())
            .map(Message::Agenda),
        Screen::Coder => {
            crate::coder_view::view(&app.coder, &app.settings.theme.resolve()).map(Message::Coder)
        }
        Screen::Search => crate::search_view::view(&app.search).map(Message::Search),
        Screen::Studio => crate::studio_view::view(&app.studio).map(Message::Studio),
        Screen::Assistant | Screen::Memory => chat_view(app),
    };

    let mut shell = row![
        sidebar(app),
        ui::separator_vertical(),
        container(content).width(Length::Fill).height(Length::Fill),
    ];

    // E.V. from anywhere: the Assistant screen's own panel, docked beside
    // whatever the user is working in. Not a second chat — same `State`, same
    // thread, same mic, so a conversation started here is waiting on the
    // Assistant screen and the other way round.
    //
    // A **column of the shell row**, not a layer over it. It was `ui::modal`
    // first, which put a scrim over the app and made the sidebar unclickable;
    // moving it to a scrim-less `stack` layer changed nothing, because the
    // full-window container that positions such a layer swallows the click
    // either way (measured — see [`ui::toast_layer`]). You summon E.V. *while*
    // working, so the page it sits next to has to stay live, and the only way
    // to get that is to not cover it.
    //
    // Suppressed on the chat screens themselves, where it would sit beside the
    // transcript it is a copy of.
    if app.assistant_open && !app.screen.is_chat() {
        shell = shell
            .push(ui::separator_vertical())
            .push(container(assistant_panel(app)).width(460).height(Length::Fill));
    }

    let shell: Element<'_, Message> = match notice(app) {
        Some((text, _)) => {
            ui::toast_layer(shell, ui::toast(text, Tone::Success, Message::NoticeExpired))
        }
        None => shell.into(),
    };

    // Over the E.V. panel: the bell is how you leave whatever is on screen for
    // the thing that finished behind it.
    let shell = match app.notifications_open {
        false => shell,
        true => ui::modal(shell, notifications_panel(), 560.0),
    };
    let shell = match app.help_open {
        false => shell,
        true => ui::modal(shell, help_panel(), 520.0),
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
                // Cancel and Close have no icon that adds meaning to the word,
                // so they are label-only; "Minimize to tray" keeps its monitor.
                ui::button_sized(
                    None,
                    "Cancel",
                    ui::ButtonVariant::Ghost,
                    ui::Size::Sm,
                    Some(Message::CloseCancelled),
                ),
                ui::button_secondary(Icon::Monitor, "Minimize to tray", Message::MinimizeToTray),
                ui::button_sized(
                    None,
                    "Close",
                    ui::ButtonVariant::Destructive,
                    ui::Size::Sm,
                    Some(Message::CloseConfirmed),
                ),
            ],
        ),
        460.0,
    )
}

/// The docked E.V. panel: the Assistant screen's own transcript and composer in
/// a column of their own. Fills its column's height — it is a real region of
/// the layout now, so there is nothing to clamp it against.
fn assistant_panel(app: &App) -> Element<'_, Message> {
    let head = ui::cluster(vec![
        ui::badge(crate::assistant::name(), Tone::Danger),
        ui::spacer(),
        // The panel is the quick answer; the screen is where the header,
        // provider pickers and full history live.
        ui::button_ghost(Icon::Message, "Open screen", Message::Nav(Screen::Assistant)),
        ui::button_ghost(Icon::X, "Close", Message::ToggleAssistant),
    ]);
    let body = crate::assistant_view::panel(
        &app.assistant,
        &app.settings.theme.resolve(),
        app.settings.hud_style,
    )
    .map(Message::Assistant);
    container(column![head, container(body).height(Length::Fill)].spacing(space::MD))
        .padding(space::MD)
        .height(Length::Fill)
        .into()
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
                {
                    // The count is E.V.'s, not the page's: Memory has nothing
                    // running behind it.
                    let (count, tone) = crate::screen_notes(Screen::Assistant);
                    ui::nav_item_counted(
                        *glyph,
                        label,
                        app.screen.is_chat(),
                        count,
                        tone,
                        Message::Nav(app.chat_tab),
                    )
                }
            } else if screen.needs_server() && !ready {
                ui::nav_item_locked(*glyph, label)
            } else {
                let (count, tone) = crate::screen_notes(*screen);
                ui::nav_item_counted(
                    *glyph,
                    label,
                    app.screen == *screen,
                    count,
                    tone,
                    Message::Nav(*screen),
                )
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
            // Above the utility strip, so the last thing before the controls is
            // what the app is currently costing. Absent, not blank, when there
            // is nothing to report — an empty gauge reads as "idle", which is a
            // different claim from "no server".
            match resource_monitor(app) {
                Some(monitor) => monitor,
                None => ui::spacer(),
            },
            // Settings sits with the window controls, not in the groups above:
            // it is where you go to change the app, not to use it. It never
            // locks — Status and Logs live inside it. Icon-only like its two
            // neighbors, so the row reads as one utility strip; each gets a
            // tooltip since none of the three carry a visible label.
            ui::cluster(footer_controls(app)).width(Length::Fill),
        ]
        .spacing(space::SM),
    )
    .width(208)
    .padding(space::MD)
    .height(Length::Fill)
    .style(ui::theme::sidebar)
    .into()
}

/// The utility strip under the separator. A `Vec` rather than a literal because
/// the mic indicator is conditional, and an always-present placeholder would
/// leave its gap in the row on every launch that never turns the mic on.
fn footer_controls(app: &App) -> Vec<Element<'_, Message>> {
    let mut controls: Vec<Element<'_, Message>> = Vec::with_capacity(8);
    // Wake-word standby holds the mic open across every screen, with no HUD
    // anywhere reporting it. The composer says so on the one screen that has a
    // composer; this says so on all of them, and clicking it is the off switch
    // — a live mic the user did not ask for must never be more than one click
    // from off. Leftmost, so it is the first thing in the strip.
    if app.assistant.standby && app.assistant.armed() {
        controls.push(ui::nav_icon_button(
            Icon::Mic,
            "Mic live — listening for its name. Click to stop.",
            true,
            Message::SetWakeWord(false),
        ));
    }
    controls.extend([
        // E.V. anywhere. Ctrl+K does the same thing; this is the half
        // of it that is discoverable without knowing the shortcut.
        ui::nav_icon_button(
            Icon::Sparkles,
            "Ask the assistant (Ctrl+K)",
            app.assistant_open,
            Message::ToggleAssistant,
        ),
        ui::nav_icon_button(
            Icon::Info,
            "Shortcuts (Ctrl+/)",
            app.help_open,
            Message::ToggleHelp,
        ),
        ui::nav_icon_button(
            Icon::Settings,
            "Settings",
            app.screen == Screen::Settings,
            Message::Nav(Screen::Settings),
        ),
                ui::nav_icon_button(
                    Icon::Scroll,
                    "Logs",
                    app.screen == Screen::Logs,
                    Message::Nav(Screen::Logs),
                ),
                ui::tooltip(
                    ui::icon_button(
                        app.settings.theme.icon(),
                        Message::SetTheme(app.settings.theme.next()),
                    ),
                    "Toggle theme",
                ),
        ui::tooltip(ui::icon_button(Icon::Refresh, Message::RestartApp), "Restart app"),
    ]);
    controls
}

// ---------------------------------------------------------------------------
// Home — compact HUD, start a run, anything waiting on you
// ---------------------------------------------------------------------------

/// The landing page: a compact HUD, the same start-a-run form as Processes,
/// then the runs that have stopped for a human. Stats stay as a thin row so
/// the inbox is the thing you actually look at.
fn dashboard_view(app: &App) -> Element<'_, Message> {
    let mode = app.assistant.mode();
    let (mode_label, mode_tone) = match mode {
        crate::assistant::Mode::Idle => (crate::assistant_view::mode_label(mode), Tone::Success),
        crate::assistant::Mode::Armed => (crate::assistant_view::mode_label(mode), Tone::Info),
        crate::assistant::Mode::Listening => (crate::assistant_view::mode_label(mode), Tone::Danger),
        crate::assistant::Mode::Thinking => (crate::assistant_view::mode_label(mode), Tone::Warning),
        crate::assistant::Mode::Speaking => (crate::assistant_view::mode_label(mode), Tone::Info),
    };
    let (srv_label, srv_tone) = server_label(app.server_state());

    let mut blocks: Vec<Element<'_, Message>> = vec![
        crate::assistant_view::hud(
            &app.assistant,
            180.0,
            app.settings.hud_style,
            &app.settings.theme.resolve(),
        )
        .map(Message::Assistant),
        ui::cluster(vec![
            ui::badge(mode_label, mode_tone),
            ui::badge(srv_label, srv_tone),
            ui::spacer(),
            ui::button_default(
                Icon::Message,
                crate::assistant::talk_label(),
                Message::Nav(Screen::Assistant),
            ),
        ])
        .into(),
    ];

    let mut tiles = vec![ui::stat(
        Icon::Sparkles,
        "Memories",
        app.memory.items.len().to_string(),
    )];
    match &app.status {
        Some(status) => {
            tiles.push(ui::stat(
                Icon::Activity,
                "Active processes",
                status.processes.active.to_string(),
            ));
            tiles.push(ui::stat(Icon::Server, "Server", srv_label));
        }
        None => {
            tiles.push(ui::stat(Icon::Activity, "Active processes", "—"));
            tiles.push(ui::stat(Icon::Server, "Server", srv_label));
        }
    }
    blocks.push(ui::cluster(tiles).into());
    blocks.push(
        ui::cluster(vec![
            ui::caption("Also"),
            ui::button_ghost(Icon::ListChecks, "Plans", Message::Nav(Screen::Plans)),
            ui::button_ghost(Icon::Zap, "Workflows", Message::Nav(Screen::Workflows)),
            ui::button_ghost(Icon::Clock, "Agenda", Message::Nav(Screen::Agenda)),
        ])
        .into(),
    );

    if app.status.is_none() {
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

    if app.server_ready() {
        blocks.push(
            crate::processes_view::new_run_composer(&app.processes).map(Message::Processes),
        );
        let waiting: Vec<_> = app
            .processes
            .processes
            .iter()
            .filter(|p| crate::processes::needs_user(p.status))
            .collect();
        let inbox: Element<'_, Message> = if waiting.is_empty() {
            ui::empty_state_action(
                Icon::Inbox,
                "No runs waiting on you.",
                ui::button_outline(Icon::Activity, "All runs", Message::Nav(Screen::Processes)),
            )
        } else {
            ui::stack(
                waiting
                    .iter()
                    .map(|p| {
                        let mut lines = vec![
                            ui::cluster(vec![
                                ui::badge(
                                    crate::domain::process_status_label(p.status.as_str()),
                                    crate::domain::process_status_tone(p.status.as_str()),
                                ),
                                ui::caption(format!("#{}", p.id)),
                                ui::spacer(),
                                ui::caption(
                                    crate::domain::relative_time(&p.created_at)
                                        .unwrap_or_default(),
                                ),
                            ])
                            .into(),
                            ui::body(crate::domain::truncate(&p.goal, 90)),
                        ];
                        if let Some(hint) = crate::domain::process_waiting_hint(p.status.as_str()) {
                            lines.push(ui::caption(hint));
                        }
                        ui::list_item(ui::stack(lines), false, Message::OpenRun(p.id))
                    })
                    .collect(),
            )
            .into()
        };
        blocks.push(ui::card_with_header(
            "Needs you",
            Some(ui::muted(
                "Plan approval or task review — the run will not move until you answer.",
            )),
            None,
            inbox,
        ));
    }

    ui::page(
        "Home",
        Some(ui::muted(format!(
            "Start a run, or pick up anything waiting on you. {} is standing by.",
            crate::assistant::name()
        ))),
        Some(ui::button_ghost(Icon::Activity, "All runs", Message::Nav(Screen::Processes))),
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
// Settings — one page, six tabs
// ---------------------------------------------------------------------------

/// Providers and Model ops are what you change; Status and Logs are what you
/// read when a change did not take. Appearance, Performance and API sit with
/// them. Each tab keeps its own server gate, so the page opens even when
/// nothing else does and lands you on the tabs that still work.
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
            SettingsTab::Appearance => appearance_view(app),
            SettingsTab::Performance => performance_view(app),
            SettingsTab::Status => status_view(app),
            SettingsTab::Api => {
                crate::apidocs_view::view(&app.apidocs, &app.shell.origin(), &app.shell.key)
                    .map(Message::ApiDocs)
            }
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
        chat_tabs().map(|(screen, label)| (label, app.screen == screen, Message::Nav(screen))),
    );
    let body = match app.screen {
        _ if app.screen.needs_server() && !app.server_ready() => {
            blocked_view(app, screen_title(app.screen))
        }
        Screen::Memory => crate::memory_view::view(&app.memory).map(Message::Memory),
        _ => with_history(
            app,
            crate::assistant::NAME,
            crate::assistant_view::view(
                &app.assistant,
                &app.settings.theme.resolve(),
                app.settings.hud_style,
            )
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
                ui::icon_tip(
                    Icon::Trash,
                    if app.history.delete_armed == Some(c.id) {
                        "Click again to delete"
                    } else {
                        "Delete chat"
                    },
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
        // The bell shares the status line rather than the name's: at 208px the
        // sidebar has no room for both, and the name wrapped onto two lines to
        // make it. What it counts is everything the sidebar badges count, from
        // every screen at once — including the Settings tabs that have no badge
        // of their own.
        ui::cluster(vec![
            ui::badge(label, tone),
            ui::spacer(),
            ui::bell(crate::notify::total(), crate::note_tone(""), Message::ToggleNotifications),
        ])
        .into(),
    ])
    .into()
}

/// The bell's panel: everything that happened while the user was on another
/// screen, newest first. A row is a way back to the work — pressing it goes to
/// the screen the note came from, which is also what marks the rest of that
/// screen's notes seen.
fn notifications_panel<'a>() -> Element<'a, Message> {
    let notes = crate::notify::notes();
    let mut head = vec![
        ui::badge(
            ui::count(notes.len(), "notification", "notifications"),
            crate::note_tone(""),
        ),
        ui::spacer(),
    ];
    if !notes.is_empty() {
        head.push(ui::button_ghost(Icon::Check, "Clear all", Message::ClearNotifications));
    }
    head.push(ui::button_ghost(Icon::X, "Close", Message::ToggleNotifications));

    let body: Element<'_, Message> = if notes.is_empty() {
        ui::empty_state_icon(
            Icon::Bell,
            "Nothing waiting. Work that finishes while you are on another screen shows up here.",
        )
    } else {
        let rows: Vec<Element<'_, Message>> = notes
            .iter()
            .map(|note| {
                let (label, tone) = match note.kind {
                    crate::notify::Kind::Review => ("needs you", Tone::Warning),
                    crate::notify::Kind::Done => ("finished", Tone::Info),
                };
                ui::cluster(vec![
                    container(ui::list_item(
                        ui::cluster(vec![
                            ui::badge(label, tone),
                            container(ui::stack(vec![
                                ui::body(note.title.clone()),
                                ui::muted(note.body.clone()),
                            ]))
                            .width(Length::Fill)
                            .into(),
                        ]),
                        false,
                        Message::OpenNote(note.id),
                    ))
                    .width(Length::Fill)
                    .into(),
                    ui::icon_tip(Icon::X, "Dismiss", Message::DismissNote(note.id)),
                ])
                .into()
            })
            .collect();
        scrollable(ui::stack(rows)).spacing(space::SM).height(Length::Fill).into()
    };

    container(ui::card(
        column![ui::cluster(head), container(body).height(Length::Fill)]
            .spacing(space::MD)
            .height(Length::Fill),
    ))
    .height(460)
    .into()
}

/// Shortcuts, a jump list, and how a run works. The missing first-run and the
/// missing palette in one sheet — Ctrl+/ or the footer.
fn help_panel<'a>() -> Element<'a, Message> {
    ui::card_with_header(
        "Shortcuts",
        Some(ui::muted("Ctrl+/ closes this sheet. Esc does too.")),
        Some(ui::button_ghost(Icon::X, "Close", Message::ToggleHelp)),
        ui::stack_lg(vec![
            ui::stack(vec![
                ui::caption("Ctrl+K — ask the assistant from any screen"),
                ui::caption("Ctrl+/ — this sheet"),
                ui::caption("Esc — close a dialog, stop a reply, or cancel"),
            ])
            .into(),
            ui::heading("Go to"),
            ui::cluster(vec![
                ui::button_ghost(Icon::House, "Home", Message::Nav(Screen::Dashboard)),
                ui::button_ghost(Icon::Activity, "Processes", Message::Nav(Screen::Processes)),
                ui::button_ghost(Icon::Cpu, "Coder", Message::Nav(Screen::Coder)),
                ui::button_ghost(Icon::Message, "Assistants", Message::Nav(Screen::Assistant)),
            ])
            .into(),
            ui::heading("How a run works"),
            ui::stack(vec![
                ui::body("1. Write a goal and pick a team."),
                ui::body("2. Approve the plan — or turn on auto-approve to skip that gate."),
                ui::body("3. Review a task if the run pauses again."),
                ui::caption(
                    "Lists you move by hand are Plans. Recurring steps are Workflows. \
                     Agenda is your day.",
                ),
            ])
            .into(),
        ]),
    )
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
        Message::Nav(Screen::Logs),
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
    if screen == Screen::Logs {
        return "Logs";
    }
    if screen.is_chat() {
        return chat_tabs().iter().find(|(s, _)| *s == screen).map(|(_, l)| *l).unwrap_or("Chat");
    }
    NAV.iter()
        .flat_map(|(_, entries)| entries.iter())
        .find(|(s, _, _)| *s == screen)
        .map(|(_, _, label)| *label)
        .unwrap_or("Agent Platform")
}


// ---------------------------------------------------------------------------
// Appearance
// ---------------------------------------------------------------------------

/// Theme, E.V.'s animation and its speaking pace. The canvas below is the real
/// one, ticking on the real mic — picking between two still thumbnails would
/// tell you nothing about animations whose whole point is how they move.
fn appearance_view(app: &App) -> Element<'_, Message> {
    let style = app.settings.hud_style;
    let rate = app.settings.voice_rate;
    let name = crate::assistant::name();
    ui::page(
        "Appearance",
        Some(ui::muted(format!("How the app and {name} look and sound."))),
        None,
        ui::stack_lg(vec![
            ui::card_with_header(
                "Theme",
                Some(ui::muted("System follows the OS and switches with it.")),
                None,
                ui::segmented([
                    ("System", app.settings.theme == ThemeMode::System, Message::SetTheme(ThemeMode::System)),
                    ("Light", app.settings.theme == ThemeMode::Light, Message::SetTheme(ThemeMode::Light)),
                    ("Dark", app.settings.theme == ThemeMode::Dark, Message::SetTheme(ThemeMode::Dark)),
                ]),
            ),
            ui::card_with_header(
                format!("{name} animation"),
                Some(ui::muted(match style {
                    HudStyle::Bubble => "A soft orb drawn on the GPU — the smoothest of the three.",
                    HudStyle::BubbleCanvas => {
                        "The same orb without the GPU. Pick this if Bubble renders blank."
                    }
                    HudStyle::Suit => "The full suit HUD: spectrum web, reticle and telemetry.",
                })),
                None,
                ui::stack(vec![
                    ui::segmented(
                        HudStyle::ALL.map(|s| (s.label(), style == s, Message::SetHudStyle(s))),
                    ),
                    crate::assistant_view::hud(&app.assistant, 260.0, style, &app.settings.theme.resolve())
                        .map(Message::Assistant),
                    ui::caption("Both are driven by the live mic — talk to see them move."),
                ]),
            ),
            ui::card_with_header(
                "Terminal",
                Some(ui::muted(format!(
                    "{name} can run shell commands on this machine. With this on it shows you \
                     the command and waits; with it off it runs whatever it decides to."
                ))),
                None,
                ui::stack(vec![
                    ui::toggle(
                        if app.settings.confirm_commands { Icon::Check } else { Icon::Alert },
                        if app.settings.confirm_commands {
                            "Ask before running a command"
                        } else {
                            "Run commands without asking"
                        },
                        app.settings.confirm_commands,
                        Message::SetConfirmCommands(!app.settings.confirm_commands),
                    ),
                    ui::caption(
                        "Turning this off gives a language model an unattended terminal with \
                         your account's permissions. There is no allowlist behind it — the \
                         card is the only check.",
                    ),
                ]),
            ),
            ui::card_with_header(
                "Name",
                Some(ui::muted(
                    "What this assistant is called — on screen, in its persona, and as \
                     the wake word. Chats and memories it already filed keep their byline.",
                )),
                None,
                ui::stack(vec![
                    ui::field(
                        "Name",
                        ui::input(
                            crate::assistant::DEFAULT_NAME,
                            &app.settings.assistant_name,
                            Message::SetAssistantName,
                        ),
                    ),
                    ui::field(
                        "Heard as",
                        ui::input(
                            "eva, ava, evie",
                            &app.settings.wake_names,
                            Message::SetWakeNames,
                        ),
                    ),
                    ui::caption(
                        "Speech-to-text writes a spoken name however it likes, so the wake \
                         word matches spellings rather than the name: list them separated \
                         by commas, one word each. Left empty it listens for the name as \
                         written.",
                    ),
                ]),
            ),
            ui::card_with_header(
                "Wake word",
                Some(ui::muted(format!(
                    "Keeps the mic open across the whole app, listening for its name. \
                     Nothing is sent unless you say “{name}, …” — everything else it hears \
                     is dropped, not saved and not typed anywhere."
                ))),
                None,
                ui::stack(vec![
                    ui::toggle(
                        if app.settings.wake_word { Icon::Mic } else { Icon::MicOff },
                        if app.settings.wake_word {
                            format!("Listening for “{name}”")
                        } else {
                            "Off".to_string()
                        },
                        app.settings.wake_word,
                        Message::SetWakeWord(!app.settings.wake_word),
                    ),
                    ui::caption(
                        "Transcription is local (whisper). Saying its name brings up voice \
                         mode, so the answer is spoken and the HUD shows what it heard.",
                    ),
                ]),
            ),
            ui::card_with_header(
                "Voice",
                Some(ui::muted(
                    "Which voice reads a reply aloud. Any Microsoft Edge neural voice id \
                     works offline of a speech backend — en-US-AriaNeural, en-GB-RyanNeural, \
                     en-AU-NatashaNeural.",
                )),
                None,
                ui::stack(vec![
                    ui::field(
                        "Voice id",
                        ui::input(
                            crate::assistant_voice::DEFAULT_VOICE,
                            &app.settings.voice_name,
                            Message::SetVoiceName,
                        ),
                    ),
                    ui::caption(
                        "With a speech backend configured on the server (SPEECH_API_BASE — \
                         Piper, Kokoro, a hosted provider) this is that backend's voice id \
                         instead, which is where a voice you trained yourself goes.",
                    ),
                ]),
            ),
            ui::card_with_header(
                "Voice speed",
                Some(ui::muted(format!("How fast {name} reads a reply aloud, in voice mode."))),
                None,
                ui::stack(vec![
                    ui::segmented(crate::assistant_voice::VOICE_RATES.map(|(label, r)| {
                        (label, rate == r, Message::SetVoiceRate(r))
                    })),
                    ui::caption(format!(
                        "{name} reads while the answer is still being written, so it also \
                         eases off when the model falls behind and picks the pace back up \
                         when text is waiting."
                    )),
                ]),
            ),
        ]),
    )
}

/// Settings → Performance. One knob, and enough of the machine's answer to it
/// that the knob is legible rather than magic (ADR 0010).
fn performance_view(app: &App) -> Element<'_, Message> {
    let picked = app.settings.resource_mode;
    // What the server says it is doing, when it has said anything. `Auto` is the
    // reason this is on screen at all: without it the user picks a mode whose
    // effect they cannot see.
    let live: Element<'_, Message> = match app.resources.as_ref() {
        // Three rows, not one: the badge line says which mode is in force, the
        // meter says how full the lane is, and the caption says it in numbers.
        // The limit is stated once, in the caption — carrying it in the badge
        // row too is what made this card read as two half-sentences.
        Some(r) => ui::stack(vec![
            row![
                ui::badge(
                    match picked {
                        ResourceMode::Auto => format!("Auto \u{2192} {}", r.resolved),
                        _ => r.resolved.clone(),
                    },
                    tier_tone(&r.resolved),
                ),
                ui::muted(ui::count(r.cpus, "core", "cores")),
            ]
            .spacing(space::MD)
            .align_y(iced::Alignment::Center)
            .into(),
            ui::meter(r.background_in_flight, r.background_limit, tier_tone(&r.resolved)),
            ui::caption(format!(
                "{} of {} background calls in flight, {} interactive.",
                r.background_in_flight, r.background_limit, r.interactive_in_flight
            )),
        ])
        .into(),
        None => ui::muted("Waiting for the server.").into(),
    };

    ui::page(
        "Performance",
        Some(ui::muted(
            "How much of this machine the server may spend on model calls. Interactive \
             work — chat, Coder, the assistant — is never queued behind background work \
             whatever this says; the setting is what bounds the background half.",
        )),
        None,
        ui::stack_lg(vec![
            ui::card_with_header(
                "Resource mode",
                Some(ui::muted(match picked {
                    ResourceMode::Auto => {
                        "Turbo while you are at the window, and Eco once you have been away \
                         for a minute. The default."
                    }
                    ResourceMode::Eco => {
                        "One background model call at a time, always. Pick this when you \
                         need the machine for something else."
                    }
                    ResourceMode::Balanced => "Half the machine, whether or not you are watching.",
                    ResourceMode::Turbo => {
                        "As much as is useful, whether or not you are watching. Runs finish \
                         soonest and everything else on the machine feels it."
                    }
                })),
                None,
                ui::stack(vec![
                    ui::segmented(
                        ResourceMode::ALL
                            .map(|m| (m.label(), picked == m, Message::SetResourceMode(m))),
                    ),
                    ui::caption(
                        "This bounds model calls, not the whole process — a build job or a \
                         local model runs at its own pace.",
                    ),
                ]),
            ),
            ui::card_with_header(
                "Right now",
                Some(ui::muted("The same numbers the sidebar monitor draws.")),
                None,
                live,
            ),
        ]),
    )
}

/// Eco is not a warning and Turbo is not an error — the scale is "how loud", so
/// it runs neutral to warning and never reaches danger.
fn tier_tone(tier: &str) -> Tone {
    match tier {
        "turbo" => Tone::Warning,
        "balanced" => Tone::Info,
        _ => Tone::Neutral,
    }
}

/// The sidebar's resource monitor, between the nav list and the utility strip.
///
/// **It must not become the thing it reports on.** So it owns no timer and no
/// sampler: every number here already exists as an atomic in the server, and it
/// is read on the health poll the app was running anyway, at a rate that follows
/// the mode (`App::resource_poll_every` — 20 s in Eco, 5 s in Turbo). Nothing is
/// drawn when the server is not answering, which is also when there is nothing
/// true to say.
///
/// What it shows is the load the user can actually act on: model calls in
/// flight against the limit their setting chose. Host CPU and memory are
/// deliberately absent — sampling them needs a per-platform dependency and a
/// thread that wakes to poll, and the number it produces is one the user cannot
/// do anything about from here.
fn resource_monitor(app: &App) -> Option<Element<'_, Message>> {
    let r = app.resources.as_ref()?;
    let tone = tier_tone(&r.resolved);
    let busy = r.background_in_flight + r.interactive_in_flight;
    let label = if busy == 0 {
        "idle".to_string()
    } else {
        ui::count(busy, "AI call", "AI calls")
    };
    Some(
        // Left-aligned, both labels together. Pushing the tier to the right edge
        // needs a Fill that survives from the sidebar's fixed width down through
        // four nested widgets, and it does not — the innermost row resolves to
        // its minimum and the label lands mid-row looking like a bug. Reading
        // "idle · turbo" as one phrase is what it is anyway.
        container(
            column![
                row![ui::caption(label), ui::mono_toned(r.resolved.clone(), tone)]
                    .spacing(space::SM)
                    .align_y(iced::Alignment::Center),
                ui::meter(r.background_in_flight, r.background_limit, tone),
            ]
            .spacing(space::XS),
        )
        .width(Length::Fill)
        .into(),
    )
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
    blocks.push(startup_card(app));
    blocks.push(local_llm_card(app));
    blocks.push(api_card(app));
    blocks.push(version_card(app));

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
        rows.push(ui::field("Version", ui::body(status.server.clone())));
        rows.push(ui::field("Platform", ui::muted(status.platform.clone())));
    }
    if let Some(err) = &app.status_error {
        rows.push(ui::caption(err.clone()));
    }

    ui::card_with_header(
        "Server",
        Some(ui::muted("The API process this app owns.")),
        actions,
        ui::stack(rows),
    )
}

/// Run at login, and come up without a window.
///
/// The login entry is the per-user `Run` key, not a Windows service: this
/// process is the tray icon and the server host at once, and a service gets no
/// desktop to put a tray icon on. See [`crate::shell::autostart`].
fn startup_card(app: &App) -> Element<'_, Message> {
    ui::card_with_header(
        "Startup",
        Some(ui::muted(
            "Run when you sign in, so the API server is up before you open the window.",
        )),
        None,
        ui::stack(vec![
            ui::toggle(
                if app.autostart { Icon::Check } else { Icon::X },
                if app.autostart { "Starts when you sign in" } else { "Off" },
                app.autostart,
                Message::SetAutostart(!app.autostart),
            ),
            ui::toggle(
                if app.settings.start_minimized { Icon::Monitor } else { Icon::X },
                "Open in the tray, with no window",
                app.settings.start_minimized,
                Message::SetStartMinimized(!app.settings.start_minimized),
            ),
            ui::caption(
                "The login entry always starts in the tray; the second toggle is for when you \
                 launch the app yourself. Either way the server runs and the tray icon opens \
                 the window.",
            ),
        ]),
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
            ui::field("Download", local_model_download(app)),
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

/// The same card in a build that has no engine to configure.
///
/// It renders rather than vanishing on purpose: a card that is simply absent
/// reads as "this app cannot do that", when the truth is one cargo feature —
/// and nothing else in the UI mentions in-process inference exists.
#[cfg(not(feature = "local-llm"))]
fn local_llm_card(_app: &App) -> Element<'_, Message> {
    ui::card_with_header(
        "Local model",
        Some(ui::muted(
            "Answer this app's own chat in-process instead of through the server.",
        )),
        None,
        ui::stack(vec![
            ui::field(
                "State",
                ui::badge_icon(Icon::Info, "Not built into this copy", Tone::Neutral),
            ),
            ui::caption(
                "llama.cpp is linked in behind a cargo feature, off by default because it                  needs an accelerator SDK to be worth running. Rebuild with it to get the                  GGUF picker, the Hugging Face downloader and the VRAM controls:",
            ),
            ui::mono("cargo run -p agent-platform-desktop --features cuda"),
            ui::caption(
                "`--features local-llm` builds without CUDA and runs on the CPU — measured                  at 11 tok/s against 123 on the GPU, so it is a fallback, not a default.                  Until then every turn goes to the server and its providers.",
            ),
        ]),
    )
}

/// The Hugging Face row on the same card: paste a reference, get a GGUF.
///
/// No search and no browse — the model card in the browser is a better catalog
/// than anything this could draw, and what it gives you is a link to paste.
#[cfg(feature = "local-llm")]
fn local_model_download(app: &App) -> Element<'_, Message> {
    let mut bar = vec![
        container(ui::input_submit(
            "owner/repo/model-Q4_K_M.gguf, or a Hugging Face link",
            &app.model_dl.input,
            Message::SetModelUrl,
            Message::DownloadModel,
        ))
        .width(Length::Fill)
        .into(),
    ];
    if app.model_dl.active {
        bar.push(ui::badge("downloading…", Tone::Info));
        // The one control that matters on a 20 GB transfer: the way out.
        bar.push(ui::button_ghost(Icon::X, "Cancel", Message::CancelModelDownload));
    } else {
        bar.push(ui::button_secondary(Icon::Download, "Get", Message::DownloadModel));
    }
    let mut rows = vec![ui::cluster(bar).into()];

    if app.model_dl.active {
        let got = crate::model_download::human(app.model_dl.received);
        rows.push(ui::caption(match app.model_dl.total {
            // The server may not send a length; a bare byte count still moves.
            None => format!("{got} so far"),
            Some(total) => format!(
                "{got} of {} ({}%)",
                crate::model_download::human(total),
                app.model_dl.received * 100 / total.max(1)
            ),
        }));
    }
    if let Some(e) = &app.model_dl.error {
        rows.push(ui::alert_error(e.clone()));
    }
    ui::stack(rows).into()
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

/// This build's version, and whether a newer one has been published.
///
/// The check is a button, never a poll: an app that phones GitHub on every
/// launch is one the user did not ask for, and this one runs offline by design.
/// There is no install button — see [`crate::update_check`] for why.
fn version_card(app: &App) -> Element<'_, Message> {
    let state = &app.update_check;
    let mut rows: Vec<Element<'_, Message>> =
        vec![ui::field("This build", ui::mono(crate::update_check::current()))];

    if let Some(error) = &state.error {
        rows.push(ui::alert_warning(error.clone()));
    } else if let Some(newer) = &state.newer {
        rows.push(ui::alert(
            Tone::Info,
            format!("Version {newer} is available"),
            Some(ui::muted("Download it from the releases page and unzip over this install.")),
        ));
    } else if state.checked {
        rows.push(ui::muted("Up to date."));
    }

    rows.push(
        ui::cluster(vec![
            ui::button_secondary(
                Icon::Refresh,
                if state.checking { "Checking…" } else { "Check for updates" },
                Message::CheckForUpdate,
            ),
            ui::button_ghost(
                Icon::FolderOpen,
                "Open releases",
                Message::RevealPath(crate::update_check::RELEASES_PAGE.to_string()),
            ),
        ])
        .into(),
    );

    ui::card_with_header(
        "Version",
        Some(ui::muted("Checked only when you ask; nothing here contacts GitHub on its own.")),
        None,
        ui::stack(rows),
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
    let selected = app.logs.selected.len();
    let matched: Vec<(u64, &String)> = app
        .logs
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| app.logs.shows(l))
        .map(|(i, l)| (app.logs.base + i as u64, l))
        .collect();

    use crate::logs::Level;
    let mut toolbar = vec![
        container(ui::input_icon(Icon::Search, "Filter lines…", &app.logs.filter, Message::LogFilterChanged))
            .width(320)
            .into(),
        ui::chips(vec![
            ("All", app.logs.level.is_none(), Message::SetLogLevel(None)),
            ("Info+", app.logs.level == Some(Level::Info), Message::SetLogLevel(Some(Level::Info))),
            ("Warn+", app.logs.level == Some(Level::Warn), Message::SetLogLevel(Some(Level::Warn))),
            ("Errors", app.logs.level == Some(Level::Error), Message::SetLogLevel(Some(Level::Error))),
        ]),
        ui::button_secondary(
            if app.logs.paused { Icon::Play } else { Icon::Pause },
            if app.logs.paused { "Resume" } else { "Pause" },
            Message::ToggleLogsPaused,
        ),
        // Copy is the primary action here — a log line is something you paste
        // into an issue, not something you read once.
        ui::button_default(
            Icon::Copy,
            if selected > 0 { "Copy selected" } else { "Copy shown" },
            Message::CopyLogs,
        ),
    ];
    if selected > 0 {
        toolbar.push(ui::badge(ui::count(selected, "line selected", "lines selected"), Tone::Info));
        toolbar.push(ui::button_ghost(Icon::XCircle, "Deselect", Message::ClearLogSelection));
    } else {
        toolbar.push(ui::button_ghost(Icon::ListChecks, "Select all", Message::SelectAllLogs));
    }
    toolbar.push(ui::button_ghost(Icon::Trash, "Clear", Message::ClearLogs));
    toolbar.push(ui::spacer());
    // A trace jump lands here with a filter the user did not type — say how much
    // is hidden, and offer the way back out.
    if app.logs.filtering() {
        toolbar.push(ui::badge(
            format!("{} of {}", matched.len(), app.logs.lines.len()),
            Tone::Info,
        ));
        toolbar.push(ui::button_ghost(Icon::XCircle, "Clear filter", Message::ClearLogFilter));
    }
    toolbar.push(ui::badge(
        if app.shell.attached { "server log" } else { "process output" },
        Tone::Neutral,
    ));
    // Wraps: the filter chips and the "N of M" badge push this past the window
    // width, and the clipped item is the one that says what is hidden.
    let toolbar = ui::cluster(toolbar).wrap();

    // Only the tail is rendered: iced lays out every child, and the ring holds
    // thousands of lines.
    let tail = &matched[matched.len().saturating_sub(500)..];

    let body: Element<'_, Message> = if tail.is_empty() {
        ui::empty_state_icon(
            Icon::Scroll,
            if app.logs.lines.is_empty() {
                "No output yet.".to_string()
            } else if app.logs.dropped > 0 {
                // The common miss after a trace jump: the request logged, then
                // scrolled out of the ring. "No match" alone reads as "never
                // happened", which sends people looking in the wrong place.
                format!(
                    "No lines match — {} earlier lines were dropped when the buffer wrapped.",
                    app.logs.dropped
                )
            } else {
                "No lines match the filter.".to_string()
            },
        )
    } else {
        let mut lines = column![].spacing(1);
        if app.logs.dropped > 0 {
            lines = lines.push(ui::caption(format!(
                "… {} earlier lines dropped (buffer wrapped)",
                app.logs.dropped
            )));
        }
        for (id, line) in tail {
            lines = lines.push(ui::list_item_compact(
                log_entry(line),
                app.logs.selected.contains(id),
                Message::ToggleLogLine(*id),
            ));
        }
        scrollable(lines).height(Length::Fill).anchor_bottom().into()
    };

    // `page_fixed`, not `page`: the log tail scrolls itself. Inside the outer
    // scrollable the two bars stacked, and the right-hand badge sat under the
    // outer one, clipped.
    ui::page_fixed(
        "Logs",
        Some(ui::muted(
            "Server output, including startup and migrations — visible before the API answers.              Click lines to select them, then copy.",
        )),
        None,
        column![toolbar, ui::code(body)].spacing(space::MD).height(Length::Fill),
    )
}

/// Column widths for a log row: level pill, clock, source. The message takes
/// what is left, and a line's extra fields hang under it at [`FIELD_INDENT`].
const LEVEL_W: f32 = 62.0;
const TIME_W: f32 = 74.0;
const SOURCE_W: f32 = 140.0;
const FIELD_INDENT: f32 = LEVEL_W + TIME_W + SOURCE_W + 3.0 * space::XS;

/// One line as columns — level pill, clock, source, message — with whatever
/// structured fields it carried on a second, muted row.
///
/// ponytail: parsed per frame rather than at ingest, because only the rendered
/// tail (500 lines) pays for it. Parse into `LogsState` if the frame time shows.
fn log_entry<'a>(line: &str) -> Element<'a, Message> {
    let entry = crate::logs::parse(line);
    let tone = row_tone(&entry);
    let head = row![
        container(match entry.level {
            Some(level) => ui::badge_icon(level_icon(level), level.label(), tone),
            None => ui::caption(""),
        })
        .width(LEVEL_W),
        container(ui::caption(entry.time.unwrap_or_default())).width(TIME_W),
        container(match entry.source {
            Some(source) => ui::badge(source, Tone::Neutral),
            None => ui::caption(""),
        })
        .width(SOURCE_W),
        match tone {
            Tone::Neutral => ui::mono(entry.message),
            t => ui::mono_toned(entry.message, t),
        },
    ]
    .spacing(space::XS)
    .align_y(iced::Alignment::Center);

    if entry.fields.is_empty() {
        return head.into();
    }
    // A trace id is the one field worth clicking: it collapses the whole log to
    // the one request, which is the same thing "View logs" does from an error
    // banner — reached from a line instead of from a failure.
    let mut chips: Vec<Element<'a, Message>> = Vec::new();
    let mut plain: Vec<String> = Vec::new();
    for (k, v) in &entry.fields {
        if is_trace_key(k) {
            chips.push(ui::badge_button(
                format!("{k} {v}"),
                Tone::Info,
                Message::LogFilterChanged(v.clone()),
            ));
        } else {
            plain.push(format!("{k}: {v}"));
        }
    }
    if !plain.is_empty() {
        chips.push(ui::caption(plain.join("    ")));
    }
    column![
        head,
        container(ui::cluster(chips)).padding(Padding::default().left(FIELD_INDENT))
    ]
    .spacing(2)
    .into()
}

/// Field names that identify a request rather than describe it — the ones worth
/// a click. Both servers write `trace_id`; `request_id` is what the older
/// Python-side middleware called the same value.
fn is_trace_key(key: &str) -> bool {
    matches!(key, "trace_id" | "request_id" | "trace")
}

/// The row's color says what the level filter says — one notion of severity, so
/// "Errors" can never hide a row painted red.
fn row_tone(entry: &crate::logs::Entry) -> Tone {
    entry.severity().map_or(Tone::Neutral, level_tone)
}

fn level_icon(level: crate::logs::Level) -> Icon {
    match level {
        crate::logs::Level::Error => Icon::XCircle,
        crate::logs::Level::Warn => Icon::Alert,
        _ => Icon::Info,
    }
}

/// INFO and DEBUG stay neutral: nearly every line is one, and a page of colored
/// pills hides the two that matter.
fn level_tone(level: crate::logs::Level) -> Tone {
    match level {
        crate::logs::Level::Error => Tone::Danger,
        crate::logs::Level::Warn => Tone::Warning,
        _ => Tone::Neutral,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::parse;

    /// The whole point of `row_tone`: the line that says a request failed is
    /// logged at INFO, so the level alone would paint it the same grey as a
    /// health check.
    #[test]
    fn a_failed_request_is_toned_by_its_status_not_its_level() {
        let five_hundred = parse(
            r#"{"level": "INFO", "message": "request completed", "status_code": 500, "trace_id": "abc"}"#,
        );
        assert_eq!(row_tone(&five_hundred), Tone::Danger);

        let four_oh_four =
            parse(r#"{"level": "INFO", "message": "request completed", "status_code": 404}"#);
        assert_eq!(row_tone(&four_oh_four), Tone::Warning);

        let ok = parse(r#"{"level": "INFO", "message": "request completed", "status_code": 200}"#);
        assert_eq!(row_tone(&ok), Tone::Neutral, "a 200 stays quiet");

        // No status to go on: the level still decides.
        assert_eq!(row_tone(&parse(r#"{"level": "ERROR", "message": "boom"}"#)), Tone::Danger);
        assert_eq!(row_tone(&parse("Application startup complete.")), Tone::Neutral);

        // A non-numeric status must not panic or swallow the level.
        let odd = parse(r#"{"level": "ERROR", "message": "boom", "status": "pending"}"#);
        assert_eq!(row_tone(&odd), Tone::Danger);
    }

    /// A trace id gets a chip, everything else stays in the muted field line.
    /// Splitting them wrong is how a row ends up with no way to pivot.
    #[test]
    fn only_trace_ids_become_chips() {
        let entry = parse(
            r#"{"level": "INFO", "message": "request completed", "trace_id": "abc", "request_id": "def", "duration_ms": 2}"#,
        );
        let (chips, plain): (Vec<_>, Vec<_>) =
            entry.fields.iter().partition(|(k, _)| is_trace_key(k));
        assert_eq!(
            chips.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect::<Vec<_>>(),
            vec![("request_id", "def"), ("trace_id", "abc")]
        );
        assert_eq!(plain.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), vec!["duration_ms"]);
    }

    #[test]
    fn nav_groups_stay_short_and_home_is_first() {
        for (name, entries) in NAV {
            assert!(
                entries.len() <= 4,
                "{name} has {} entries — split the group rather than dump",
                entries.len()
            );
        }
        let labels: Vec<&str> = NAV
            .iter()
            .flat_map(|(_, entries)| entries.iter().map(|(_, _, label)| *label))
            .collect();
        assert_eq!(labels[0], "Home");
        assert!(!labels.contains(&"Dashboard"));
        assert!(labels.contains(&"Processes"));
        assert!(labels.contains(&"Assistants"));
    }
}
