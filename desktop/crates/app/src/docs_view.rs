//! The Docs tab: a file rail on the left, the selected file's text on the right.
//! Folders drill in, a file loads into the reader. Content is rendered as
//! markdown — derived PDF text and `.md` both read as prose, and plain text
//! degrades to a single paragraph.

use crate::docs::{Message, State};
use crate::ui::{self, space, Icon};
use iced::widget::{column, container, markdown, row, scrollable};
use iced::{Element, Length, Theme};

const RAIL_WIDTH: f32 = 300.0;

pub fn view<'a>(state: &'a State, theme: &Theme) -> Element<'a, Message> {
    if state.project.is_none() {
        return ui::page(
            "Docs",
            Some(ui::muted("Pick a project to browse its files.")),
            None,
            ui::empty_state_icon(Icon::Folder, "No project selected."),
        );
    }

    let description: Element<'a, Message> = if state.path.is_empty() {
        ui::muted("A project's files — documents, datasets, and ingested PDFs as derived text.")
    } else {
        ui::muted(format!("/{}", state.path))
    };

    let actions: Option<Element<'a, Message>> = (!state.path.is_empty())
        .then(|| ui::button_outline(Icon::ArrowUp, "Up", Message::Up));

    let body = row![
        container(rail(state)).width(RAIL_WIDTH).height(Length::Fill),
        ui::separator_vertical(),
        container(reader(state, theme)).width(Length::Fill).height(Length::Fill),
    ]
    .spacing(space::MD)
    .height(Length::Fill);

    // page_fixed, not page: both panes scroll themselves, and an outer
    // scrollable would nest (the rule in desktop/CLAUDE.md).
    let mut head: Vec<Element<'a, Message>> = Vec::new();
    if let Some(err) = &state.error {
        head.push(ui::error_bar(err, Message::TraceLogs, Message::DismissError, Vec::new()));
    }
    head.push(body.into());

    ui::page_fixed(
        "Docs",
        Some(description),
        actions,
        column(head).spacing(space::MD).height(Length::Fill),
    )
}

fn rail(state: &State) -> Element<'_, Message> {
    if state.entries.is_empty() {
        let msg = if state.loading { "Loading…" } else { "This folder is empty." };
        return container(ui::muted(msg)).padding(space::SM).into();
    }
    let rows: Vec<Element<'_, Message>> = state
        .entries
        .iter()
        .map(|e| {
            let is_dir = e.kind == "dir";
            let glyph = if is_dir { Icon::Folder } else { Icon::Scroll };
            let selected = state.selected.as_deref() == Some(e.path.as_str());
            ui::list_item(
                ui::cluster(vec![ui::icon::glyph(glyph), ui::body(e.name.clone())]),
                selected,
                Message::Open(e.clone()),
            )
        })
        .collect();
    scrollable(column(rows).spacing(space::XS)).spacing(space::SM).height(Length::Fill).into()
}

fn reader<'a>(state: &'a State, theme: &Theme) -> Element<'a, Message> {
    if state.selected.is_none() {
        return ui::empty_state("Select a file to read it.");
    }
    if state.md.is_empty() {
        let msg = if state.loading { "Loading…" } else { "This file is empty." };
        return ui::empty_state(msg);
    }
    let mut blocks: Vec<Element<'a, Message>> = Vec::new();
    if state.content_kind.as_deref() == Some("pdf_derived_markdown") {
        blocks.push(ui::caption("Extracted text from a PDF."));
    }
    blocks.push(
        markdown::view(&state.md, markdown::Settings::from(theme)).map(Message::LinkClicked),
    );
    scrollable(column(blocks).spacing(space::MD))
        .spacing(space::SM)
        .height(Length::Fill)
        .into()
}
