//! The assistant's planning chat — the half of the Phase 7 roadmap that never
//! got a native surface. Agenda shows the board; this is the conversation that
//! *proposes* what goes on it.
//!
//! It sits beside the board rather than on a screen of its own, because every
//! action it offers lands two inches to the left: approve a proposal here and
//! the rows appear there, in the same frame.
//!
//! Unlike [`crate::chat`], the thread is the server's. Nothing is streamed —
//! `/assistant/chat/send` plans actions and answers in one blocking call — so a
//! turn is a single slow request, and the user's own message is shown optimistically
//! while it runs.

use crate::domain::err_string;
use agent_platform_client::types::*;
use agent_platform_client::Client;
use iced::widget::markdown;
use iced::Task;
use std::collections::HashMap;

/// A thread in the picker. The list is titles, which collide ("New chat" twice
/// over), so the option carries the id and compares on it.
#[derive(Debug, Clone)]
pub struct ThreadOption {
    pub id: i64,
    pub label: String,
}

impl PartialEq for ThreadOption {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl std::fmt::Display for ThreadOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

#[derive(Debug, Default)]
pub struct State {
    /// The pane is closed until asked for: the board is the screen's subject and
    /// a chat that opens itself would take half of it uninvited.
    pub open: bool,
    pub project: Option<i64>,
    pub threads: Vec<AssistantThreadSummary>,
    pub thread_id: Option<i64>,
    pub messages: Vec<AssistantChatMessage>,
    /// Parsed markdown per message, same indices — parsed on load, not per frame.
    pub md: Vec<Vec<markdown::Item>>,
    /// What the assistant is offering right now. Distinct from a message's own
    /// `proposed_actions` snapshot, which is history.
    pub pending: Vec<PlannedAction>,
    pub form: Option<PlanningForm>,
    /// Answers to `form`, seeded from each field's `default`.
    pub answers: HashMap<String, serde_json::Value>,
    pub usage: Option<ContextUsage>,
    pub draft: String,
    /// A turn is in flight. Everything that would start another is disabled —
    /// two turns against one thread interleave badly server-side.
    pub sending: bool,
    pub loading: bool,
    pub error: Option<String>,
    /// What the last apply did, when the auto-continue turn did not narrate it.
    pub notice: Option<String>,
}

impl State {
    pub fn scroll_id() -> iced::widget::Id {
        iced::widget::Id::new("agenda-chat-transcript")
    }

    /// The picker is one line high. The server titles a thread from its first
    /// message, so the title alone tells threads apart — the preview is only
    /// worth its width until that lands, which is what "New chat" means.
    pub fn options(&self) -> Vec<ThreadOption> {
        self.threads
            .iter()
            .map(|t| ThreadOption {
                id: t.id,
                label: ellipsize(
                    if t.title.trim().is_empty() || t.title == "New chat" {
                        if t.preview.is_empty() { "New chat" } else { &t.preview }
                    } else {
                        &t.title
                    },
                    44,
                ),
            })
            .collect()
    }

    pub fn selected(&self) -> Option<ThreadOption> {
        let id = self.thread_id?;
        self.options().into_iter().find(|o| o.id == id)
    }

    /// The index of the last user turn — what Retry regenerates from.
    pub fn last_user_index(&self) -> Option<usize> {
        self.messages.iter().rposition(|m| m.role == "user")
    }

    /// A required field with no answer blocks submission, so a form is not spent
    /// on an LLM turn that has to ask again.
    pub fn form_ready(&self) -> bool {
        let Some(form) = &self.form else { return false };
        form.fields.iter().filter(|f| f.required).all(|f| answered(self.answers.get(&f.id)))
    }

    pub fn answer(&self, id: &str) -> Option<&serde_json::Value> {
        self.answers.get(id)
    }

    /// Is `option` picked for a `multi_select` field?
    pub fn picked(&self, id: &str, option: &str) -> bool {
        match self.answers.get(id) {
            Some(serde_json::Value::Array(items)) => {
                items.iter().any(|v| v.as_str() == Some(option))
            }
            _ => false,
        }
    }

    /// Replace the whole conversation with what the server just returned. Every
    /// route that touches a thread answers with the thread, so this is the one
    /// place state changes and the client never patches its own copy.
    fn absorb(&mut self, thread: AssistantChatThread) {
        if let Some(id) = thread.thread_id {
            self.thread_id = Some(id);
        }
        self.md = thread
            .messages
            .iter()
            .map(|m| markdown::parse(&m.content).collect())
            .collect();
        self.messages = thread.messages;
        self.pending = thread.pending_actions;
        self.seed_form(thread.pending_form);
        if thread.context_usage.is_some() {
            self.usage = thread.context_usage;
        }
    }

