//! Chat rendering: transcript above, composer below.

use crate::chat::{Message, State};
use crate::ui::{self, space, Tone};
use iced::widget::{column, container, scrollable};
use iced::{Element, Length};

pub fn view(state: &State) -> Element<'_, Message> {
    let transcript: Element<'_, Message> = if state.messages.is_empty() {
        ui::empty_state("Ask the platform's model anything.")
    } else {
        let turns: Vec<Element<'_, Message>> = state
            .messages
            .iter()
            .map(|m| {
                let (label, tone) = match m.role.as_str() {
                    "user" => ("You", Tone::Info),
                    "assistant" => ("Assistant", Tone::Success),
                    other => (other, Tone::Neutral),
                };
                ui::card(ui::stack(vec![
                    ui::badge(label, tone),
                    ui::body(m.content.clone()),
                ]))
            })
            .collect();
        scrollable(ui::stack(turns)).height(Length::Fill).anchor_bottom().into()
    };

    let composer_row: Element<'_, Message> = ui::cluster(vec![
            container(ui::input("Message…", &state.draft, Message::DraftChanged))
                .width(Length::Fill)
                .into(),
            container(ui::input("model (optional)", &state.model, Message::ModelChanged))
                .width(180)
                .into(),
            if state.sending {
                ui::badge("thinking…", Tone::Info)
        } else {
            ui::button_default("Send", Message::Send)
        },
    ])
    .into();
    let composer = ui::card(composer_row);

    let mut blocks: Vec<Element<'_, Message>> = Vec::new();
    if let Some(err) = &state.error {
        blocks.push(
            ui::cluster(vec![
                container(ui::alert_error(err.clone())).width(Length::Fill).into(),
                ui::button_ghost("Dismiss", Message::DismissError),
            ])
            .into(),
        );
    }
    blocks.push(container(transcript).height(Length::Fill).into());
    blocks.push(composer);

    ui::page(
        "Chat",
        Some(ui::muted("Talks to the same provider the agents use.")),
        Some(ui::button_outline("Clear", Message::Clear)),
        {
            let body: Element<'_, Message> =
                column(blocks).spacing(space::MD).height(Length::Fill).into();
            body
        },
    )
}
