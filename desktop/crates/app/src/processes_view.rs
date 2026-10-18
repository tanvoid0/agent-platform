//! Processes screen rendering — composed from the shadcn-style `ui` kit.

use crate::domain::truncate;
use crate::domain::{self, BoardColumn, BoardRow};
use crate::processes::{Message, State, ViewMode};
use crate::ui::{self, space, Icon, Tone};
use agent_platform_client::types::{ProcessRecord, ReviewDecision, TaskNodeRecord};
use iced::widget::{column, container, markdown, row, scrollable, text_editor, Row};
use iced::{Element, Length, Theme};

pub fn view<'a>(state: &'a State, iced_theme: &Theme) -> Element<'a, Message> {
    let main = row![
        run_list(state),
        ui::separator_vertical(),
        container(detail_pane(state, iced_theme)).width(Length::Fill).height(Length::Fill),
    ];

    // Overlays, shadcn `Dialog`-style. Reject wins: it is raised from inside
    // the review modal as well as from the board.
    if let Some(task_id) = state.confirm_reject {
        return ui::modal(main, reject_confirm(task_id), 480.0);
    }
    match &state.review {
        None => main.into(),
        Some(draft) => ui::modal(main, review_modal(state, draft), 720.0),
    }
}

// ---------------------------------------------------------------------------
// Left: composer + run list
// ---------------------------------------------------------------------------

/// Start-a-run form. Shared with Home so the inbox is not a dead end.
pub(crate) fn new_run_composer(state: &State) -> Element<'_, Message> {
    let team_names: Vec<String> = state.teams.iter().map(|t| t.name.clone()).collect();
    let selected_team = state
        .composer
        .team_id
        .and_then(|id| state.teams.iter().find(|t| t.id == id))
        .map(|t| t.name.clone());

    let mut project_names = vec![UNASSIGNED.to_string()];
    project_names.extend(state.projects.iter().map(|p| p.name.clone()));
    let selected_project = state
        .composer
        .project_id
        .and_then(|id| state.projects.iter().find(|p| p.id == id))
        .map(|p| p.name.clone())
        .or_else(|| Some(UNASSIGNED.to_string()));

    let teams_by_name: Vec<(String, i64)> =
        state.teams.iter().map(|t| (t.name.clone(), t.id)).collect();
    let projects_by_name: Vec<(String, i64)> =
        state.projects.iter().map(|p| (p.name.clone(), p.id)).collect();

    ui::card(ui::stack(vec![
            ui::heading("New run"),
            ui::caption("A goal, a team, then a plan you approve — unless auto-approve is on."),
        ui::input_icon(
            Icon::Sparkles,
            "What should the team accomplish?",
            &state.composer.goal,
            Message::GoalChanged,
        ),
        ui::select("Team template", team_names, selected_team, move |name: String| {
            let id = teams_by_name
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, id)| *id)
                .unwrap_or_default();
            Message::TeamPicked(id)
        }),
        ui::select("Project", project_names, selected_project, move |name: String| {
            Message::ProjectPicked(
                projects_by_name.iter().find(|(n, _)| *n == name).map(|(_, id)| *id),
            )
        }),
        ui::stack(vec![
            ui::checkbox(
                "Auto-approve the plan",
                state.composer.auto_approve,
                Message::ToggleAutoApprove,
            ),
            ui::caption("Skips the plan gate. Task review still pauses the run."),
        ])
        .into(),
        // A fresh install has no teams, and a run needs one. Without this the
        // picker was simply empty and Start run answered "Pick a team first."
        // about a list that had nothing in it.
        if state.teams.is_empty() {
            ui::alert(
                Tone::Info,
                "No teams yet",
                Some(
                    ui::cluster(vec![
                        ui::muted("A run needs a team. Start from one of the built-in templates."),
                        ui::button_secondary(Icon::Users, "Open Teams", Message::OpenTeams),
                    ])
                    .into(),
                ),
            )
        } else if state.composer.submitting {
            ui::button_sized(
                Some(Icon::Clock),
                "Starting…",
                ui::ButtonVariant::Default,
                ui::Size::Sm,
                None,
            )
        } else {
            ui::button_default(Icon::Play, "Start run", Message::Submit)
        },
    ]))
}