    /// A new form arrives with its own defaults; a form that went away takes its
    /// half-typed answers with it rather than leaking them into the next one.
    fn seed_form(&mut self, form: Option<PlanningForm>) {
        self.answers.clear();
        if let Some(form) = &form {
            for field in &form.fields {
                if let Some(default) = field.default.clone().filter(|v| answered(Some(v))) {
                    self.answers.insert(field.id.clone(), default);
                }
            }
        }
        self.form = form;
    }

    /// Show the user's turn before the server has one — the round trip is an
    /// LLM call, and a composer that empties into nothing reads as a dropped
    /// message.
    fn push_local_user_turn(&mut self, content: &str) {
        self.md.push(markdown::parse(content).collect());
        self.messages.push(AssistantChatMessage {
            role: "user".into(),
            content: content.to_string(),
            proposed_actions: Vec::new(),
            proposal_status: None,
        });
    }
}

fn answered(value: Option<&serde_json::Value>) -> bool {
    match value {
        Some(serde_json::Value::String(s)) => !s.trim().is_empty(),
        Some(serde_json::Value::Array(items)) => !items.is_empty(),
        Some(serde_json::Value::Null) | None => false,
        Some(_) => true,
    }
}

fn ellipsize(s: &str, max: usize) -> String {
    let trimmed: String = s.chars().take(max).collect();
    if trimmed.chars().count() < s.chars().count() {
        format!("{trimmed}…")
    } else {
        trimmed
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    /// "View logs" on a traced error banner — intercepted in `main::update`
    /// before it reaches here, so this arm exists only to satisfy exhaustiveness.
    TraceLogs(String),
    Open,
    Close,
    /// The project changed (or the pane opened): reload threads and the newest one.
    Reload,
    ThreadsLoaded(Result<Vec<AssistantThreadSummary>, String>),
    SelectThread(i64),
    NewThread,
    ThreadCreated(Result<i64, String>),
    Loaded(Result<Box<AssistantChatThread>, String>),
    DraftChanged(String),
    Send,
    Retry,
    /// A turn came back — the same shape from send, retry and form submit.
    Turned(Result<Box<AssistantChatThread>, String>),
    SetText(String, String),
    SetBool(String, bool),
    Pick(String, String),
    ToggleOption(String, String),
    SubmitForm,
    ApplyActions,
    DismissActions,
    Applied(Result<AssistantApplyResult, String>),
    LinkClicked(String),
    DismissError,
}

fn load_thread(client: &Client, project: i64, thread: Option<i64>) -> Task<Message> {
    let c = client.clone();
    Task::perform(
        async move { err_string(c.assistant_thread(project, thread).await).map(Box::new) },
        Message::Loaded,
    )
}

fn load_threads(client: &Client, project: i64) -> Task<Message> {
    let c = client.clone();
    Task::perform(
        async move { err_string(c.assistant_threads(project).await).map(|r| r.threads) },
        Message::ThreadsLoaded,
    )
}

/// The project Agenda is showing. Switching projects switches boards, so the
/// thread goes with it.
pub fn set_project(state: &mut State, client: &Client, project: Option<i64>) -> Task<Message> {
    if state.project == project {
        return Task::none();
    }
    state.project = project;
    state.thread_id = None;
    state.threads.clear();
    state.messages.clear();
    state.md.clear();
    state.pending.clear();
    state.seed_form(None);
    state.usage = None;
    state.notice = None;
    if state.open {
        reload(state, client)
    } else {
        Task::none()
    }
}

fn reload(state: &mut State, client: &Client) -> Task<Message> {
    let Some(project) = state.project else { return Task::none() };
    state.loading = true;
    Task::batch([load_threads(client, project), load_thread(client, project, state.thread_id)])
}

pub fn update(state: &mut State, client: &Client, message: Message) -> Task<Message> {
    match message {
        Message::TraceLogs(_) => Task::none(),
        Message::Open => {
            if state.open {
                return Task::none();
            }
            state.open = true;
            // Opened once, kept: reopening a pane that already has the thread
            // should not spend an LLM-free but still round-trip refetch.
            if state.messages.is_empty() {
                return reload(state, client);
            }
            Task::none()
        }
        Message::Close => {
            state.open = false;
            Task::none()
        }
        Message::Reload => reload(state, client),
        Message::ThreadsLoaded(Ok(threads)) => {
            state.threads = threads;
            Task::none()
        }
        Message::SelectThread(id) => {
            if state.thread_id == Some(id) || state.sending {
                return Task::none();
            }
            let Some(project) = state.project else { return Task::none() };
            state.thread_id = Some(id);
            state.loading = true;
            state.notice = None;
            load_thread(client, project, Some(id))
        }
        Message::NewThread => {
            let Some(project) = state.project else { return Task::none() };
            if state.sending {
                return Task::none();
            }
            state.loading = true;
            let c = client.clone();
            Task::perform(
                async move { err_string(c.assistant_new_thread(project).await).map(|r| r.thread_id) },
                Message::ThreadCreated,
            )
        }
        Message::ThreadCreated(Ok(id)) => {
            state.thread_id = Some(id);
            state.notice = None;
            reload(state, client)
        }
        Message::Loaded(Ok(thread)) => {
            state.loading = false;
            state.absorb(*thread);
            iced::widget::operation::snap_to_end(State::scroll_id())
        }
        Message::DraftChanged(v) => {
            state.draft = v;
            Task::none()
        }
        Message::Send => {
            let text = state.draft.trim().to_string();
            let Some(project) = state.project else { return Task::none() };
            if text.is_empty() || state.sending {
                return Task::none();
            }
            state.draft.clear();
            state.push_local_user_turn(&text);
            state.sending = true;
            state.notice = None;
            let (c, thread) = (client.clone(), state.thread_id);
            Task::batch([
                iced::widget::operation::snap_to_end(State::scroll_id()),
                Task::perform(
                    async move {
                        let body = AssistantChatSend { message: text, thread_id: thread };
                        err_string(c.assistant_chat_send(project, &body).await).map(Box::new)
                    },
                    Message::Turned,
                ),
            ])
        }
        Message::Retry => {
            let (Some(project), Some(thread_id)) = (state.project, state.thread_id) else {
                return Task::none();
            };
            let Some(index) = state.last_user_index() else { return Task::none() };
            if state.sending {
                return Task::none();
            }
            state.sending = true;
            state.notice = None;
            let c = client.clone();
            Task::perform(
                async move {
                    let body = AssistantChatRetry { thread_id, message_index: index };
                    err_string(c.assistant_chat_retry(project, &body).await).map(Box::new)
                },
                Message::Turned,
            )
        }
        Message::SetText(id, value) => {
            state.answers.insert(id, serde_json::Value::String(value));
            Task::none()
        }
        Message::SetBool(id, value) => {
            state.answers.insert(id, serde_json::Value::Bool(value));
            Task::none()
        }
        Message::Pick(id, value) => {
            state.answers.insert(id, serde_json::Value::String(value));
            Task::none()
        }
        Message::ToggleOption(id, option) => {
            let entry =
                state.answers.entry(id).or_insert_with(|| serde_json::Value::Array(Vec::new()));
            if !entry.is_array() {
                *entry = serde_json::Value::Array(Vec::new());
            }
            if let Some(items) = entry.as_array_mut() {
                match items.iter().position(|v| v.as_str() == Some(option.as_str())) {
                    Some(i) => {
                        items.remove(i);
                    }
                    None => items.push(serde_json::Value::String(option)),
                }
            }
            Task::none()
        }
        Message::SubmitForm => {
            let Some(project) = state.project else { return Task::none() };
            let Some(form) = &state.form else { return Task::none() };
            if state.sending || !state.form_ready() {
                return Task::none();
            }
            // "general" is not a guess: the server prefers the pending action's
            // own domain over it, and falls back to the routed profile's.
            let domain = form.domain.clone().unwrap_or_else(|| "general".into());
            let answers = state.answers.clone();
            let (c, thread) = (client.clone(), state.thread_id);
            state.sending = true;
            state.notice = None;
            Task::perform(
                async move {
                    let body = AssistantFormSubmit {
                        domain,
                        answers,
                        thread_id: thread,
                        auto_continue: true,
                    };
                    err_string(c.assistant_submit_form(project, &body).await).map(Box::new)
                },
                Message::Turned,
            )
        }
        Message::Turned(Ok(thread)) => {
            state.sending = false;
            state.absorb(*thread);
            iced::widget::operation::snap_to_end(State::scroll_id())
        }
        Message::ApplyActions => apply(state, client, false),
        Message::DismissActions => apply(state, client, true),
        Message::Applied(Ok(result)) => {
            state.sending = false;
            // The apply itself already landed; the continuation turn is allowed
            // to fail, so what changed is reported from the apply's own result —
            // but only when that turn is missing. When it came back it carries
            // the same summary into the transcript, and a banner saying it again
            // is one line of duplicate.
            if result.continuation.is_none() {
                let mut parts: Vec<String> = Vec::new();
                if !result.applied.is_empty() {
                    parts.push(format!("Applied: {}", result.applied.join("; ")));
                }
                if !result.skipped.is_empty() {
                    parts.push(format!("Skipped: {}", result.skipped.join("; ")));
                }
                parts.extend(result.guidance);
                state.notice = (!parts.is_empty()).then(|| parts.join(" "));
            }
            reload(state, client)
        }
        Message::LinkClicked(url) => {
            if url.starts_with("http://") || url.starts_with("https://") {
                crate::shell::reveal_path(&url);
            }
            Task::none()
        }
        Message::DismissError => {
            state.error = None;
            state.notice = None;
            Task::none()
        }

        Message::ThreadsLoaded(Err(e))
        | Message::ThreadCreated(Err(e))
        | Message::Loaded(Err(e))
        | Message::Turned(Err(e))
        | Message::Applied(Err(e)) => {
            state.loading = false;
            state.sending = false;
            state.error = Some(e);
            Task::none()
        }
    }
}

/// Approve or dismiss the pending proposal. Dismissing is the same call with an
/// empty action list — the server resolves the thread's snapshot either way, and
/// a dismissal that left the snapshot pending would re-offer it on reopen.
fn apply(state: &mut State, client: &Client, dismiss: bool) -> Task<Message> {
    let Some(project) = state.project else { return Task::none() };
    if state.sending {
        return Task::none();
    }
    let actions = if dismiss { Vec::new() } else { state.pending.clone() };
    let (c, thread) = (client.clone(), state.thread_id);
    state.sending = true;
    state.notice = None;
    Task::perform(
        async move {
            let body = AssistantApplyBody {
                actions,
                thread_id: thread,
                // A dismissal has nothing to narrate; an approval does.
                auto_continue: !dismiss,
            };
            err_string(c.assistant_apply_actions(project, &body).await)
        },
        Message::Applied,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Client {
        Client::new("http://127.0.0.1:1", "k")
    }

    fn field(id: &str, kind: &str, required: bool) -> PlanningFormField {
        PlanningFormField {
            id: id.into(),
            label: id.into(),
            kind: kind.into(),
            required,
            options: vec!["a".into(), "b".into()],
            placeholder: None,
            help_text: None,
            default: None,
        }
    }

    fn form(fields: Vec<PlanningFormField>) -> PlanningForm {
        PlanningForm {
            purpose: None,
            title: None,
            description: None,
            domain: Some("fitness".into()),
            fields,
        }
    }

    fn open(form: Option<PlanningForm>) -> State {
        let mut state = State { open: true, project: Some(1), ..State::default() };
        state.seed_form(form);
        state
    }

    /// A form arrives prefilled from the stored profile; those defaults are the
    /// answers unless the user changes them, or every re-asked field is retyped.
    #[test]
    fn defaults_seed_the_answers_but_blank_ones_do_not() {
        let mut filled = field("sex", "single_select", false);
        filled.default = Some(serde_json::json!("a"));
        let mut blank = field("notes", "text", false);
        blank.default = Some(serde_json::json!(""));
        let state = open(Some(form(vec![filled, blank])));
        assert_eq!(state.answer("sex"), Some(&serde_json::json!("a")));
        assert_eq!(state.answer("notes"), None, "an empty default is not an answer");
    }

    /// A form that is replaced must not leave the previous one's answers behind —
    /// they would be submitted against fields that no longer exist.
    #[test]
    fn a_new_form_clears_the_old_answers() {
        let mut state = open(Some(form(vec![field("sex", "text", false)])));
        let _ = update(&mut state, &client(), Message::SetText("sex".into(), "x".into()));
        state.seed_form(Some(form(vec![field("age", "text", false)])));
        assert!(state.answers.is_empty());
    }

    #[test]
    fn required_fields_gate_the_submit() {
        let mut state = open(Some(form(vec![field("sex", "text", true)])));
        assert!(!state.form_ready());
        let _ = update(&mut state, &client(), Message::SetText("sex".into(), "  ".into()));
        assert!(!state.form_ready(), "whitespace is not an answer");
        let _ = update(&mut state, &client(), Message::SetText("sex".into(), "male".into()));
        assert!(state.form_ready());
    }

    #[test]
    fn multi_select_toggles_both_ways() {
        let mut state = open(Some(form(vec![field("equipment", "multi_select", false)])));
        let toggle = |s: &mut State| {
            let _ = update(
                s,
                &client(),
                Message::ToggleOption("equipment".into(), "a".into()),
            );
        };
        toggle(&mut state);
        assert!(state.picked("equipment", "a"));
        toggle(&mut state);
        assert!(!state.picked("equipment", "a"));
    }

    /// The composer must not empty into nothing while a slow turn runs.
    #[test]
    fn sending_shows_the_user_turn_immediately() {
        let mut state = State { open: true, project: Some(1), draft: " plan my week ".into(), ..State::default() };
        let _ = update(&mut state, &client(), Message::Send);
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].content, "plan my week");
        assert_eq!(state.md.len(), state.messages.len());
        assert!(state.draft.is_empty());
        assert!(state.sending);
    }

    /// Two turns against one thread interleave badly server-side.
    #[test]
    fn a_second_send_is_ignored_while_one_is_in_flight() {
        let mut state =
            State { open: true, project: Some(1), draft: "hi".into(), sending: true, ..State::default() };
        let _ = update(&mut state, &client(), Message::Send);
        assert!(state.messages.is_empty());
        assert_eq!(state.draft, "hi");
    }

    /// The server's copy of the thread is the truth: the optimistic turn is
    /// replaced by what came back, not appended to.
    #[test]
    fn a_reply_replaces_the_local_transcript() {
        let mut state = State { open: true, project: Some(1), draft: "hi".into(), ..State::default() };
        let _ = update(&mut state, &client(), Message::Send);
        let thread = AssistantChatThread {
            thread_id: Some(4),
            messages: vec![
                AssistantChatMessage {
                    role: "user".into(),
                    content: "hi".into(),
                    proposed_actions: Vec::new(),
                    proposal_status: None,
                },
                AssistantChatMessage {
                    role: "assistant".into(),
                    content: "hello".into(),
                    proposed_actions: Vec::new(),
                    proposal_status: None,
                },
            ],
            ..AssistantChatThread::default()
        };
        let _ = update(&mut state, &client(), Message::Turned(Ok(Box::new(thread))));
        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.thread_id, Some(4));
        assert!(!state.sending);
    }

