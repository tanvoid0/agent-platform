//! Plans rendering: the board list on the left, the open board's columns on the
//! right. iced has no drag-and-drop worth the code, so a card moves with the
//! chevrons on it — one column per press.

use crate::todos::{shifted, Message, State};
use crate::ui::{self, space, Icon, Tone};
use agent_platform_client::types::{TodoItem, TODO_STATUSES};
use iced::widget::{container, row, scrollable, Column, Row};
use iced::{Element, Length};

/// Column headings: the wire status, then what it is called on screen.
fn label(status: &str) -> &'static str {
    match status {
        "plan" => "Plan",
        "backlog" => "Backlog",
        "in_progress" => "In progress",
        "review" => "Review",
        "done" => "Done",
        _ => "Other",
    }
}

fn tone(status: &str) -> Tone {
    match status {
        "in_progress" => Tone::Info,
        "review" => Tone::Warning,
        "done" => Tone::Success,
        _ => Tone::Neutral,
    }
}

pub fn view(state: &State) -> Element<'_, Message> {
    let mut blocks: Vec<Element<'_, Message>> = Vec::new();
    if let Some(err) = &state.error {
        blocks.push(
            ui::cluster(vec![
                container(ui::alert_error_traced(err, Message::TraceLogs)).width(Length::Fill).into(),
                ui::button_ghost(Icon::X, "Dismiss", Message::Dismiss),
            ])
            .into(),
        );
    }

    if let Some(name) = &state.new_board {
        blocks.push(ui::section(
            "New board",
            None,
            Element::from(ui::cluster(vec![
                Element::from(
                    container(ui::input_submit(
                        "What are you planning?",
                        name,
                        Message::NewBoardNameChanged,
                        Message::CreateBoard,
                    ))
                    .width(Length::Fill),
                ),
                ui::button_default(Icon::Check, "Create", Message::CreateBoard),
                ui::button_ghost(Icon::X, "Cancel", Message::CancelNewBoard),
            ])),
        ));
    }

    blocks.push(if state.boards.is_empty() {
        ui::empty_state_icon(
            Icon::ListChecks,
            "No boards yet. A board is a list of things to do, in columns you \
             move them through.",
        )
    } else {
        row![board_list(state), container(board(state)).width(Length::Fill)]
            .spacing(space::MD)
            .into()
    });

    ui::page(
        "Plans",
        Some(ui::muted("Boards of things to do, moved through their columns by hand.")),
        Some(
            ui::cluster(vec![
                ui::button_secondary(Icon::Refresh, "Refresh", Message::Refresh),
                ui::button_default(Icon::Plus, "New board", Message::NewBoard),
            ])
            .into(),
        ),
        ui::stack_lg(blocks),
    )
}

/// The boards, with the open one selected. Fixed width so the columns beside it
/// keep their place when a board name is long.
fn board_list(state: &State) -> Element<'_, Message> {
    let rows: Vec<Element<'_, Message>> = state
        .boards
        .iter()
        .map(|b| {
            ui::cluster(vec![
                Element::from(
                    container(ui::list_item(
                        Element::from(ui::stack(vec![
                            ui::body(b.name.clone()),
                            ui::caption(ui::count(b.item_count as usize, "item", "items")),
                        ])),
                        state.selected == Some(b.id),
                        Message::Select(b.id),
                    ))
                    .width(Length::Fill),
                ),
                ui::icon_button(Icon::Trash, Message::DeleteBoard(b.id)),
            ])
            .into()
        })
        .collect();

    container(ui::card(ui::stack(rows))).width(240).into()
}

fn board(state: &State) -> Element<'_, Message> {
    let Some(board) = &state.board else {
        return ui::empty_state_icon(Icon::ListChecks, "Pick a board.");
    };

    let composer = ui::cluster(vec![
        container(ui::input_submit(
            "Add to Plan…",
            &state.draft,
            Message::DraftChanged,
            Message::AddItem,
        ))
        .width(Length::Fill)
        .into(),
        ui::button_default(Icon::Plus, "Add", Message::AddItem),
    ]);

    let columns: Vec<Element<'_, Message>> =
        TODO_STATUSES.iter().map(|status| status_column(state, status)).collect();

    ui::card_with_header(
        board.name.clone(),
        board.description.clone().map(ui::muted),
        None,
        Column::with_children(vec![
            composer.into(),
            // Columns scroll sideways rather than squeezing: five of them below
            // ~1000px would leave cards too narrow to read. The `spacing` is the
            // scrollbar's own gutter — iced 0.14 floats it over the content, and
            // without this it sits on the bottom row of every column (an empty
            // one renders as a half-cut "—").
            scrollable(Row::with_children(columns).spacing(space::SM))
                .direction(scrollable::Direction::Horizontal(Default::default()))
                .spacing(space::SM)
                .into(),
        ])
        .spacing(space::MD),
    )
}

fn status_column<'a>(state: &'a State, status: &'a str) -> Element<'a, Message> {
    let items = state.column(status);
    let mut children: Vec<Element<'a, Message>> = vec![ui::cluster(vec![
        ui::badge(label(status), tone(status)),
        ui::caption(items.len().to_string()),
    ])
    .into()];

    if items.is_empty() {
        children.push(ui::caption("—"));
    }
    for item in items {
        children.push(card(state, item));
    }

    container(Column::with_children(children).spacing(space::SM))
        .width(240)
        .padding(space::SM)
        .style(ui::theme::code_block)
        .into()
}

fn card<'a>(state: &'a State, item: &'a TodoItem) -> Element<'a, Message> {
    let mut lines: Vec<Element<'a, Message>> = vec![ui::body(item.title.clone())];

    let mut meta: Vec<Element<'a, Message>> = Vec::new();
    if let Some(category) = state.category(item.category_id) {
        meta.push(ui::badge(category.name.clone(), Tone::Neutral));
    }
    if let Some(due) = &item.due_at {
        meta.push(ui::caption(due.chars().take(10).collect::<String>()));
    }
    if !meta.is_empty() {
        lines.push(ui::cluster(meta).into());
    }

    // Chevrons stand in for dragging; the one at the end of the board is left
    // out rather than shown dead, so the edges of the flow are visible.
    let mut actions: Vec<Element<'a, Message>> = Vec::new();
    if shifted(&item.status, -1).is_some() {
        actions.push(ui::icon_button(Icon::ChevronLeft, Message::MoveItem(item.id, -1)));
    }
    if shifted(&item.status, 1).is_some() {
        actions.push(ui::icon_button(Icon::ChevronRight, Message::MoveItem(item.id, 1)));
    }
    actions.push(ui::spacer());
    actions.push(ui::icon_button(Icon::Trash, Message::DeleteItem(item.id)));
    lines.push(ui::cluster(actions).into());

    ui::card(ui::stack(lines))
}