fn run_list(state: &State) -> Element<'_, Message> {
    let composer = new_run_composer(state);
    let visible = state.visible_processes();

    let items: Vec<Element<'_, Message>> = visible
        .iter()
        .map(|p| run_list_item(p, state.selected == Some(p.id)))
        .collect();

    let list: Element<'_, Message> = if items.is_empty() {
        if state.processes.is_empty() {
            ui::empty_state_icon(Icon::Activity, "No runs yet. Start one above.")
        } else {
            ui::empty_state_icon(Icon::Search, "No runs match this filter.")
        }
    } else {
        scrollable(ui::stack(items)).height(Length::Fill).into()
    };

    let header = ui::stack(vec![
        ui::cluster(vec![
            ui::caption("Recent runs"),
            ui::spacer(),
            ui::caption(format!("{} of {}", visible.len(), state.processes.len())),
        ])
        .into(),
        ui::input_icon(
            Icon::Search,
            "Filter runs…",
            &state.run_search,
            Message::RunSearchChanged,
        ),
        ui::chips(
            crate::processes::RunScope::ALL
                .map(|s| (s.label(), state.run_scope == s, Message::SetRunScope(s))),
        ),
    ]);

    container(column![composer, header, list].spacing(space::MD).padding(space::MD))
    .width(340)
    .height(Length::Fill)
    .into()
}

const UNASSIGNED: &str = "Unassigned";

fn run_list_item(p: &ProcessRecord, selected: bool) -> Element<'_, Message> {
    let when = domain::relative_time(&p.created_at).unwrap_or_default();
    let mut lines = vec![
        ui::cluster(vec![
            ui::badge(
                domain::process_status_label(p.status.as_str()),
                domain::process_status_tone(p.status.as_str()),
            ),
            ui::caption(format!("#{}", p.id)),
            ui::spacer(),
            ui::caption(when),
        ])
        .into(),
        ui::body(truncate(&p.goal, 90)),
    ];
    if let Some(hint) = domain::process_waiting_hint(p.status.as_str()) {
        lines.push(ui::caption(hint));
    }
    ui::list_item(ui::stack(lines), selected, Message::Select(p.id))
}

// ---------------------------------------------------------------------------
// Right: detail pane
// ---------------------------------------------------------------------------

fn detail_pane<'a>(state: &'a State, iced_theme: &Theme) -> Element<'a, Message> {
    let mut blocks: Vec<Element<'_, Message>> = Vec::new();

    if let Some(err) = &state.error {
        blocks.push(ui::error_bar(err, Message::TraceLogs, Message::DismissNotice, Vec::new()));
    }

    let Some(process) = state.selected_process() else {
        blocks.push(if state.selected.is_some() {
            ui::empty_state_icon(Icon::Clock, "Loading run…")
        } else {
            ui::empty_state_icon(Icon::Activity, "Pick a run from the list, or start a new one.")
        });
        return ui::page(
            "Processes",
            Some(ui::muted("A team run: a goal, a plan you approve, then the work.")),
            None,
            ui::stack_lg(blocks),
        );
    };

    blocks.push(summary_card(state, process));
    blocks.push(
        ui::segmented(ViewMode::ALL.map(|m| (m.label(), state.view == m, Message::SetView(m))))
    );
    blocks.push(match state.view {
        ViewMode::Graph => graph_view(state),
        ViewMode::Board => board_view(state),
        ViewMode::Timeline => timeline_view(state),
        ViewMode::Events => events_view(state, iced_theme),
    });
    if let Some(uuid) = &state.inspecting {
        blocks.push(inspector(state, uuid, iced_theme));
    }
    if state.chat_open {
        blocks.extend(chat_card(state, iced_theme));
    }

    ui::page(
        format!("Run #{}", process.id),
        Some(ui::muted(process.goal.clone())),
        Some(actions_row(state, process)),
        ui::stack_lg(blocks),
    )
}

