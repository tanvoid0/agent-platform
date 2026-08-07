//! Processes screen rendering — composed from the shadcn-style `ui` kit.

use crate::domain::{self, BoardColumn, BoardRow};
use crate::processes::{Message, State, ViewMode};
use crate::ui::{self, space, Icon, Tone};
use agent_platform_client::types::{ProcessRecord, ReviewDecision, TaskNodeRecord};
use iced::widget::{checkbox, column, container, row, scrollable};
use iced::{Element, Length, Theme};

pub fn view<'a>(state: &'a State, iced_theme: &Theme) -> Element<'a, Message> {
    let main = row![
        run_list(state),
        ui::separator_vertical(),
        container(detail_pane(state, iced_theme)).width(Length::Fill).height(Length::Fill),
    ];

    // The review modal is an overlay, shadcn `Dialog`-style.
    match &state.review {
        None => main.into(),
        Some(draft) => ui::modal(main, review_modal(draft), 680.0),
    }
}

// ---------------------------------------------------------------------------
// Left: composer + run list
// ---------------------------------------------------------------------------

fn run_list(state: &State) -> Element<'_, Message> {
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

    let composer = ui::card(
        ui::stack(vec![
            ui::heading("New run"),
            ui::input_icon(Icon::Sparkles, "What should the team accomplish?", &state.composer.goal, Message::GoalChanged),
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
            checkbox(state.composer.auto_approve)
                .label("Auto-approve the plan")
                .on_toggle(Message::ToggleAutoApprove)
                .size(16)
                .text_size(ui::font::SM)
                .into(),
            if state.composer.submitting {
                ui::button_sized(Some(Icon::Clock), "Starting…", ui::ButtonVariant::Default, ui::Size::Sm, None)
            } else {
                ui::button_default(Icon::Play, "Start run", Message::Submit)
            },
        ]),
    );

    let items: Vec<Element<'_, Message>> = state
        .processes
        .iter()
        .map(|p| run_list_item(p, state.selected == Some(p.id)))
        .collect();

    let list: Element<'_, Message> = if items.is_empty() {
        ui::empty_state_icon(Icon::Activity, "No runs in this scope yet.")
    } else {
        scrollable(ui::stack(items)).height(Length::Fill).into()
    };

    container(
        column![composer, ui::caption("RECENT RUNS"), list]
            .spacing(space::MD)
            .padding(space::MD),
    )
    .width(340)
    .height(Length::Fill)
    .into()
}

const UNASSIGNED: &str = "Unassigned";

fn run_list_item(p: &ProcessRecord, selected: bool) -> Element<'_, Message> {
    let when = domain::relative_time(&p.created_at).unwrap_or_default();
    ui::list_item(
        ui::stack(vec![
            ui::cluster(vec![
                ui::badge(p.status.as_str(), domain::process_status_tone(p.status.as_str())),
                ui::caption(format!("#{}", p.id)),
                ui::spacer(),
                ui::caption(when),
            ])
            .into(),
            ui::body(truncate(&p.goal, 90)),
        ]),
        selected,
        Message::Select(p.id),
    )
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

// ---------------------------------------------------------------------------
// Right: detail pane
// ---------------------------------------------------------------------------

