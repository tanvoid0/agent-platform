//! The planning chat's pane: thread picker on top, transcript in the middle,
//! composer pinned at the bottom.
//!
//! Whatever the assistant is *asking for* — a proposal to approve, an intake
//! form to fill — is the last thing in the scrolling region, not a card wedged
//! between it and the composer. A fitness intake form is taller than this pane,
//! and a fixed card that size pushes the composer and its own submit button off
//! the bottom of the window. Scrolling to it costs a snap, which arriving turns
//! already do.

use crate::agenda_chat::{Message, State};
use crate::ui::{self, space, Icon, Tone};
use agent_platform_client::types::{PlannedAction, PlanningForm, PlanningFormField};
use iced::widget::{column, container, markdown, scrollable};
use iced::{Element, Length, Theme};

/// Wide enough for a two-column form row and a readable line of prose; the board
/// beside it keeps the rest of the window.
pub const WIDTH: f32 = 460.0;

pub fn pane<'a>(state: &'a State, theme: &Theme) -> Element<'a, Message> {
    let mut blocks: Vec<Element<'a, Message>> = vec![header(state)];

    if let Some(usage) = &state.usage {
        if usage.context_window > 0 {
            blocks.push(ui::caption(format!(
                "Context {:.0}% of {}",
                usage.percent_used, usage.context_window
            )));
        }
    }
    if let Some(err) = &state.error {
        blocks.push(ui::error_bar(err, Message::TraceLogs, Message::DismissError, Vec::new()));
    }
    if let Some(notice) = &state.notice {
        blocks.push(ui::dismissible(
            ui::alert(Tone::Success, notice.clone(), None),
            Message::DismissError,
            Vec::new(),
        ));
    }

    blocks.push(container(transcript(state, theme)).height(Length::Fill).into());
    blocks.push(composer(state));

    container(column(blocks).spacing(space::MD).height(Length::Fill))
        .width(WIDTH)
        .height(Length::Fill)
        .into()
}

/// Which thread, and the two ways out of it: a new one, or closing the pane.
fn header(state: &State) -> Element<'_, Message> {
    let options = state.options();
    let picker: Element<'_, Message> = if options.is_empty() {
        ui::muted("No conversations yet")
    } else {
        ui::select("Conversation", options, state.selected(), |o| Message::SelectThread(o.id))
    };

    ui::cluster(vec![
        container(picker).width(Length::Fill).into(),
        ui::icon_button(Icon::Plus, Message::NewThread),
        ui::icon_button(Icon::X, Message::Close),
    ])
    .into()
}

fn transcript<'a>(state: &'a State, theme: &Theme) -> Element<'a, Message> {
    if state.messages.is_empty() && state.form.is_none() {
        return if state.loading {
            ui::empty_state("Loading…")
        } else {
            ui::empty_state_icon(
                Icon::Sparkles,
                "Tell the assistant what you are trying to get done. \
                 It plans; the board on the left is where the plan lands.",
            )
        };
    }

    let mut turns: Vec<Element<'a, Message>> = state
        .messages
        .iter()
        .zip(&state.md)
        .map(|(m, items)| {
            let is_user = m.role == "user";
            let (label, tone) =
                if is_user { ("You", Tone::Neutral) } else { ("Assistant", Tone::Info) };

            let mut parts: Vec<Element<'a, Message>> = vec![if is_user {
                ui::body(m.content.clone())
            } else {
                markdown::view(items, markdown::Settings::from(theme)).map(Message::LinkClicked)
            }];
            // What became of a proposal this turn carried. A reopened thread has
            // to show a decision that was already taken as taken, or the same
            // actions read as still on offer.
            if let Some(status) = m.proposal_status.as_deref().filter(|s| *s != "pending") {
                parts.push(ui::badge(
                    ui::count(m.proposed_actions.len(), "action", "actions") + " " + status,
                    if status == "approved" { Tone::Success } else { Tone::Neutral },
                ));
            }
            ui::turn(label, tone, is_user, column(parts).spacing(space::XS).into())
        })
        .collect();

    // Regenerating is offered on the finished thread only: mid-turn it would
    // race the reply that is already coming.
    if !state.sending && state.thread_id.is_some() && state.last_user_index().is_some() {
        turns.push(ui::button_ghost(Icon::RotateCcw, "Retry that turn", Message::Retry));
    }

    if let Some(form) = &state.form {
        turns.push(form_card(state, form));
    } else if !state.pending.is_empty() {
        turns.push(proposal_card(state));
    }

    scrollable(ui::stack_lg(turns).padding(iced::Padding { right: 12.0, ..Default::default() }))
        .id(State::scroll_id())
        .height(Length::Fill)
        .into()
}

