//! Memory dashboard: everything the assistants remember about you, in one list
//! you can search, edit, add to and delete from.
//!
//! Deliberately plain. Memory is the one feature where a user's first question
//! is "what does it know about me?" — the answer has to be a readable list, not
//! a visualization.

use crate::memory::{Memory, Store};
use crate::ui::{self, space, Icon, Tone};
use iced::widget::{column, container, row};
use iced::{Element, Length};

pub fn view(store: &Store) -> Element<'_, crate::memory::Message> {
    use crate::memory::Message;

    let mut blocks: Vec<Element<'_, Message>> = Vec::new();

    if let Some(err) = &store.error {
        blocks.push(ui::alert(
            Tone::Danger,
            "Memories are not being saved",
            Some(
                ui::stack(vec![
                    ui::muted(err.clone()),
                    ui::button_ghost(Icon::X, "Dismiss", Message::DismissError),
                ])
                .into(),
            ),
        ));
    }

    if !store.enabled {
        blocks.push(ui::alert(
            Tone::Warning,
            "Memory is off",
            Some(ui::muted(
                "Nothing new is learned and nothing below is sent to the model. \
                 What is already here is kept — turn memory back on to use it again.",
            )),
        ));
    }

    // The composer sits above the list: adding a fact by hand is the fastest
    // correction for a memory the model got subtly wrong.
    blocks.push(ui::section(
        "Add a memory",
        None,
        ui::cluster(vec![
            container(ui::input_submit(
                "Something the assistants should always know…",
                &store.draft,
                Message::DraftChanged,
                Message::Add,
            ))
            .width(Length::Fill)
            .into(),
            ui::button_secondary(Icon::Plus, "Add", Message::Add),
        ]),
    ));

    let rows = store.visible();
    let list: Element<'_, Message> = if rows.is_empty() {
        ui::empty_state_icon(
            Icon::Sparkles,
            if store.items.is_empty() {
                "Nothing remembered yet. Talk to Chat or E.V. and durable facts \
                 about you turn up here on their own."
            } else {
                "No memories match that search."
            },
        )
    } else {
        ui::stack(rows.into_iter().map(|m| memory_row(store, m)).collect()).into()
    };

    blocks.push(ui::section(
        "Remembered",
        Some(ui::cluster(vec![
            container(ui::input_icon(
                Icon::Search,
                "Search memories…",
                &store.search,
                Message::SearchChanged,
            ))
            .width(260)
            .into(),
            ui::badge(ui::count(store.items.len(), "memory", "memories"), Tone::Neutral),
        ])
        .into()),
        list,
    ));

    ui::page(
        "Memory",
        Some(ui::muted(
            "Durable facts the assistants picked up from your conversations, and \
             carry into new ones. Everything here is stored locally.",
        )),
        Some({
            let mut actions = vec![ui::button_outline(
                if store.enabled { Icon::Check } else { Icon::X },
                if store.enabled { "Memory on" } else { "Memory off" },
                Message::ToggleEnabled,
            )];
            // A red "Forget all" with nothing to forget is alarm without stakes.
            if !store.items.is_empty() {
                // Two-step: the first click only arms the button, since there is
                // no undo for a wipe.
                if store.confirm_forget {
                    actions.push(ui::button_destructive(
                        Icon::Trash,
                        "Really forget all?",
                        Message::ForgetAll,
                    ));
                    actions.push(ui::button_ghost(Icon::X, "Cancel", Message::CancelForget));
                } else {
                    actions.push(ui::button_destructive(
                        Icon::Trash,
                        "Forget all",
                        Message::ForgetAll,
                    ));
                }
            }
            ui::cluster(actions).into()
        }),
        ui::stack_lg(blocks),
    )
}

/// One remembered fact: read-only until you press the pencil, then an input with
/// Save and Cancel. Only one row is ever editable at a time, so there is never a
/// question of which draft wins.
fn memory_row<'a>(store: &'a Store, m: &'a Memory) -> Element<'a, crate::memory::Message> {
    use crate::memory::Message;

    if let Some((id, draft)) = &store.editing {
        if *id == m.id {
            return ui::card(ui::stack(vec![
                ui::input_submit("", draft, Message::EditChanged, Message::SaveEdit),
                ui::cluster(vec![
                    ui::button_secondary(Icon::Save, "Save", Message::SaveEdit),
                    ui::button_ghost(Icon::X, "Cancel", Message::CancelEdit),
                    ui::spacer(),
                    ui::caption("Clear the text to forget this memory."),
                ])
                .into(),
            ]));
        }
    }

    ui::card(
        row![
            column![
                ui::body(m.text.clone()),
                ui::caption(format!("{} · {}", m.source, m.created)),
            ]
            .spacing(space::XS)
            .width(Length::Fill),
            ui::cluster(vec![
                ui::icon_button(Icon::Pencil, Message::StartEdit(m.id)),
                ui::icon_button(Icon::Trash, Message::Delete(m.id)),
            ]),
        ]
        .spacing(space::SM)
        .align_y(iced::Alignment::Center),
    )
}
