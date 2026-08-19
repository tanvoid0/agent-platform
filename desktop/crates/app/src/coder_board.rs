//! The sessions board — more than one Coder session at a time.
//!
//! A session is a [`crate::coder::State`]: one thread, one transcript, one
//! stream, one queue, one set of checkpoints. This holds N of them and puts one
//! on screen. Everything the screen already did keeps working on the one in
//! front, because the board derefs to it — `state.sending`, `state.pending`,
//! `state.root` all still read the session the user is looking at.
//!
//! The one thing that could not stay implicit is *which* session a frame belongs
//! to. A stream started by a background session goes on arriving after the user
//! has switched tabs, so every task a session's message produces is tagged with
//! that session's id ([`Message::For`]) and routed back to it. An untagged
//! message — one `main` starts on entering the screen — lands on whichever
//! session is in front, which is what those are for.
//!
//! Two sessions in the *same* checkout are the one thing this refuses: the
//! shadow-git repo the checkpoints live in is one per folder, so two turns
//! writing it would interleave `commit_all` and each checkpoint would hold the
//! other session's work. Either the folders differ, or one of them is Isolated
//! into a worktree of its own — see [`crate::coder::State::busy_roots`].

use crate::coder::{self, Message, State, Status};
use agent_platform_client::Client;
use iced::{Element, Task, Theme};
use std::path::PathBuf;

/// A session and the id the board knows it by. Ids are handed out and never
/// reused, so a frame from a closed session cannot land in the one that took its
/// place in the list.
struct Slot {
    id: u64,
    state: State,
}

/// One session as the sidebar draws it. Kept rather than computed per frame
/// because `view` borrows from `&self` and the widgets want `&'a str`.
#[derive(Debug, Clone)]
pub struct Row {
    pub id: u64,
    pub title: String,
    pub status: Status,
    /// The one on screen.
    pub active: bool,
}

pub struct Board {
    /// Never empty: closing the last session starts a new one in its place
    /// rather than leaving the screen with nothing behind it.
    sessions: Vec<Slot>,
    active: usize,
    seq: u64,
    rows: Vec<Row>,
}

impl Board {
    pub fn new(state: State) -> Self {
        let mut board =
            Self { sessions: vec![Slot { id: 1, state }], active: 0, seq: 1, rows: Vec::new() };
        board.refresh();
        board
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The session on screen. `Deref` reaches it too — this is for the callers
    /// that want to be explicit about which one they mean.
    pub fn active(&self) -> &State {
        &self.sessions[self.active].state
    }

    pub fn get(&self, id: u64) -> Option<&State> {
        self.sessions.iter().find(|s| s.id == id).map(|s| &s.state)
    }

    /// Sessions with a turn in flight, by id. `main` diffs this across an update
    /// to find the one that just finished — the toast is per session, and the
    /// one that matters most is a background session the user is not watching.
    pub fn running(&self) -> Vec<u64> {
        self.sessions.iter().filter(|s| s.state.sending).map(|s| s.id).collect()
    }

    /// Anything worth running the clock and the spinner for, in any session —
    /// a background turn still needs its seconds counted.
    pub fn any_busy(&self) -> bool {
        self.sessions.iter().any(|s| {
            let st = &s.state;
            st.sending || st.pending.is_some() || st.threads_loading || st.checkpoints_loading
        })
    }

    fn index_of(&self, id: u64) -> Option<usize> {
        self.sessions.iter().position(|s| s.id == id)
    }

    fn refresh(&mut self) {
        let active = self.active;
        self.rows = self
            .sessions
            .iter()
            .enumerate()
            .map(|(i, s)| Row {
                id: s.id,
                title: s.state.title(),
                status: s.state.status(),
                active: i == active,
            })
            .collect();
    }
}

impl std::ops::Deref for Board {
    type Target = State;

    fn deref(&self) -> &State {
        &self.sessions[self.active].state
    }
}

impl std::ops::DerefMut for Board {
    fn deref_mut(&mut self) -> &mut State {
        &mut self.sessions[self.active].state
    }
}

pub fn view<'a>(board: &'a Board, iced_theme: &Theme) -> Element<'a, Message> {
    crate::coder_view::view(board.active(), iced_theme, &board.rows)
}

pub fn update(board: &mut Board, client: &Client, message: Message) -> Task<Message> {
    let task = route(board, client, message);
    // One place, so a row can never describe a session as it was before the
    // message that just changed it.
    board.refresh();
    task
}