fn actions_row<'a>(state: &'a State, process: &'a ProcessRecord) -> Element<'a, Message> {
    let status = process.status.as_str();
    let mut buttons: Vec<Element<'a, Message>> = Vec::new();

    // While a request is in flight every action is withheld, not just labelled:
    // a second Retry click started a second run of the same plan.
    let gated = |variant: ui::ButtonVariant, glyph: Icon, label: String, msg: Message| {
        ui::button_sized(
            Some(glyph),
            label,
            variant,
            ui::Size::Sm,
            (!state.busy).then_some(msg),
        )
    };

    if state.busy {
        buttons.push(ui::badge("working…", Tone::Info));
    }
    if status == "approval_required" {
        buttons.push(gated(ui::ButtonVariant::Default, Icon::Check, "Approve plan".into(), Message::Approve));
    }
    let waiting = state.awaiting_review_task_ids().len();
    if waiting > 0 {
        buttons.push(gated(
            ui::ButtonVariant::Default,
            Icon::Check,
            format!("Approve all ({waiting})"),
            Message::ApproveAllReviews,
        ));
    }
    if matches!(status, "pending" | "planning" | "approved" | "running" | "approval_required") {
        buttons.push(gated(ui::ButtonVariant::Destructive, Icon::X, "Cancel".into(), Message::Cancel));
    }
    if matches!(status, "failed" | "cancelled" | "completed") {
        buttons.push(gated(ui::ButtonVariant::Secondary, Icon::RotateCcw, "Retry".into(), Message::Retry));
    }
    // Not gated on status: the flag is read at the *next* gate, so arming it on
    // a run that is already past one is the point.
    if let Some(process) = state.selected_process() {
        let on = process.auto_approve;
        buttons.push(gated(
            if on { ui::ButtonVariant::Secondary } else { ui::ButtonVariant::Outline },
            Icon::Check,
            if on { "Auto-approve: on".into() } else { "Auto-approve: off".into() },
            Message::SetAutoApprove(!on),
        ));
    }
    // Not on a terminal run: the server's `SyncBranch::Terminal` answers 200 with
    // "sync does not apply", which the toast then renders as a success. Retry is
    // the button that does something there, and it is already beside this one.
    if !matches!(status, "failed" | "cancelled" | "completed") {
        buttons.push(gated(ui::ButtonVariant::Outline, Icon::Refresh, "Sync".into(), Message::Sync));
    }
    buttons.push(ui::button_ghost(Icon::Download, "Export", Message::Export));
    buttons.push(ui::button_ghost(
        Icon::Message,
        if state.chat_open { "Hide chat" } else { "Ask about this" },
        Message::ToggleChat,
    ));
    ui::cluster(buttons).into()
}

/// A chat thread scoped to what is on screen: the inspected subagent if one is
/// open, otherwise the run. Switching scope switches thread rather than
/// carrying one conversation across unrelated records.
fn chat_card<'a>(state: &'a State, iced_theme: &Theme) -> Option<Element<'a, Message>> {
    let thread = state.chat_key().and_then(|k| state.chats.get(&k))?;
    let (subtitle, hint) = match &state.inspecting {
        Some(uuid) => (
            format!("Scoped to subagent {}", domain::short_uuid(uuid)),
            "Ask about this subagent's task.",
        ),
        None => ("Scoped to this run".to_string(), "Ask about this run's plan, tasks or failure."),
    };

    Some(ui::card_with_header(
        "Chat",
        Some(ui::muted(subtitle)),
        Some(ui::button_ghost(Icon::X, "Close", Message::ToggleChat)),
        // Capped: this card lives inside the detail pane's own scroll, so a Fill
        // transcript would fight it.
        crate::chat_view::panel(thread, iced_theme, hint, Length::Fixed(280.0))
            .map(Message::Chat),
    ))
}

fn summary_card<'a>(state: &'a State, process: &'a ProcessRecord) -> Element<'a, Message> {
    let mut stats = vec![
        ui::stat(Icon::Activity, "Status", domain::process_status_label(process.status.as_str()).to_string()),
        ui::stat(Icon::Cpu, "Tokens", process.total_tokens.to_string()),
        ui::stat(Icon::Gauge, "Cost", format!("${:.4}", process.total_cost)),
    ];
    if let Some(tools) = process.tool_invocations_used {
        stats.push(ui::stat(Icon::Settings, "Tool calls", tools.to_string()));
    }
    if let Some(detail) = &state.detail {
        stats.push(ui::stat(Icon::Scroll, "Tasks", detail.tasks.len().to_string()));
    }
    if let Some(elapsed) = domain::process_elapsed(process) {
        stats.push(ui::stat(Icon::Clock, "Elapsed", elapsed));
    }

    let mut rows = vec![ui::cluster(stats).into()];
    // How far along the plan is, without counting cards on the board.
    if let Some(detail) = &state.detail {
        let total = detail.tasks.len();
        let done = detail.tasks.iter().filter(|t| t.status == "completed").count();
        if total > 0 {
            let tone = if detail.tasks.iter().any(|t| t.status == "failed") {
                Tone::Danger
            } else if done == total {
                Tone::Success
            } else {
                Tone::Info
            };
            rows.push(
                ui::cluster(vec![
                    ui::caption(format!("{done}/{total} tasks")),
                    container(ui::meter(done, total, tone)).width(Length::Fill).into(),
                ])
                .into(),
            );
        }
    }
    if let Some(reason) = &process.failure_reason {
        rows.push(ui::alert(Tone::Danger, "Failure", Some(ui::mono(reason.clone()))));
        // Most runs die on the LLM call — a missing key, a model the account
        // cannot reach. The banner used to state that and stop; Providers is
        // the page that fixes it, so it is one click from here.
        rows.push(
            ui::button_secondary(Icon::Settings, "Check providers", Message::OpenSettings),
        );
    }
    if let Some(when) = domain::relative_time(&process.updated_at) {
        rows.push(ui::caption(format!("Updated {when}")));
    }
    ui::stack(rows).into()
}

