//! Workflows screen: the list, a JSON step editor, and per-run results.
//!
//! Reads top to bottom: editor (when open), then the workflows, then the runs
//! of whichever workflow is selected. No canvas, no node graph — a workflow is
//! a short list of steps, and a list renders a list best.

use crate::domain::truncate;
use crate::ui::{self, space, Icon, Tone};
use crate::workflows::{Message, State};
use agent_platform_client::types::{WorkflowInfo, WorkflowRunInfo};
use iced::widget::{column, container, row, text_editor};
use iced::{Element, Length};

pub fn view(state: &State) -> Element<'_, Message> {
    let mut blocks: Vec<Element<'_, Message>> = Vec::new();

    if let Some(err) = &state.error {
        blocks.push(
            ui::cluster(vec![
                container(ui::alert_error_traced(err, Message::TraceLogs))
                    .width(Length::Fill)
                    .into(),
                ui::button_ghost(Icon::X, "Dismiss", Message::Dismiss),
            ])
            .into(),
        );
    }

    if let Some(editor) = &state.editor {
        blocks.push(editor_card(state, editor));
    }

    let list: Element<'_, Message> = if !state.loaded {
        ui::empty_state_icon(Icon::Clock, "Loading workflows…")
    } else if state.items.is_empty() {
        ui::empty_state_action(
            Icon::Zap,
            "No workflows yet. A workflow is a fixed list of HTTP or action steps \
             the server runs for you — on demand, on a timer, or when another app \
             calls its run endpoint.",
            ui::button_default(Icon::Plus, "New workflow", Message::New),
        )
    } else {
        ui::stack(state.items.iter().map(|wf| workflow_card(state, wf)).collect()).into()
    };
    // No wrapping section: every workflow is already a card, and a "Workflows"
    // card under the "Workflows" page title was a box inside a box.
    blocks.push(list);

    if let Some(selected) = state.selected {
        if let Some(wf) = state.items.iter().find(|w| w.id == selected) {
            blocks.push(ui::section(
                "Recent runs",
                Some(ui::muted(format!("\"{}\", newest first.", wf.name))),
                runs_view(state),
            ));
        }
    }

    let page = ui::page(
        "Workflows",
        Some(ui::muted(
            "Fixed automations on a timer or the API. A team run belongs on Processes.",
        )),
        Some(
            ui::cluster(vec![
                ui::button_secondary(Icon::Refresh, "Refresh", Message::Refresh),
                ui::button_default(Icon::Plus, "New workflow", Message::New),
            ])
            .into(),
        ),
        ui::stack_lg(blocks),
    );
    match &state.confirm {
        None => page,
        Some(confirm) => ui::modal(
            page,
            ui::confirm_dialog(
                "Delete this workflow?",
                "This cannot be undone.",
                vec![
                    ui::button_ghost(Icon::X, "Cancel", Message::CancelConfirm),
                    ui::button_destructive(Icon::Trash, "Delete", confirm.then.clone()),
                ],
            ),
            420.0,
        ),
    }
}

fn editor_card<'a>(state: &'a State, editor: &'a crate::workflows::Editor) -> Element<'a, Message> {
    let title = if editor.id.is_some() { "Edit workflow" } else { "New workflow" };

    // Notion-style helper: describe the change in prose; the reply lands here
    // and any proposed steps land straight in the editor above.
    let mut assist: Vec<Element<'a, Message>> = vec![ui::cluster(vec![
        container(ui::input_submit(
            "Ask AI to draft, review or change these steps…",
            &editor.assist_prompt,
            Message::AssistPromptChanged,
            Message::AskAssist,
        ))
        .width(Length::Fill)
        .into(),
        ui::button_secondary(
            Icon::Sparkles,
            if editor.assist_busy { "Thinking…" } else { "Ask AI" },
            Message::AskAssist,
        ),
    ])
    .into()];
    if let Some(reply) = &editor.assist_reply {
        assist.push(ui::muted(reply.clone()));
    }

    ui::card_with_header(
        title,
        Some(ui::muted(
            "Steps run top to bottom. Reference earlier data with \
             {{trigger.body.…}} and {{steps.<id>.output.…}}.",
        )),
        None,
        ui::stack(vec![
            ui::field("Name", ui::input("What this automation does", &editor.name, Message::NameChanged)),
            ui::field(
                "Description",
                ui::input("Optional", &editor.description, Message::DescriptionChanged),
            ),
            ui::field(
                "Run every (seconds)",
                ui::input("Leave empty for manual/API only", &editor.interval, Message::IntervalChanged),
            ),
            ui::field(
                "Steps (JSON)",
                ui::code(
                    text_editor(&editor.steps)
                        .on_action(Message::StepsEdited)
                        .font(iced::Font::MONOSPACE)
                        .height(240),
                ),
            ),
            ui::field("Assistant", ui::stack(assist)),
            ui::cluster(vec![
                if state.busy {
                    ui::button_secondary(Icon::Clock, "Saving…", Message::Save)
                } else {
                    ui::button_default(Icon::Save, "Save", Message::Save)
                },
                ui::button_ghost(Icon::X, "Cancel", Message::CancelEditor),
            ])
            .into(),
        ]),
    )
}