fn detail_pane<'a>(state: &'a State, iced_theme: &Theme) -> Element<'a, Message> {
    let mut blocks: Vec<Element<'_, Message>> = Vec::new();

    if let Some(err) = &state.error {
        blocks.push(dismissible(ui::alert_error_traced(err, Message::TraceLogs)));
    }

    let Some(process) = state.selected_process() else {
        blocks.push(if state.selected.is_some() {
            ui::empty_state_icon(Icon::Clock, "Loading run…")
        } else {
            ui::empty_state_icon(Icon::Activity, "Nothing selected.")
        });
        return ui::page(
            "Processes",
            Some(ui::muted("Pick a run, or start a new one.")),
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
        ViewMode::Events => events_view(state),
    });
    if let Some(uuid) = &state.inspecting {
        blocks.push(inspector(state, uuid));
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

fn dismissible(inner: Element<'_, Message>) -> Element<'_, Message> {
    ui::cluster(vec![
        container(inner).width(Length::Fill).into(),
        ui::button_ghost(Icon::X, "Dismiss", Message::DismissNotice),
    ])
    .into()
}

fn actions_row<'a>(state: &'a State, process: &'a ProcessRecord) -> Element<'a, Message> {
    let status = process.status.as_str();
    let mut buttons: Vec<Element<'a, Message>> = Vec::new();

    if state.busy {
        buttons.push(ui::badge("working…", Tone::Info));
    }
    if status == "approval_required" {
        buttons.push(ui::button_default(Icon::Check, "Approve plan", Message::Approve));
    }
    if matches!(status, "pending" | "planning" | "approved" | "running" | "approval_required") {
        buttons.push(ui::button_destructive(Icon::X, "Cancel", Message::Cancel));
    }
    if matches!(status, "failed" | "cancelled" | "completed") {
        buttons.push(ui::button_secondary(Icon::RotateCcw, "Retry", Message::Retry));
    }
    buttons.push(ui::button_outline(Icon::Refresh, "Sync", Message::Sync));
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
        ui::stat(Icon::Activity, "Status", process.status.as_str().to_string()),
        ui::stat(Icon::Cpu, "Tokens", process.total_tokens.to_string()),
        ui::stat(Icon::Gauge, "Cost", format!("${:.4}", process.total_cost)),
    ];
    if let Some(tools) = process.tool_invocations_used {
        stats.push(ui::stat(Icon::Settings, "Tool calls", tools.to_string()));
    }
    if let Some(detail) = &state.detail {
        stats.push(ui::stat(Icon::Scroll, "Tasks", detail.tasks.len().to_string()));
    }

    let mut rows = vec![ui::cluster(stats).into()];
    if let Some(reason) = &process.failure_reason {
        rows.push(ui::alert(Tone::Danger, "Failure", Some(ui::mono(reason.clone()))));
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
        controls.push(ui::caption("LINEAGE"));
        controls.push(ui::segmented(
            crate::graph::Lineage::ALL.map(|l| (l.label(), state.lineage == l, Message::SetLineage(l))),
        ));
    }
    controls.push(ui::spacer());
    controls.push(ui::caption(format!(
        "{} nodes · drag to pan, scroll to zoom",
        layout.nodes.len()
    )));
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
        checkbox(state.needs_attention_only)
            .label("Needs attention")
            .on_toggle(Message::ToggleNeedsAttention)
            .size(16)
            .text_size(ui::font::SM)
            .into(),
    ])
    .into()
}

fn board_view(state: &State) -> Element<'_, Message> {
    let columns: Vec<Element<'_, Message>> = BoardColumn::ALL
        .iter()
        .map(|col| {
            let rows = state.rows_in_column(*col);
            let count = rows.len();
            let cards: Vec<Element<'_, Message>> = if rows.is_empty() {
                vec![ui::caption("—")]
            } else {
                let inspecting = state.inspecting.clone();
                rows.into_iter().map(|r| board_card(inspecting.as_deref(), r)).collect()
            };
            container(
                ui::stack(vec![
                    ui::cluster(vec![
                        ui::badge(col.label(), col.tone()),
                        ui::caption(count.to_string()),
                    ])
                    .into(),
                    ui::stack(cards).into(),
                ])
                .width(Length::Fill),
            )
            .width(Length::FillPortion(1))
            .into()
        })
        .collect();

    ui::card(
        column![board_toolbar(state), ui::separator(), row(columns).spacing(space::SM)]
            .spacing(space::MD),
    )
}

fn board_card<'a>(inspecting: Option<&str>, row: BoardRow) -> Element<'a, Message> {
    let uuid = row.subagent.client_uuid.clone();
    let selected = inspecting == Some(uuid.as_str());
    let mut lines = vec![ui::body(row.subagent.role.clone())];
    if let Some(activity) = domain::relative_task_activity(&row) {
        lines.push(ui::caption(activity));
    } else {
        lines.push(ui::caption(domain::short_uuid(&uuid)));
    }
    if row.column == BoardColumn::AwaitingReview {
        if let Some(task) = &row.task {
            lines.push(ui::button_secondary(Icon::Eye, "Review", Message::OpenReview(task.id)));
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
    for tr in &rows {
        if tr.wave_index != current {
            if !bucket.is_empty() {
                waves.push(wave_block(current, std::mem::take(&mut bucket)));
            }
            current = tr.wave_index;
        }
        bucket.push(
            ui::list_item(
                ui::cluster(vec![
                    ui::badge(tr.column.label(), tr.column.tone()),
                    ui::body(tr.role.clone()),
                    ui::spacer(),
                    ui::caption(domain::short_uuid(&tr.client_uuid)),
                ]),
                state.inspecting.as_deref() == Some(tr.client_uuid.as_str()),
                Message::Inspect(Some(tr.client_uuid.clone())),
            ),
        );
    }
    if !bucket.is_empty() {
        waves.push(wave_block(current, bucket));
    }

    ui::card(ui::stack_lg(waves))
}

fn wave_block<'a>(index: usize, rows: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    ui::stack(vec![
        ui::caption(format!("WAVE {}", index + 1)),
        ui::stack(rows).into(),
    ])
    .into()
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