// ---------------------------------------------------------------------------
// Graph
// ---------------------------------------------------------------------------

fn graph_view(state: &State) -> Element<'_, Message> {
    let layout = state.graph_layout();
    if layout.nodes.is_empty() {
        return ui::card(ui::empty_state_icon(Icon::Clock, "No plan yet — the graph appears once planning finishes."));
    }

    let mut controls: Vec<Element<'_, Message>> = Vec::new();
    // The lineage filter only means something once a sub-DAG has nested tasks.
    let tasks = state.detail.as_ref().map(|d| d.tasks.as_slice()).unwrap_or_default();
    if crate::graph::max_lineage_depth(tasks) > 0 {
        controls.push(ui::caption("Show"));
        controls.push(ui::segmented(
            crate::graph::Lineage::ALL.map(|l| (l.label(), state.lineage == l, Message::SetLineage(l))),
        ));
    }
    controls.push(ui::spacer());
    controls.push(ui::caption(format!(
        "{} nodes · drag to pan, scroll to zoom",
        layout.nodes.len()
    )));
    // Panning past the last node leaves an empty canvas with no way back.
    controls.push(ui::button_ghost(Icon::Refresh, "Reset view", Message::ResetViewport));
    let toolbar: Element<'_, Message> = ui::cluster(controls).into();

    let canvas = iced::widget::canvas(crate::graph::DagCanvas {
        layout,
        viewport: state.viewport,
        selected: state.inspecting.clone(),
    })
    .width(Length::Fill)
    .height(420);

    ui::card(column![toolbar, ui::separator(), canvas].spacing(space::MD))
}

// ---------------------------------------------------------------------------
// Board
// ---------------------------------------------------------------------------

fn board_toolbar(state: &State) -> Element<'_, Message> {
    ui::cluster(vec![
        container(ui::input(
            "Search subagents…",
            &state.board_search,
            Message::BoardSearchChanged,
        ))
        .width(280)
        .into(),
        ui::checkbox(
            "Needs attention",
            state.needs_attention_only,
            Message::ToggleNeedsAttention,
        ),
    ])
    .into()
}

fn board_view(state: &State) -> Element<'_, Message> {
    let columns: Vec<Element<'_, Message>> = BoardColumn::ALL
        .iter()
        .map(|col| {
            let rows = state.rows_in_column(*col);
            let count = rows.len();
            let folded = state.collapsed.contains(col);
            let cards: Vec<Element<'_, Message>> = if folded {
                vec![ui::caption(ui::count(count, "card", "cards"))]
            } else if rows.is_empty() {
                vec![ui::caption("—")]
            } else {
                let inspecting = state.inspecting.clone();
                rows.into_iter().map(|r| board_card(inspecting.as_deref(), r)).collect()
            };
            // The header is the fold handle — a run with fifty completed nodes
            // pushed every live column into a scroll nobody wanted.
            let header = ui::list_item_compact(
                ui::cluster(vec![
                    ui::badge(col.label(), col.tone()),
                    ui::caption(count.to_string()),
                    ui::spacer(),
                    ui::caption(if folded { "+" } else { "–" }),
                ]),
                folded,
                Message::ToggleColumn(*col),
            );
            container(ui::stack(vec![header, ui::stack(cards).into()]).width(Length::Fill))
                .width(Length::FillPortion(1))
                .into()
        })
        .collect();

    ui::card(
        column![board_toolbar(state), ui::separator(), row(columns).spacing(space::SM)]
            .spacing(space::MD),
    )
}

