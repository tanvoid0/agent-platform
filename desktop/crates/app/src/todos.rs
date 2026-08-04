//! Plans — the todo boards the server has always had and no native screen ever
//! showed (they were a web-only surface until now).
//!
//! One board at a time: pick it on the left, and its items sit in columns by
//! status. Columns are statuses rather than categories because a status is the
//! thing a card *moves* through; the category rides along as a badge.

use agent_platform_client::types::*;
use agent_platform_client::Client;
use iced::Task;

#[derive(Debug, Clone, Default)]
pub struct State {
    pub boards: Vec<TodoBoardSummary>,
    /// The open board, with its categories and items.
    pub board: Option<TodoBoardDetail>,
    pub selected: Option<i64>,
    /// Draft title for the new-item composer.
    pub draft: String,
    /// Draft name for a new board; `None` while the composer is closed.
    pub new_board: Option<String>,
    pub busy: bool,
    pub error: Option<String>,
}

impl State {
    /// Items in a column, highest priority first — the order the board is read.
    pub fn column(&self, status: &str) -> Vec<&TodoItem> {
        let Some(board) = &self.board else { return Vec::new() };
        let mut items: Vec<&TodoItem> =
            board.items.iter().filter(|i| i.status == status).collect();
        items.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.id.cmp(&b.id)));
        items
    }

    pub fn category(&self, id: Option<i64>) -> Option<&TodoCategory> {
        let (board, id) = (self.board.as_ref()?, id?);
        board.categories.iter().find(|c| c.id == id)
    }
}

/// The status a card moves to, or `None` at either end of the board.
pub fn shifted(status: &str, delta: i32) -> Option<&'static str> {
    let at = TODO_STATUSES.iter().position(|s| *s == status)?;
    let next = at as i32 + delta;
    (next >= 0).then(|| TODO_STATUSES.get(next as usize).copied()).flatten()
}

#[derive(Debug, Clone)]
pub enum Message {
    Refresh,
    BoardsLoaded(Result<Vec<TodoBoardSummary>, String>),
    BoardLoaded(Result<Box<TodoBoardDetail>, String>),
    Select(i64),

    NewBoard,
    NewBoardNameChanged(String),
    CancelNewBoard,
    CreateBoard,
    DeleteBoard(i64),

    DraftChanged(String),
    AddItem,
    /// Move a card one column left (-1) or right (+1).
    MoveItem(i64, i32),
    DeleteItem(i64),
    /// Any write finished; the board is refetched rather than patched locally.
    Done(Result<(), String>),
    Dismiss,
}

fn err_string<T>(r: agent_platform_client::Result<T>) -> Result<T, String> {
    r.map_err(|e| e.to_string())
}

pub fn refresh(client: &Client) -> Task<Message> {
    let c = client.clone();
    Task::perform(async move { err_string(c.todo_boards().await).map(|r| r.boards) }, Message::BoardsLoaded)
}

fn load_board(client: &Client, id: i64) -> Task<Message> {
    let c = client.clone();
    Task::perform(
        async move { err_string(c.todo_board(id).await).map(Box::new) },
        Message::BoardLoaded,
    )
}