fn events_view(state: &State) -> Element<'_, Message> {
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

    let body: Element<'_, Message> = if matched.is_empty() {
        if state.events.is_empty() {
            ui::empty_state_icon(Icon::Scroll, "No events yet.")
        } else {
            ui::empty_state_icon(Icon::Search, "No events match this filter.")
        }
    } else {
        // Newest last, tail-rendered: iced lays out every child.
        let tail = &matched[matched.len().saturating_sub(400)..];
        let rows: Vec<Element<'_, Message>> = tail
            .iter()
            .map(|e| {
                ui::cluster(vec![
                    ui::badge(e.event_type.clone(), event_tone(&e.event_type)),
                    container(ui::mono(e.content.clone())).width(Length::Fill).into(),
                    ui::caption(domain::relative_time(&e.created_at).unwrap_or_default()),
                ])
                .into()
            })
            .collect();
        scrollable(ui::stack(rows)).spacing(space::SM).height(400).anchor_bottom().into()
    };

    let count = if filter.is_empty() {
        ui::count(state.events.len(), "event", "events")
    } else {
        format!("{} of {}", matched.len(), ui::count(state.events.len(), "event", "events"))
    };

    let toolbar: Element<'_, Message> = ui::cluster(vec![
        container(ui::input_icon(Icon::Search, "Filter events…", &state.event_filter, Message::EventFilterChanged))
            .width(280)
            .into(),
        ui::spacer(),
        ui::caption(count),
    ])
    .into();

    ui::card(column![toolbar, ui::separator(), body].spacing(space::MD))
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

fn inspector<'a>(state: &'a State, uuid: &'a str) -> Element<'a, Message> {
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
            body.push(ui::code(ui::mono(output.clone())));
        }
        if let Some(debug) = &task.failure_debug_json {
            body.push(ui::alert(Tone::Danger, "Failure detail", Some(ui::code(ui::mono(debug.clone())))));
        }
    }

    let actions: Option<Element<'a, Message>> = task.map(|t| {
        let mut buttons = vec![ui::button_ghost(Icon::X, "Close", Message::Inspect(None))];
        if row.column == BoardColumn::AwaitingReview {
            buttons.insert(0, ui::button_default(Icon::Eye, "Review", Message::OpenReview(t.id)));
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

// ---------------------------------------------------------------------------
// Review modal
// ---------------------------------------------------------------------------

fn review_modal(draft: &crate::processes::ReviewDraft) -> Element<'_, Message> {
    let dialog = ui::card_with_header(
        format!("Review: {}", draft.role),
        Some(ui::muted("Approve the output, request changes, or reject the task.")),
        None,
        ui::stack(vec![
            ui::caption("OUTPUT (edited on approve)"),
            ui::input("Output", &draft.output, Message::ReviewOutputChanged),
            ui::caption("FEEDBACK"),
            ui::input("Why?", &draft.feedback, Message::ReviewFeedbackChanged),
            ui::caption("REVISED INSTRUCTIONS (request changes)"),
            ui::input("New instructions", &draft.instructions, Message::ReviewInstructionsChanged),
            ui::cluster(vec![
                ui::button_default(Icon::Check, "Approve", Message::SubmitReview(ReviewDecision::Approve)),
                ui::button_secondary(
                    Icon::Pencil,
                    "Request changes",
                    Message::SubmitReview(ReviewDecision::RequestChanges),
                ),
                ui::button_destructive(Icon::X, "Reject", Message::SubmitReview(ReviewDecision::Reject)),
                ui::spacer(),
                ui::button_ghost(Icon::X, "Cancel", Message::CloseReview),
            ])
            .into(),
        ]),
    );

    dialog
}