/// One card on the board. The role alone does not say what the node is *for* —
/// a plan with three "Documentation Writer" nodes was three identical cards
/// over a truncated uuid (`unboring...`, whatever the planner invented), and
/// telling them apart meant clicking each one. The instruction is what
/// distinguishes them, so it is on the card.
fn board_card<'a>(inspecting: Option<&str>, row: BoardRow) -> Element<'a, Message> {
    let uuid = row.subagent.client_uuid.clone();
    let selected = inspecting == Some(uuid.as_str());
    let mut lines = vec![ui::body(row.subagent.role.clone())];

    let instructions = row.subagent.instructions.trim();
    if !instructions.is_empty() {
        lines.push(ui::caption(domain::truncate(instructions, 110)));
    }

    // The footer line: what this node is costing and when it last moved. Both
    // are facts you want while scanning a column, not after opening a card.
    let mut facts: Vec<Element<'a, Message>> = Vec::new();
    if let Some(task) = &row.task {
        if task.tokens_used > 0 {
            facts.push(ui::badge(format!("{} tok", task.tokens_used), Tone::Neutral));
        }
        if task.revision_count.unwrap_or(0) > 0 {
            facts.push(ui::badge(
                ui::count(task.revision_count.unwrap_or(0) as usize, "revision", "revisions"),
                Tone::Warning,
            ));
        }
    }
    if row.subagent.requires_review.unwrap_or(false) {
        facts.push(ui::badge("review gate", Tone::Info));
    }
    if let Some(model) = row.subagent.model.as_deref().filter(|m| !m.trim().is_empty()) {
        facts.push(ui::badge(model.to_string(), Tone::Neutral));
    }
    match domain::relative_task_activity(&row) {
        Some(activity) => facts.push(ui::caption(activity)),
        // Before it starts there is no activity to report, and the uuid is the
        // only other handle on the node — kept, but short and last.
        None => facts.push(ui::caption(domain::short_uuid(&uuid))),
    }
    lines.push(ui::wrap_row(facts));

    if row.column == BoardColumn::AwaitingReview {
        if let Some(task) = &row.task {
            // Approve keeps the agent's own output — the modal is for when you
            // want to edit it, request changes, or reject.
            lines.push(
                ui::cluster(vec![
                    ui::button_default(Icon::Check, "Approve", Message::ApproveTask(task.id)),
                    ui::button_secondary(Icon::Eye, "Review", Message::OpenReview(task.id)),
                    ui::icon_tip(Icon::X, "Reject", Message::RejectTask(task.id)),
                ])
                .into(),
            );
        }
    }
    ui::list_item(ui::stack(lines), selected, Message::Inspect(Some(uuid)))
}

// ---------------------------------------------------------------------------
// Timeline
// ---------------------------------------------------------------------------

fn timeline_view(state: &State) -> Element<'_, Message> {
    let dag = state.dag();
    let tasks: &[TaskNodeRecord] =
        state.detail.as_ref().map(|d| d.tasks.as_slice()).unwrap_or_default();
    let rows = domain::build_timeline_rows(dag.as_ref(), tasks);

    if rows.is_empty() {
        return ui::card(ui::empty_state_icon(Icon::Clock, "No plan yet — the timeline appears once planning finishes."));
    }

    let mut waves: Vec<Element<'_, Message>> = Vec::new();
    let mut current = usize::MAX;
    let mut bucket: Vec<Element<'_, Message>> = Vec::new();
    // A wave runs in parallel, so its wall clock is its slowest task, not the sum.
    let mut slowest: i64 = 0;
    for tr in &rows {
        if tr.wave_index != current {
            if !bucket.is_empty() {
                let elapsed = (slowest > 0).then(|| domain::compact_duration(slowest));
                waves.push(wave_block(current, std::mem::take(&mut bucket), elapsed));
            }
            slowest = 0;
            current = tr.wave_index;
        }
        // Same facts the board carries, so switching tabs does not lose them.
        let task = state.task_by_uuid(&tr.client_uuid);
        let mut line: Vec<Element<'_, Message>> = vec![
            ui::badge(tr.column.label(), tr.column.tone()),
            ui::body(tr.role.clone()),
            ui::spacer(),
        ];
        if let Some(task) = task {
            if let Some(secs) = domain::task_duration_secs(task) {
                slowest = slowest.max(secs);
                line.push(ui::badge_icon(Icon::Clock, domain::compact_duration(secs), Tone::Neutral));
            }
            if task.tokens_used > 0 {
                line.push(ui::badge(format!("{} tok", task.tokens_used), Tone::Neutral));
            }
            if task.revision_count.unwrap_or(0) > 0 {
                line.push(ui::badge(
                    ui::count(task.revision_count.unwrap_or(0) as usize, "revision", "revisions"),
                    Tone::Warning,
                ));
            }
        }
        line.push(ui::caption(domain::short_uuid(&tr.client_uuid)));
        bucket.push(ui::list_item(
            ui::cluster(line),
            state.inspecting.as_deref() == Some(tr.client_uuid.as_str()),
            Message::Inspect(Some(tr.client_uuid.clone())),
        ));
    }
    if !bucket.is_empty() {
        let elapsed = (slowest > 0).then(|| domain::compact_duration(slowest));
        waves.push(wave_block(current, bucket, elapsed));
    }

    ui::card(ui::stack_lg(waves))
}