fn route(board: &mut Board, client: &Client, message: Message) -> Task<Message> {
    match message {
        Message::For(id, inner) => match board.index_of(id) {
            Some(i) => dispatch(board, i, client, *inner),
            // The session was closed while its work was in flight. Closing
            // answered whatever call the server was parked on, so what arrives
            // after it has nowhere to land and nothing to settle.
            None => Task::none(),
        },
        Message::SelectSession(id) => {
            if let Some(i) = board.index_of(id) {
                board.active = i;
            }
            Task::none()
        }
        Message::CloseSession(id) => close(board, client, id),
        // The clock and the spinner belong to the board, not to the tab in
        // front: a background turn's seconds still have to count, or switching
        // back to it shows a turn that has apparently been running for none.
        Message::Tick | Message::AnimTick => {
            let ticks: Vec<Task<Message>> = (0..board.sessions.len())
                .map(|i| dispatch(board, i, client, message.clone()))
                .collect();
            Task::batch(ticks)
        }
        // The New button opens a session beside this one rather than replacing
        // it. The handoff still replaces — it is the same conversation carried
        // over, and two tabs for it would leave a dead one behind.
        Message::New => {
            let fresh = coder::fresh_from(board.active());
            board.seq += 1;
            board.sessions.push(Slot { id: board.seq, state: fresh });
            board.active = board.sessions.len() - 1;
            Task::none()
        }
        other => dispatch(board, board.active, client, other),
    }
}

/// Hand a message to one session, and tag everything it starts with that
/// session's id so the answers come back to it.
fn dispatch(board: &mut Board, i: usize, client: &Client, message: Message) -> Task<Message> {
    let id = board.sessions[i].id;
    // The checkpoint invariant, refreshed per message rather than watched: a
    // turn can only start where no other session is mid-turn.
    //
    // Parked on the approval card counts as mid-turn — `sending` is false there
    // and the turn is very much alive: its checkpoint has not been taken, so a
    // turn starting beside it would have that commit swallow both sessions'
    // changes. Seen on the screen, not in a test.
    let busy: Vec<PathBuf> = board
        .sessions
        .iter()
        .enumerate()
        .filter(|(j, s)| *j != i && (s.state.sending || s.state.pending.is_some()))
        .filter_map(|(_, s)| s.state.root.clone())
        .collect();
    // An allowlist rule belongs to the folder, not to the tab it was approved
    // in — and `main` persists the session on screen, so a rule left in one tab
    // is a rule the next save drops.
    let shares_rules = matches!(message, Message::AlwaysAllow);

    board.sessions[i].state.busy_roots = busy;
    let task = coder::update(&mut board.sessions[i].state, client, message);

    if shares_rules {
        let rules = board.sessions[i].state.allowlist.clone();
        for slot in &mut board.sessions {
            slot.state.allowlist = rules.clone();
        }
    }
    task.map(move |m| Message::For(id, Box::new(m)))
}