    /// The picker is one line; a label that wraps pushes the whole pane down.
    #[test]
    fn thread_labels_stay_one_line() {
        let thread = |title: &str, preview: &str| AssistantThreadSummary {
            id: 1,
            title: title.into(),
            message_count: 2,
            preview: preview.into(),
            updated_at: String::new(),
        };
        let mut state = State::default();
        state.threads = vec![
            thread("Weekly review routine", "add a task to review my week"),
            thread("New chat", "something I typed before it was titled"),
            thread(&"x".repeat(80), ""),
        ];
        let labels: Vec<String> = state.options().into_iter().map(|o| o.label).collect();
        assert_eq!(labels[0], "Weekly review routine", "the title carries it alone");
        assert!(labels[1].starts_with("something I typed"), "an untitled thread falls back");
        assert!(labels[2].chars().count() <= 45);
    }

    #[test]
    fn retry_targets_the_last_user_turn() {
        let mut state = State::default();
        for (role, content) in [("user", "a"), ("assistant", "b"), ("user", "c"), ("assistant", "d")] {
            state.messages.push(AssistantChatMessage {
                role: role.into(),
                content: content.into(),
                proposed_actions: Vec::new(),
                proposal_status: None,
            });
        }
        assert_eq!(state.last_user_index(), Some(2));
    }

    /// Switching project switches board, and a thread belongs to a board.
    #[test]
    fn changing_project_drops_the_thread() {
        let mut state = State {
            open: true,
            project: Some(1),
            thread_id: Some(9),
            ..State::default()
        };
        state.messages.push(AssistantChatMessage {
            role: "user".into(),
            content: "x".into(),
            proposed_actions: Vec::new(),
            proposal_status: None,
        });
        let _ = set_project(&mut state, &client(), Some(2));
        assert_eq!(state.thread_id, None);
        assert!(state.messages.is_empty());
    }
}