fn wave_block<'a>(index: usize, rows: Vec<Element<'a, Message>>, elapsed: Option<String>) -> Element<'a, Message> {
    let count = ui::count(rows.len(), "task", "tasks");
    ui::stack(vec![
        ui::cluster(match elapsed {
            Some(e) => vec![
                ui::badge(format!("WAVE {}", index + 1), Tone::Info),
                ui::caption(count),
                ui::spacer(),
                ui::badge_icon(Icon::Clock, e, Tone::Neutral),
            ],
            None => vec![ui::badge(format!("WAVE {}", index + 1), Tone::Info), ui::caption(count)],
        })
        .into(),
        ui::stack(rows).into(),
    ])
    .into()
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

fn events_view<'a>(state: &'a State, iced_theme: &Theme) -> Element<'a, Message> {
    let filter = state.event_filter.to_lowercase();
    let matched: Vec<&agent_platform_client::types::EventLogRecord> = state
        .events
        .iter()
        .filter(|e| {
            filter.is_empty()
                || e.event_type.to_lowercase().contains(&filter)
                || e.content.to_lowercase().contains(&filter)
        })
        .collect();

    let body: Element<'a, Message> = if matched.is_empty() {
        if state.events.is_empty() {
            ui::empty_state_icon(Icon::Scroll, "No events yet.")
        } else {
            ui::empty_state_icon(Icon::Search, "No events match this filter.")
        }
    } else {
        // Newest last, tail-rendered: iced lays out every child.
        const TAIL: usize = 400;
        let tail = &matched[matched.len().saturating_sub(TAIL)..];
        let rows: Vec<Element<'a, Message>> = tail
            .iter()
            .map(|e| {
                let open = state.event_open == Some(e.id);
                ui::list_item(
                    ui::stack(vec![event_meta(e).into(), ui::muted(event_summary(&e.content))]),
                    open,
                    Message::OpenEvent((!open).then_some(e.id)),
                )
            })
            .collect();
        // No silent truncation: say so when older events are off the end, and
        // say it about the buffer too — those are gone until the next Export.
        let dropped = state.events_trimmed;
        let list: Element<'a, Message> = if matched.len() > TAIL || dropped > 0 {
            let mut note = Vec::new();
            if matched.len() > TAIL {
                note.push(format!(
                    "Showing the newest {TAIL} of {} matching events",
                    matched.len()
                ));
            }
            if dropped > 0 {
                note.push(format!(
                    "{} older {} dropped from memory — Export has the full log",
                    dropped,
                    if dropped == 1 { "event was" } else { "events were" }
                ));
            }
            column![
                ui::caption(note.join(" · ")),
                scrollable(ui::stack(rows)).spacing(space::SM).height(420).anchor_bottom(),
            ]
            .spacing(space::XS)
            .into()
        } else {
            scrollable(ui::stack(rows)).spacing(space::SM).height(420).anchor_bottom().into()
        };

        // Sidebar, not an inline expansion: a log body is taller than the row
        // list can absorb without pushing every other event off screen.
        match state
            .event_md
            .as_ref()
            .and_then(|(id, items)| matched.iter().find(|e| e.id == *id).map(|e| (*e, items)))
        {
            Some((e, items)) => row![
                container(list).width(Length::FillPortion(1)),
                container(event_detail(e, items, iced_theme)).width(Length::FillPortion(1)),
            ]
            .spacing(space::MD)
            .into(),
            None => list,
        }
    };

    let count = if filter.is_empty() {
        ui::count(state.events.len(), "event", "events")
    } else {
        format!("{} of {}", matched.len(), ui::count(state.events.len(), "event", "events"))
    };

    let toolbar: Element<'a, Message> = ui::cluster(vec![
        container(ui::input_icon(Icon::Search, "Filter events…", &state.event_filter, Message::EventFilterChanged))
            .width(280)
            .into(),
        ui::spacer(),
        ui::caption(count),
    ])
    .into();

    ui::card(column![toolbar, event_type_chips(state), ui::separator(), body].spacing(space::MD))
}

/// One chip per event type present in this run — typing "task_completed" into
/// the box to see what a run did was the only way to narrow the log before.
fn event_type_chips(state: &State) -> Element<'_, Message> {
    let mut types: Vec<&str> = state.events.iter().map(|e| e.event_type.as_str()).collect();
    types.sort_unstable();
    types.dedup();
    if types.len() < 2 {
        return ui::spacer();
    }
    let mut options: Vec<(&str, bool, Message)> =
        vec![("All", state.event_filter.is_empty(), Message::EventFilterChanged(String::new()))];
    options.extend(types.into_iter().map(|t| {
        (t, state.event_filter == t, Message::EventFilterChanged(t.to_string()))
    }));
    ui::chips(options)
}

