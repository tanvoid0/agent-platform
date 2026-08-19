//! Agenda rendering: what to do now on top, what the reviewer thinks under it.

use crate::agenda::{Message, State};
use crate::agenda_chat;
use crate::agenda_chat_view;
use crate::ui::{self, space, Icon, Tone};
use agent_platform_client::types::{AssistantReview, TodoItem, ASSISTANT_HORIZONS};
use iced::widget::{container, row, scrollable, Row};
use iced::{Element, Length, Theme};

fn horizon_label(horizon: &str) -> &'static str {
    match horizon {
        "day" => "Today",
        "week" => "This week",
        "month" => "This month",
        _ => "Ahead",
    }
}

pub fn view<'a>(state: &'a State, theme: &Theme) -> Element<'a, Message> {
    let mut blocks: Vec<Element<'a, Message>> = Vec::new();

    if let Some(err) = &state.error {
        blocks.push(ui::error_bar(err, Message::TraceLogs, Message::Dismiss, Vec::new()));
    }

    blocks.push(picker(state));

    if state.projects.is_empty() {
        blocks.push(if state.loading {
            ui::empty_state("Loading…")
        } else {
            ui::empty_state_icon(
                Icon::Folder,
                "No projects yet. The assistant keeps one board per project — \
                 make a project first.",
            )
        });
        return page(state, theme, blocks);
    }

    let Some(dashboard) = &state.dashboard else {
        blocks.push(if state.loading {
            ui::empty_state("Loading…")
        } else {
            ui::empty_state_icon(Icon::Clock, "Pick a project.")
        });
        return page(state, theme, blocks);
    };

    let stats = &dashboard.stats;
    blocks.push(
        Row::with_children(vec![
            ui::stat(Icon::ListChecks, "Active", stats.active_count.to_string()),
            ui::stat(Icon::CheckCircle, "Done", stats.done_count.to_string()),
            ui::stat(Icon::Alert, "Overdue", stats.overdue_count.to_string()),
            ui::stat(Icon::Refresh, "Habits due", stats.habits_due_count.to_string()),
        ])
        .spacing(space::SM)
        .into(),
    );

    for review in &state.reviews {
        blocks.push(review_banner(review));
    }

    blocks.extend(section(state, "Overdue", &dashboard.overdue, None));
    blocks.extend(section(
        state,
        horizon_label(&dashboard.horizon),
        &dashboard.items,
        Some("Nothing scheduled for this stretch."),
    ));
    blocks.extend(section(state, "Habits", &dashboard.habits_due, None));
    blocks.extend(section(state, "Goals", &dashboard.goals, None));

    page(state, theme, blocks)
}

fn page<'a>(
    state: &'a State,
    theme: &Theme,
    blocks: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    let review = if state.busy {
        ui::button_sized(
            Some(Icon::Clock),
            "Working…",
            ui::ButtonVariant::Secondary,
            ui::Size::Default,
            None,
        )
    } else {
        ui::button_secondary(Icon::Sparkles, "Run review", Message::RunReview)
    };

    let description = Some(ui::muted(
        "Your day: what is due and what slipped. Agent runs live on Processes; \
         lists you move by hand live on Plans.",
    ));
    let actions = Some(
        ui::cluster(vec![
            if state.chat.open {
                ui::button_ghost(Icon::Message, "Hide assistant", Message::Chat(agenda_chat::Message::Close))
            } else {
                ui::button_secondary(Icon::Message, "Ask assistant", Message::Chat(agenda_chat::Message::Open))
            },
            review,
            ui::button_default(Icon::Refresh, "Refresh", Message::Refresh),
        ])
        .into(),
    );

    if !state.chat.open {
        return ui::page("Agenda", description, actions, ui::stack_lg(blocks));
    }

    // Board and chat share the page, so the page itself must not scroll: the
    // board scrolls on its own and the chat pins its composer. `ui::page` would
    // nest both inside a third scrollable.
    ui::page_fixed(
        "Agenda",
        description,
        actions,
        row![
            container(
                scrollable(ui::stack_lg(blocks))
                    .spacing(space::SM)
                    .height(Length::Fill)
            )
            .width(Length::Fill),
            ui::separator_vertical(),
            agenda_chat_view::pane(&state.chat, theme).map(Message::Chat),
        ]
        .spacing(space::MD),
    )
}