fn workflow_card<'a>(state: &'a State, wf: &'a WorkflowInfo) -> Element<'a, Message> {
    let mut badges = vec![if wf.enabled {
        ui::badge("Enabled", Tone::Success)
    } else {
        ui::badge("Disabled", Tone::Neutral)
    }];
    badges.push(ui::badge(ui::count(wf.steps.len(), "step", "steps"), Tone::Neutral));
    if let Some(seconds) = wf.interval_seconds {
        badges.push(ui::badge(format!("every {}", human_secs(seconds)), Tone::Info));
    }

    let selected = state.selected == Some(wf.id);
    let mut lines = vec![
        ui::cluster(vec![ui::body(wf.name.clone()), ui::spacer()].into_iter().chain(badges).collect())
            .into(),
    ];
    if let Some(desc) = &wf.description {
        lines.push(ui::muted(desc.clone()));
    }
    lines.push(
        ui::cluster(vec![
            // One run at a time server-side, so every card's button is dead
            // while any workflow runs — look it, don't just act it.
            ui::button_sized(
                Some(Icon::Play),
                if state.running == Some(wf.id) { "Running…" } else { "Run now" },
                ui::ButtonVariant::Secondary,
                ui::Size::Sm,
                state.running.is_none().then_some(Message::RunNow(wf.id)),
            ),
            ui::button_ghost(
                Icon::Scroll,
                if selected { "Hide runs" } else { "Runs" },
                Message::Select(wf.id),
            ),
            ui::button_ghost(Icon::Pencil, "Edit", Message::Edit(wf.id)),
            ui::button_ghost(
                if wf.enabled { Icon::Pause } else { Icon::Play },
                if wf.enabled { "Disable" } else { "Enable" },
                Message::SetEnabled(wf.id, !wf.enabled),
            ),
            ui::spacer(),
            ui::button_ghost(Icon::Trash, "Delete", Message::Delete(wf.id)),
        ])
        .into(),
    );

    ui::tile(ui::stack(lines))
}

fn runs_view(state: &State) -> Element<'_, Message> {
    if state.runs.is_empty() {
        return if state.runs_loading {
            ui::empty_state_icon(Icon::Clock, "Loading runs…")
        } else {
            ui::empty_state_icon(Icon::Inbox, "No runs yet. Press \"Run now\" to try it.")
        };
    }
    ui::stack(state.runs.iter().map(|run| run_row(state, run)).collect()).into()
}

fn run_row<'a>(state: &'a State, run: &'a WorkflowRunInfo) -> Element<'a, Message> {
    let tone = match run.status.as_str() {
        "succeeded" => Tone::Success,
        "failed" => Tone::Danger,
        _ => Tone::Warning,
    };
    let expanded = state.expanded_run == Some(run.id);

    let mut lines: Vec<Element<'a, Message>> = vec![row![
        ui::badge(status_label(&run.status), tone),
        ui::muted(format!("#{} · {} · {}", run.id, run.trigger, human_time(&run.started_at))),
        ui::spacer(),
        ui::button_ghost(
            Icon::Eye,
            if expanded { "Hide steps" } else { "Steps" },
            Message::ToggleRun(run.id),
        ),
    ]
    .spacing(space::SM)
    .align_y(iced::Alignment::Center)
    .into()];

    if let Some(err) = &run.error {
        lines.push(ui::caption(err.clone()));
    }
    if expanded {
        lines.push(ui::code(ui::stack(run.steps.iter().map(step_row).collect())));
    }

    ui::tile(ui::stack(lines))
}

fn step_row<'a>(
    step: &'a agent_platform_client::types::WorkflowStepResult,
) -> Element<'a, Message> {
    let (icon, tone) = match step.status.as_str() {
        "succeeded" => (Icon::CheckCircle, Tone::Success),
        "failed" => (Icon::XCircle, Tone::Danger),
        _ => (Icon::Clock, Tone::Neutral),
    };
    let mut cells: Vec<Element<'a, Message>> = vec![
        ui::badge_icon(icon, step.status.clone(), tone),
        ui::mono(step.id.clone()),
    ];
    if let Some(ms) = step.duration_ms {
        cells.push(ui::muted(format!("{ms} ms")));
    }
    if let Some(err) = &step.error {
        cells.push(ui::muted(err.clone()));
    } else if let Some(output) = &step.output {
        let text = serde_json::to_string(output).unwrap_or_default();
        cells.push(container(ui::muted(truncate(&text, 160))).width(Length::Fill).into());
    }
    column![ui::cluster(cells)].padding(iced::Padding::from([2.0, 0.0])).into()
}

fn status_label(status: &str) -> &str {
    match status {
        "succeeded" => "Succeeded",
        "failed" => "Failed",
        "running" => "Running",
        "cancelled" => "Cancelled",
        other => other,
    }
}

fn human_secs(seconds: i64) -> String {
    match seconds {
        s if s < 120 => format!("{s}s"),
        s if s < 7200 => format!("{}m", s / 60),
        s => format!("{}h", s / 3600),
    }
}

/// `2026-08-04T09:30:12.345` → `2026-08-04 09:30:12`; anything shorter passes through.
fn human_time(iso: &str) -> String {
    iso.chars().take(19).collect::<String>().replace('T', " ")
}