pub fn update(state: &mut State, client: &Client, message: Message) -> Task<Message> {
    match message {
        Message::Refresh => match state.selected {
            Some(id) => Task::batch([refresh(client), load_board(client, id)]),
            None => refresh(client),
        },
        Message::BoardsLoaded(Ok(boards)) => {
            // Open the first board on a cold start, so the screen is never an
            // empty pane next to a populated list.
            let first = boards.first().map(|b| b.id);
            state.boards = boards;
            match state.selected {
                Some(id) if state.boards.iter().any(|b| b.id == id) => Task::none(),
                _ => match first {
                    Some(id) => {
                        state.selected = Some(id);
                        load_board(client, id)
                    }
                    None => {
                        state.board = None;
                        state.selected = None;
                        Task::none()
                    }
                },
            }
        }
        Message::BoardLoaded(Ok(board)) => {
            state.selected = Some(board.id);
            state.board = Some(*board);
            Task::none()
        }
        Message::Select(id) => {
            state.selected = Some(id);
            load_board(client, id)
        }

        Message::NewBoard => {
            state.new_board = Some(String::new());
            Task::none()
        }
        Message::NewBoardNameChanged(name) => {
            state.new_board = Some(name);
            Task::none()
        }
        Message::CancelNewBoard => {
            state.new_board = None;
            Task::none()
        }
        Message::CreateBoard => {
            let name = state.new_board.clone().unwrap_or_default().trim().to_string();
            if name.is_empty() {
                return Task::none();
            }
            state.new_board = None;
            state.busy = true;
            let c = client.clone();
            Task::perform(
                async move {
                    err_string(c.create_todo_board(&TodoBoardBody { name, description: None }).await)
                        .map(|_| ())
                },
                Message::Done,
            )
        }
        Message::DeleteBoard(id) => {
            state.busy = true;
            if state.selected == Some(id) {
                state.selected = None;
                state.board = None;
            }
            let c = client.clone();
            Task::perform(async move { err_string(c.delete_todo_board(id).await) }, Message::Done)
        }

        Message::DraftChanged(title) => {
            state.draft = title;
            Task::none()
        }
        Message::AddItem => {
            let title = state.draft.trim().to_string();
            let Some(board) = state.selected else { return Task::none() };
            if title.is_empty() {
                return Task::none();
            }
            state.draft.clear();
            state.busy = true;
            let c = client.clone();
            Task::perform(
                async move {
                    err_string(
                        c.create_todo_item(board, &TodoItemBody { title, category_id: None }).await,
                    )
                    .map(|_| ())
                },
                Message::Done,
            )
        }
        Message::MoveItem(id, delta) => {
            let Some(item) = state
                .board
                .as_ref()
                .and_then(|b| b.items.iter().find(|i| i.id == id))
            else {
                return Task::none();
            };
            let Some(status) = shifted(&item.status, delta) else { return Task::none() };
            state.busy = true;
            let c = client.clone();
            let patch = TodoItemPatch { status: Some(status.to_string()), ..Default::default() };
            Task::perform(
                async move { err_string(c.update_todo_item(id, &patch).await).map(|_| ()) },
                Message::Done,
            )
        }
        Message::DeleteItem(id) => {
            state.busy = true;
            let c = client.clone();
            Task::perform(async move { err_string(c.delete_todo_item(id).await) }, Message::Done)
        }
        Message::Done(Ok(())) => {
            state.busy = false;
            update(state, client, Message::Refresh)
        }
        Message::Dismiss => {
            state.error = None;
            Task::none()
        }

        Message::BoardsLoaded(Err(e))
        | Message::BoardLoaded(Err(e))
        | Message::Done(Err(e)) => {
            state.busy = false;
            state.error = Some(e);
            Task::none()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_stops_at_both_ends_of_the_board() {
        assert_eq!(shifted("plan", -1), None);
        assert_eq!(shifted("plan", 1), Some("backlog"));
        assert_eq!(shifted("review", 1), Some("done"));
        assert_eq!(shifted("done", 1), None);
        // A status the server grew that this build does not know about must not
        // panic or teleport the card to the first column.
        assert_eq!(shifted("archived", 1), None);
    }

    fn board_with(items: Vec<(i64, &str, i64)>) -> TodoBoardDetail {
        TodoBoardDetail {
            id: 1,
            name: "b".into(),
            description: None,
            categories: vec![],
            items: items
                .into_iter()
                .map(|(id, status, priority)| TodoItem {
                    id,
                    category_id: None,
                    title: format!("item {id}"),
                    description: String::new(),
                    status: status.into(),
                    priority,
                    tags: vec![],
                    due_at: None,
                })
                .collect(),
        }
    }

    #[test]
    fn columns_split_by_status_and_sort_by_priority() {
        let state = State {
            board: Some(board_with(vec![(1, "plan", 0), (2, "done", 0), (3, "plan", 5)])),
            ..State::default()
        };
        let plan: Vec<i64> = state.column("plan").iter().map(|i| i.id).collect();
        assert_eq!(plan, vec![3, 1], "higher priority first");
        assert_eq!(state.column("done").len(), 1);
        assert!(state.column("in_progress").is_empty());
    }
}