/// What the assistant wants to put on the board, and the two answers to it.
/// Every action goes or none does — picking them apart would need the parameter
/// editor this pane deliberately does not have.
fn proposal_card(state: &State) -> Element<'_, Message> {
    let mut lines: Vec<Element<'_, Message>> =
        state.pending.iter().map(action_row).collect();

    lines.push(
        ui::cluster(if state.sending {
            vec![ui::badge("working…", Tone::Info)]
        } else {
            vec![
                ui::button_default(Icon::Check, "Add to board", Message::ApplyActions),
                ui::button_ghost(Icon::X, "Not now", Message::DismissActions),
            ]
        })
        .into(),
    );

    ui::section(
        ui::count(state.pending.len(), "suggestion", "suggestions"),
        None,
        ui::stack(lines),
    )
}

fn action_row(action: &PlannedAction) -> Element<'_, Message> {
    let mut lines: Vec<Element<'_, Message>> = vec![ui::body(action.name.clone())];
    if let Some(why) = action.reasoning.as_ref().filter(|w| !w.trim().is_empty()) {
        lines.push(ui::caption(why.clone()));
    }
    ui::stack(lines).into()
}

/// The intake or clarifying form. Submit stays disabled until every required
/// field is answered — the alternative is spending a slow LLM turn on a form the
/// assistant then has to ask for again.
fn form_card<'a>(state: &'a State, form: &'a PlanningForm) -> Element<'a, Message> {
    let mut lines: Vec<Element<'a, Message>> = Vec::new();
    if let Some(description) = form.description.as_ref().filter(|d| !d.is_empty()) {
        lines.push(ui::muted(description.clone()));
    }
    lines.extend(form.fields.iter().map(|f| field_row(state, f)));

    let submit = if state.sending {
        ui::badge("working…", Tone::Info)
    } else if state.form_ready() {
        ui::button_default(Icon::Send, "Save and continue", Message::SubmitForm)
    } else {
        ui::button_sized(
            Some(Icon::Send),
            "Save and continue",
            ui::ButtonVariant::Default,
            ui::Size::Sm,
            None,
        )
    };
    lines.push(ui::cluster(vec![submit]).into());

    ui::section(form.title.clone().unwrap_or_else(|| "Details needed".into()), None, ui::stack(lines))
}

fn field_row<'a>(state: &'a State, field: &'a PlanningFormField) -> Element<'a, Message> {
    let label = if field.required {
        format!("{} *", field.label)
    } else {
        field.label.clone()
    };

    let id = field.id.clone();
    let text_value = state.answer(&field.id).and_then(|v| v.as_str()).unwrap_or("");

    let control: Element<'a, Message> = match field.kind.as_str() {
        "boolean" => {
            let picked = state.answer(&field.id).and_then(|v| v.as_bool());
            let no_id = id.clone();
            ui::segmented(vec![
                ("Yes", picked == Some(true), Message::SetBool(id, true)),
                ("No", picked == Some(false), Message::SetBool(no_id, false)),
            ])
        }
        "single_select" => ui::select(
            "Choose…",
            field.options.clone(),
            state.answer(&field.id).and_then(|v| v.as_str()).map(str::to_string),
            move |choice: String| Message::Pick(id.clone(), choice),
        ),
        "multi_select" => ui::chips(field.options.iter().map(move |o| {
            (o.as_str(), state.picked(&field.id, o), Message::ToggleOption(field.id.clone(), o.clone()))
        })),
        // An unknown kind is still answerable as free text; a field this client
        // cannot draw must not become a field the user cannot fill.
        _ => ui::input(
            field.placeholder.as_deref().unwrap_or("Your answer"),
            text_value,
            move |v| Message::SetText(id.clone(), v),
        ),
    };

    let mut lines: Vec<Element<'a, Message>> = vec![ui::caption(label)];
    if let Some(help) = field.help_text.as_ref().filter(|h| !h.is_empty()) {
        lines.push(ui::caption(help.clone()));
    }
    lines.push(control);
    ui::stack(lines).into()
}

fn composer(state: &State) -> Element<'_, Message> {
    ui::card(
        ui::cluster(vec![
            container(ui::input_submit(
                "Ask the assistant to plan something…",
                &state.draft,
                Message::DraftChanged,
                Message::Send,
            ))
            .width(Length::Fill)
            .into(),
            if state.sending {
                ui::badge("thinking…", Tone::Info)
            } else {
                ui::button_default(Icon::Send, "Send", Message::Send)
            },
        ]),
    )
}