/// Project on the left, horizon on the right — the two things that decide what
/// the rest of the page shows.
fn picker(state: &State) -> Element<'_, Message> {
    let names: Vec<String> = state.projects.iter().map(|p| p.name.clone()).collect();
    let selected = state.project.and_then(|id| state.project_name(id)).map(str::to_string);
    let by_name: Vec<(String, i64)> =
        state.projects.iter().map(|p| (p.name.clone(), p.id)).collect();

    let horizons = ASSISTANT_HORIZONS.iter().map(|h| {
        (horizon_label(h), state.horizon == *h, Message::SetHorizon((*h).to_string()))
    });

    ui::card(
        row![
            container(ui::select("Project", names, selected, move |name: String| {
                Message::SelectProject(
                    by_name.iter().find(|(n, _)| *n == name).map(|(_, id)| *id).unwrap_or_default(),
                )
            }))
            .width(Length::Fill),
            ui::segmented(horizons),
        ]
        .spacing(space::MD)
        .align_y(iced::Alignment::Center),
    )
}

/// A titled list of cards. An empty section is dropped rather than shown as a
/// header over nothing — except the horizon's own list, which says so.
fn section<'a>(
    state: &'a State,
    title: &'a str,
    items: &'a [TodoItem],
    empty: Option<&'a str>,
) -> Option<Element<'a, Message>> {
    if items.is_empty() {
        return empty.map(|message| ui::section(title, None, ui::empty_state(message)));
    }

    let rows: Vec<Element<'a, Message>> = items.iter().map(|i| card(state, i)).collect();
    Some(ui::section(
        title,
        Some(ui::caption(ui::count(items.len(), "item", "items"))),
        ui::stack(rows),
    ))
}

fn card<'a>(state: &'a State, item: &'a TodoItem) -> Element<'a, Message> {
    let mut meta: Vec<Element<'a, Message>> = Vec::new();
    if let Some(category) = state.category(item.category_id) {
        meta.push(ui::badge(category.name.clone(), Tone::Neutral));
    }
    if let Some(due) = &item.due_at {
        meta.push(ui::caption(due.chars().take(10).collect::<String>()));
    }
    if item.status == "done" {
        meta.push(ui::badge("Done", Tone::Success));
    }

    let mut lines: Vec<Element<'a, Message>> = vec![ui::body(item.title.clone())];
    if !meta.is_empty() {
        lines.push(ui::cluster(meta).into());
    }

    let action: Element<'a, Message> = if item.status == "done" {
        ui::caption("")
    } else {
        ui::button_ghost(Icon::Check, "Complete", Message::Complete(item.id))
    };

    ui::card(
        ui::cluster(vec![
            container(ui::stack(lines)).width(Length::Fill).into(),
            action,
        ])
        .align_y(iced::Alignment::Center),
    )
}

/// What the reviewer proposed. Applying takes every action it listed — picking
/// them apart is the chat's job, not this screen's.
fn review_banner(review: &AssistantReview) -> Element<'_, Message> {
    let mut lines: Vec<Element<'_, Message>> = Vec::new();
    if let Some(summary) = &review.summary {
        lines.push(ui::body(summary.clone()));
    }
    for action in &review.proposed_actions {
        let text = match &action.reasoning {
            Some(why) => format!("{} — {why}", action.name),
            None => action.name.clone(),
        };
        lines.push(ui::caption(text));
    }
    lines.push(
        ui::cluster(vec![
            ui::button_default(
                Icon::Check,
                if review.proposed_actions.len() == 1 { "Apply change" } else { "Apply changes" },
                Message::ApplyReview(review.id),
            ),
            ui::button_ghost(Icon::X, "Dismiss", Message::DismissReview(review.id)),
        ])
        .into(),
    );

    ui::alert(Tone::Info, "Review", Some(ui::stack(lines).into()))
}