fn close(board: &mut Board, client: &Client, id: u64) -> Task<Message> {
    let Some(i) = board.index_of(id) else { return Task::none() };
    // Stop before removing: the server may be blocked on a call this session
    // owes a result for, and a stream dropped while it holds one stalls the turn
    // for the full delegation timeout instead of ending it.
    let stop = dispatch(board, i, client, Message::Stop);

    if board.sessions.len() == 1 {
        // The board is never empty. Closing the only session is starting over in
        // the same folder, which is what New already means.
        board.sessions[0].state = coder::fresh_from(&board.sessions[0].state);
        return stop;
    }
    board.sessions.remove(i);
    if board.active > i {
        board.active -= 1;
    } else {
        board.active = board.active.min(board.sessions.len() - 1);
    }
    stop
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coder::Turn;

    fn client() -> Client {
        Client::new("http://127.0.0.1:1", "k")
    }

    fn board_on(root: &str) -> Board {
        let mut state = State::with_root(root);
        // A thread already exists, as it would after one turn — `Send` opens one
        // over HTTP otherwise and these tests never reach a server.
        state.thread_id = Some(1);
        Board::new(state)
    }

    fn send(board: &mut Board, id: u64, prompt: &str) -> Task<Message> {
        let _ = update(board, &client(), Message::For(id, Box::new(Message::DraftChanged(prompt.into()))));
        update(board, &client(), Message::For(id, Box::new(Message::Send)))
    }

    /// The whole point of the board: a second session runs its own turn, in its
    /// own folder, and the first one does not notice.
    #[test]
    fn two_sessions_on_two_folders_each_run_their_own_turn() {
        let mut board = board_on("D:/work/one");
        let _ = update(&mut board, &client(), Message::New);
        // A new session starts in the same folder — this one is opened elsewhere.
        let second = board.rows()[1].id;
        let _ = update(
            &mut board,
            &client(),
            Message::For(second, Box::new(Message::RootPicked(Some("D:/work/two".into())))),
        );

        let _ = send(&mut board, 1, "first");
        let _ = send(&mut board, second, "second");

        assert_eq!(board.rows().len(), 2);
        assert!(board.get(1).unwrap().sending);
        assert!(board.get(second).unwrap().sending);
        assert_eq!(board.get(1).unwrap().turns.len(), 1, "one turn each, not two in one");
        assert!(matches!(board.get(1).unwrap().turns[0], Turn::User(ref t) if t == "first"));
        assert!(matches!(board.get(second).unwrap().turns[0], Turn::User(ref t) if t == "second"));
        assert!(board.get(1).unwrap().error.is_none());
        assert!(board.get(second).unwrap().error.is_none());
    }

    /// The checkpoint invariant. One shadow repo per folder means one turn per
    /// folder — the second is refused with the way out named, not queued.
    #[test]
    fn a_second_session_will_not_run_a_turn_in_a_folder_already_working() {
        let mut board = board_on("D:/work/one");
        let _ = update(&mut board, &client(), Message::New);
        let second = board.rows()[1].id;

        let _ = send(&mut board, 1, "first");
        let _ = send(&mut board, second, "second");

        assert!(board.get(1).unwrap().sending);
        assert!(!board.get(second).unwrap().sending, "same checkout, so it does not start");
        assert!(board.get(second).unwrap().turns.is_empty(), "and no row for a turn that never ran");
        let refusal = board.get(second).unwrap().error.clone().unwrap_or_default();
        assert!(refusal.contains("Isolate"), "the refusal names the way out: {refusal}");
        // The session that *is* running was not touched by any of it.
        assert!(board.get(1).unwrap().error.is_none());
    }

    /// A turn parked on the approval card has not been checkpointed yet, so it
    /// still holds the folder — `sending` is false there and the turn is not
    /// over.
    #[test]
    fn a_session_waiting_on_an_approval_still_holds_its_checkout() {
        let mut board = board_on("D:/work/one");
        let _ = update(&mut board, &client(), Message::New);
        let second = board.rows()[1].id;

        board.sessions[0].state.pending =
            Some(coder::Pending { call_id: "c1".into(), command: "cargo test".into() });
        assert!(!board.sessions[0].state.sending, "parked, not streaming");

        let _ = send(&mut board, second, "meanwhile");
        assert_eq!(board.rows()[0].status, Status::Awaiting, "and the board says so");
        assert!(!board.get(second).unwrap().sending, "the folder is still spoken for");
        assert!(board.get(second).unwrap().error.is_some());
    }

    /// A frame for a session that has gone is dropped, not applied to whichever
    /// session happens to be in front — that would write one transcript into
    /// another.
    #[test]
    fn a_frame_for_a_closed_session_lands_nowhere() {
        let mut board = board_on("D:/work/one");
        let _ = update(&mut board, &client(), Message::New);
        let second = board.rows()[1].id;
        let _ = send(&mut board, second, "second");

        let _ = update(&mut board, &client(), Message::CloseSession(second));
        assert_eq!(board.rows().len(), 1);
        assert_eq!(board.rows()[0].id, 1);

        let _ = update(
            &mut board,
            &client(),
            Message::For(second, Box::new(Message::DraftChanged("late".into()))),
        );
        assert!(board.get(1).unwrap().draft.is_empty(), "the survivor is untouched");
    }

    /// Closing the only session leaves a session standing: the screen has no
    /// empty state, and this is what New already means.
    #[test]
    fn closing_the_last_session_starts_a_fresh_one_instead() {
        let mut board = board_on("D:/work/one");
        let _ = send(&mut board, 1, "hello");
        assert_eq!(board.turns.len(), 1);

        let _ = update(&mut board, &client(), Message::CloseSession(1));
        assert_eq!(board.rows().len(), 1);
        assert!(board.turns.is_empty(), "the conversation went");
        assert_eq!(board.root_label(), "D:/work/one", "the folder stayed");
    }

    /// A rule approved in one tab is the folder's rule, and `main` persists
    /// whichever tab is in front — so it has to be in all of them or the next
    /// save drops it.
    #[test]
    fn an_always_allow_rule_reaches_every_session() {
        let mut board = board_on("D:/work/one");
        let _ = update(&mut board, &client(), Message::New);
        let second = board.rows()[1].id;

        board.sessions[1].state.pending =
            Some(coder::Pending { call_id: "c1".into(), command: "cargo test --lib".into() });
        let _ = update(&mut board, &client(), Message::For(second, Box::new(Message::AlwaysAllow)));

        let rules = board.get(1).unwrap().rules().to_vec();
        assert_eq!(rules, vec!["cargo test".to_string()], "the other tab has it too");
    }

    /// Switching tabs is what the board is for; the transcript that comes with
    /// it is the one that session was holding.
    #[test]
    fn selecting_a_session_puts_it_on_screen() {
        let mut board = board_on("D:/work/one");
        let _ = send(&mut board, 1, "first");
        let _ = update(&mut board, &client(), Message::New);

        assert!(board.turns.is_empty(), "the new one is on screen and it is empty");
        assert!(board.rows()[1].active);

        let _ = update(&mut board, &client(), Message::SelectSession(1));
        assert_eq!(board.turns.len(), 1);
        assert!(board.rows()[0].active);
        assert_eq!(board.rows()[0].title, "first", "a session is named by what it was asked");
        assert_eq!(board.rows()[1].title, "New session");
        assert_eq!(board.rows()[0].status, Status::Running);
        assert_eq!(board.rows()[1].status, Status::Idle);
    }
}
