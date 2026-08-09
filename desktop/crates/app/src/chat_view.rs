//! Chat rendering: transcript scrolls, composer stays pinned below it.
//!
//! The plain half of the pair — same transcript shape as E.V.'s, without the
//! HUD, the voice or the persona. Model override lives in the header, not the
//! composer: it is a setting for the thread, not part of writing a message.

use crate::chat::{Message, State};
use crate::ui::{self, space, Icon, Tone};
use iced::widget::{column, container, markdown, scrollable};
use iced::{Element, Length, Theme};

/// Transcript + composer, with no page chrome — the screen wraps it in a page,
/// the processes pane wraps it in a card.
///
/// `height` is the caller's: the screen gives the transcript the window, a card
/// inside a scrolling pane has to cap it or it fights the outer scroll.
pub fn panel<'a>(
    state: &'a State,
    iced_theme: &Theme,
    empty_hint: &'static str,
    height: Length,
) -> Element<'a, Message> {
    let transcript: Element<'_, Message> = if state.messages.is_empty() {
        ui::empty_state_icon(Icon::Message, empty_hint)
    } else {
        let turns: Vec<Element<'_, Message>> = state
            .messages
            .iter()
            .zip(&state.md)
            .enumerate()
            .map(|(i, (m, items))| {
                let is_user = m.role == "user";
                let (label, tone) = match m.role.as_str() {
                    "user" => ("You", Tone::Neutral),
                    "assistant" => ("Assistant", Tone::Info),
                    other => (other, Tone::Neutral),
                };
                let mut parts: Vec<Element<'_, Message>> = Vec::new();
                // A reasoning model's chain-of-thought rides above the reply:
                // open while it streams (before the answer starts), collapsed
                // behind a toggle after.
                let reasoning = state.reasoning.get(i).map(String::as_str).unwrap_or("");
                if !reasoning.is_empty() {
                    let open = state.reasoning_live(i) || state.reasoning_open.contains(&i);
                    parts.push(ui::thinking(reasoning, open, Message::ToggleReasoning(i)));
                }
                parts.push(if is_user {
                    ui::body(m.content.clone())
                } else {
                    markdown::view(items, markdown::Settings::from(iced_theme))
                        .map(Message::LinkClicked)
                });
                ui::turn(label, tone, is_user, column(parts).spacing(space::XS).into())
            })
            .collect();
        scrollable(
            ui::stack_lg(turns)
                .padding(iced::Padding { right: 12.0, ..Default::default() }),
        )
        .id(state.scroll_id())
        .height(height)
        .into()
    };

    let composer = ui::card(
        ui::cluster(vec![
            container(ui::input_submit(
                "Message…",
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
    );

    let mut blocks: Vec<Element<'_, Message>> = Vec::new();
    if let Some(err) = &state.error {
        blocks.push(ui::error_bar(err, Message::TraceLogs, Message::DismissError, Vec::new()));
    }
    // An empty transcript keeps its natural height inside a capped panel, so it
    // does not stretch its card. A filling caller is the whole window, though —
    // there shrinking floats the composer up into the middle of the page.
    let filled = matches!(height, Length::Fill);
    let body_height =
        if state.messages.is_empty() && !filled { Length::Shrink } else { height };
    blocks.push(container(transcript).height(body_height).into());
    blocks.push(composer);
    column(blocks)
        .spacing(space::MD)
        .height(if filled { Length::Fill } else { Length::Shrink })
        .into()
}