/// The pills every event row and its detail header share.
fn event_meta<'a>(e: &'a agent_platform_client::types::EventLogRecord) -> Row<'a, Message> {
    let mut meta: Vec<Element<'a, Message>> =
        vec![ui::badge(e.event_type.clone(), event_tone(&e.event_type))];
    if let Some(task_id) = e.task_id {
        meta.push(ui::badge_icon(Icon::Users, format!("task {task_id}"), Tone::Neutral));
    }
    let lines = e.content.lines().count();
    if lines > 1 {
        meta.push(ui::badge_icon(Icon::Scroll, ui::count(lines, "line", "lines"), Tone::Neutral));
    }
    meta.push(ui::spacer());
    meta.push(ui::caption(domain::relative_time(&e.created_at).unwrap_or_default()));
    ui::cluster(meta)
}

/// One line of the body for the collapsed row — markdown headings and bullets
/// read as noise at this size, so the first line with words in it wins.
fn event_summary(content: &str) -> String {
    let line = content
        .lines()
        .map(|l| l.trim_start_matches(['#', '*', '-', '>', ' ']).trim())
        .find(|l| !l.is_empty())
        .unwrap_or("");
    truncate(line, 140)
}

fn event_detail<'a>(
    e: &'a agent_platform_client::types::EventLogRecord,
    items: &'a [markdown::Item],
    iced_theme: &Theme,
) -> Element<'a, Message> {
    let mut header = vec![ui::mono(format!("#{}", e.id)), ui::spacer()];
    if let Some(task_id) = e.task_id {
        header.push(ui::button_ghost(Icon::Users, "Open subagent", Message::InspectTask(task_id)));
    }
    header.push(ui::button_ghost(Icon::Copy, "Copy", Message::CopyEvent(e.id)));
    header.push(ui::icon_tip(Icon::X, "Close", Message::OpenEvent(None)));
    let header = ui::cluster(header);
    ui::card(ui::stack(vec![
        header.into(),
        event_meta(e).into(),
        ui::caption(e.created_at.clone()),
        ui::separator(),
        scrollable(markdown::view(items, markdown::Settings::from(iced_theme)).map(Message::LinkClicked))
            .height(380)
            .into(),
    ]))
}

fn event_tone(event_type: &str) -> Tone {
    match event_type {
        "error" => Tone::Danger,
        "status_change" => Tone::Info,
        "task_completed" => Tone::Success,
        _ => Tone::Neutral,
    }
}

// ---------------------------------------------------------------------------
// Subagent inspector
// ---------------------------------------------------------------------------

fn inspector<'a>(state: &'a State, uuid: &'a str, iced_theme: &Theme) -> Element<'a, Message> {
    let rows = state.board_rows();
    let Some(row) = rows.iter().find(|r| r.subagent.client_uuid == uuid) else {
        return container(ui::empty_state_icon(Icon::Users, "Subagent not found.")).into();
    };
    let task = state.task_by_uuid(uuid);

    let mut body = vec![
        ui::field("Status", ui::badge(row.column.label(), row.column.tone())),
        ui::field("UUID", ui::mono(uuid.to_string())),
    ];
    if let Some(model) = &row.subagent.model {
        body.push(ui::field("Model", ui::mono(model.clone())));
    }
    // A node that renders instead of writing (ADR 0018). Its output line names
    // the file route, so this says what it is rather than repeating that.
    if let Some(task) = task.filter(|t| t.modality != "text") {
        body.push(ui::field(
            "Produces",
            ui::cluster(vec![
                ui::badge_icon(Icon::Image, task.modality.clone(), Tone::Info),
                match task.media_job_id {
                    Some(id) => ui::mono(format!("media job {id}")),
                    None => ui::caption("not started yet"),
                },
            ]),
        ));
    }
    // Sub-DAG tasks have no planner entry, so fall back to the task's own
    // dependencies_json rather than showing nothing.
    let deps = row
        .subagent
        .dependencies
        .clone()
        .filter(|d| !d.is_empty())
        .or_else(|| task.map(domain::parse_task_dependencies).filter(|d| !d.is_empty()));
    if let Some(deps) = deps {
        body.push(ui::field(
            "Depends on",
            ui::mono(deps.iter().map(|d| domain::short_uuid(d)).collect::<Vec<_>>().join(", ")),
        ));
    }
    body.push(ui::field("Instructions", ui::body(row.subagent.instructions.clone())));
    body.push(ui::field("System prompt", ui::muted(row.subagent.system_prompt.clone())));

    if let Some(task) = task {
        body.push(ui::separator());
        body.push(ui::field("Tokens", ui::body(task.tokens_used.to_string())));
        if let Some(count) = task.revision_count {
            body.push(ui::field("Revisions", ui::body(count.to_string())));
        }
        if let Some(feedback) = &task.review_feedback {
            body.push(ui::field("Review feedback", ui::body(feedback.clone())));
        }
        if let Some(output) = task.output.as_ref().or(task.draft_output.as_ref()) {
            // Agents write markdown; mono made every heading and bullet noise.
            // Parsed in `refresh_output_md`, not here — this runs every frame.
            let rendered: Element<'a, Message> = match &state.output_md {
                Some((id, _, items)) if *id == task.id => {
                    markdown::view(items, markdown::Settings::from(iced_theme))
                        .map(Message::LinkClicked)
                }
                _ => ui::mono(output.clone()),
            };
            // Capped: a long output used to push everything under it — including
            // the failure detail — off the bottom of the pane.
            let long = output.lines().count() > 14;
            let block = scrollable(rendered).width(Length::Fill);
            body.push(ui::code(if long { block.height(260) } else { block }));
            body.push(ui::button_ghost(Icon::Copy, "Copy output", Message::CopyOutput(task.id)));
        }
        if let Some(debug) = &task.failure_debug_json {
            body.push(ui::alert(Tone::Danger, "Failure detail", Some(ui::code(ui::mono(debug.clone())))));
        }
    }

    let actions: Option<Element<'a, Message>> = task.map(|t| {
        let mut buttons = vec![ui::button_ghost(Icon::X, "Close", Message::Inspect(None))];
        if row.column == BoardColumn::AwaitingReview {
            buttons.insert(0, ui::button_secondary(Icon::Eye, "Review", Message::OpenReview(t.id)));
            buttons.insert(0, ui::button_default(Icon::Check, "Approve", Message::ApproveTask(t.id)));
            buttons.push(ui::button_destructive(Icon::X, "Reject", Message::RejectTask(t.id)));
        }
        if row.column == BoardColumn::Failed {
            buttons.insert(0, ui::button_secondary(Icon::RotateCcw, "Retry task", Message::RetryTask(t.id)));
        }
        ui::cluster(buttons).into()
    });

    ui::card_with_header(
        row.subagent.role.clone(),
        Some(ui::muted("Subagent detail")),
        actions,
        ui::stack(body),
    )
}

/// Reject is not a per-task undo: the server fails the whole run on it, so the
/// dialog says that rather than letting a stray click end the work.
fn reject_confirm<'a>(task_id: i64) -> Element<'a, Message> {
    ui::confirm_dialog(
        "Reject this task?",
        "Rejecting fails the task and ends the whole run. Request changes instead if the work can be redone.",
        vec![
            ui::button_ghost(Icon::X, "Cancel", Message::CancelReject),
            ui::button_destructive(Icon::X, "Reject and end run", Message::RejectTaskConfirmed(task_id)),
        ],
    )
}

// ---------------------------------------------------------------------------
// Review modal
// ---------------------------------------------------------------------------

fn review_modal<'a>(state: &'a State, draft: &'a crate::processes::ReviewDraft) -> Element<'a, Message> {
    ui::card_with_header(
        format!("Review: {}", draft.role),
        Some(ui::muted("Approve the output, request changes, or reject the task. Esc closes.")),
        None,
        ui::stack(vec![
            ui::caption("Output — edited on approve"),
            ui::code(
                text_editor(&state.review_output)
                    .on_action(Message::ReviewOutputEdited)
                    .height(220),
            ),
            ui::caption("Feedback"),
            ui::input("Why?", &draft.feedback, Message::ReviewFeedbackChanged),
            ui::caption("Revised instructions (request changes)"),
            ui::input("New instructions", &draft.instructions, Message::ReviewInstructionsChanged),
            ui::cluster(vec![
                ui::button_default(Icon::Check, "Approve", Message::SubmitReview(ReviewDecision::Approve)),
                ui::button_secondary(
                    Icon::Pencil,
                    "Request changes",
                    Message::SubmitReview(ReviewDecision::RequestChanges),
                ),
                ui::button_destructive(Icon::X, "Reject", Message::RejectTask(draft.task_id)),
                ui::spacer(),
                ui::button_ghost(Icon::X, "Cancel", Message::CloseReview),
            ])
            .into(),
        ]),
    )
}
