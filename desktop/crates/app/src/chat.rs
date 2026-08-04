//! Chat against the platform's LLM proxy. One thread, kept in memory — the
//! server's chat endpoint is stateless, so the whole history is sent each turn.

use agent_platform_client::sse::{self, ChatChunk};
use agent_platform_client::types::{ChatCompletionBody, ChatMessage, ProviderEntry};
use agent_platform_client::Client;
use iced::Task;

/// Identity of the transcript scrollable, so a reply can snap it to the end
/// without anchoring it there permanently (which fights the user's scrolling).
pub fn transcript_id() -> iced::widget::Id {
    iced::widget::Id::new("chat-transcript")
}

pub struct State {
    pub messages: Vec<ChatMessage>,
    /// Parsed markdown per message, same indices as `messages` — parsed once at
    /// push rather than per frame.
    pub md: Vec<Vec<iced::widget::markdown::Item>>,
    /// Chain-of-thought per message, same indices — empty for everything except
    /// assistant turns from a reasoning model. Display-only: never sent back.
    pub reasoning: Vec<String>,
    /// Messages whose thinking section the user has expanded.
    pub reasoning_open: std::collections::HashSet<usize>,
    pub draft: String,
    /// Provider/model override for this thread; empty = the server's default.
    /// The plain screen persists the pair in `shell::Settings`.
    pub provider: String,
    pub model: String,
    /// Providers the proxy knows, for the header dropdowns. Loaded on screen
    /// entry; empty until then (the dropdowns just have nothing to offer).
    pub catalog: Vec<ProviderEntry>,
    pub sending: bool,
    /// An assistant turn is open and collecting deltas — the next one appends to
    /// it rather than starting another bubble. Public so the view can keep the
    /// live turn's thinking section open while it streams.
    pub streaming: bool,
    pub error: Option<String>,
    /// Scope context sent ahead of the thread and never shown in it. Set by
    /// callers that chat *about* something (a run, a subagent); `None` for the
    /// plain screen. Refreshed per send, so it follows the live record.
    pub system: Option<String>,
    /// Each thread needs its own scrollable identity — several can be alive at
    /// once and a reply must snap the right one.
    scroll: iced::widget::Id,
}

impl Default for State {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            md: Vec::new(),
            reasoning: Vec::new(),
            reasoning_open: Default::default(),
            draft: String::new(),
            provider: String::new(),
            model: String::new(),
            catalog: Vec::new(),
            sending: false,
            streaming: false,
            error: None,
            system: None,
            scroll: transcript_id(),
        }
    }
}

impl State {
    /// The plain screen's thread, opened on the persisted provider/model pair.
    pub fn with_defaults(provider: String, model: String) -> Self {
        Self { provider, model, ..Self::default() }
    }

    /// A thread owned by something else on screen, addressed by `key`.
    pub fn scoped(key: &str) -> Self {
        Self { scroll: format!("chat-transcript:{key}").into(), ..Self::default() }
    }

    pub fn scroll_id(&self) -> iced::widget::Id {
        self.scroll.clone()
    }

    /// Replace the thread with a saved conversation (empty = a fresh one).
    /// Callers guard against an in-flight send; the draft is left alone.
    pub fn load_thread(&mut self, messages: Vec<ChatMessage>, reasoning: Vec<String>) {
        self.md = messages
            .iter()
            .map(|m| iced::widget::markdown::parse(&m.content).collect())
            .collect();
        self.reasoning = if reasoning.len() == messages.len() {
            reasoning
        } else {
            vec![String::new(); messages.len()]
        };
        self.messages = messages;
        self.reasoning_open.clear();
        self.streaming = false;
        self.error = None;
    }

    fn push_turn(&mut self, role: &str, content: String) {
        self.md.push(iced::widget::markdown::parse(&content).collect());
        self.reasoning.push(String::new());
        self.messages.push(ChatMessage::text(role, content));
    }

    /// Append a streamed delta to the assistant turn in flight, opening one if
    /// this is the first token. Markdown is re-parsed per delta, not per frame —
    /// the reply is short enough that a full reparse beats an incremental parser.
    fn push_delta(&mut self, text: &str) {
        if !self.streaming {
            self.push_turn("assistant", String::new());
            self.streaming = true;
        }
        let last = self.messages.len() - 1;
        self.messages[last].content.push_str(text);
        self.md[last] = iced::widget::markdown::parse(&self.messages[last].content).collect();
    }

    /// Append a reasoning delta — same turn-opening rule as `push_delta`, since
    /// a thinking model's reasoning arrives before its first content token.
    fn push_reasoning(&mut self, text: &str) {
        if !self.streaming {
            self.push_turn("assistant", String::new());
            self.streaming = true;
        }
        let last = self.messages.len() - 1;
        self.reasoning[last].push_str(text);
    }

    /// Is this the streaming turn whose answer hasn't started yet? The view
    /// keeps that one's thinking section open without a click.
    pub fn reasoning_live(&self, idx: usize) -> bool {
        self.streaming && idx + 1 == self.messages.len() && self.messages[idx].content.is_empty()
    }

    /// The wire history: scope context first, then the visible turns.
    fn wire_messages(&self) -> Vec<ChatMessage> {
        let system = self
            .system
            .iter()
            .map(|c| ChatMessage::text("system", c.clone()));
        system.chain(self.messages.iter().cloned()).collect()
    }

    pub fn provider_ids(&self) -> Vec<String> {
        self.catalog.iter().map(|p| p.id.clone()).collect()
    }

    /// Models the chosen provider offers; every provider's models when no
    /// provider is picked (the proxy resolves an alias to its provider).
    pub fn model_options(&self) -> Vec<String> {
        self.catalog
            .iter()
            .filter(|p| self.provider.is_empty() || p.id == self.provider)
            .flat_map(|p| p.models.options.iter().cloned())
            .collect()
    }
}

/// Fetch the provider catalog for the dropdowns.
pub fn load_catalog(client: &Client) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        async move { client.llm_providers().await.map(|c| c.providers).map_err(|e| e.to_string()) },
        Message::CatalogLoaded,
    )
}

#[derive(Debug, Clone)]
pub enum Message {
    DraftChanged(String),
    ProviderChanged(String),
    ModelChanged(String),
    /// Back to the server's default provider and model.
    UseDefaults,
    CatalogLoaded(Result<Vec<agent_platform_client::types::ProviderEntry>, String>),
    Send,
    /// One chunk of the streamed reply.
    Chunk(ChatChunk),
    /// Show/hide the thinking section of message `idx`.
    ToggleReasoning(usize),
    LinkClicked(String),
    Clear,
    DismissError,
}

pub fn update(state: &mut State, client: &Client, message: Message) -> Task<Message> {
    match message {
        Message::DraftChanged(v) => {
            state.draft = v;
            Task::none()
        }
        Message::ProviderChanged(v) => {
            // The picked model belongs to the old provider; keep it only if
            // the new one also offers it.
            state.provider = v;
            if !state.model_options().iter().any(|m| m == &state.model) {
                state.model.clear();
            }
            Task::none()
        }
        Message::ModelChanged(v) => {
            state.model = v;
            Task::none()
        }
        Message::UseDefaults => {
            state.provider.clear();
            state.model.clear();
            Task::none()
        }
        Message::CatalogLoaded(Ok(providers)) => {
            state.catalog = providers;
            Task::none()
        }
        // The dropdowns just stay empty; chat itself still works on defaults.
        Message::CatalogLoaded(Err(_)) => Task::none(),
        Message::Send => {
            let prompt = state.draft.trim().to_string();
            if prompt.is_empty() || state.sending {
                return Task::none();
            }
            state.push_turn("user", prompt);
            state.draft.clear();
            state.sending = true;

            let body = ChatCompletionBody {
                messages: state.wire_messages(),
                model: non_empty(&state.model),
                provider: non_empty(&state.provider),
                temperature: None,
                max_tokens: None,
                tools: None,
                stream: Some(true),
            };
            Task::batch([
                iced::widget::operation::snap_to_end(state.scroll_id()),
                Task::run(sse::chat_stream(client.clone(), body), Message::Chunk),
            ])
        }
        Message::Chunk(ChatChunk::Delta(text)) => {
            state.push_delta(&text);
            iced::widget::operation::snap_to_end(state.scroll_id())
        }
        Message::Chunk(ChatChunk::Reasoning(text)) => {
            state.push_reasoning(&text);
            iced::widget::operation::snap_to_end(state.scroll_id())
        }
        Message::ToggleReasoning(idx) => {
            if !state.reasoning_open.remove(&idx) {
                state.reasoning_open.insert(idx);
            }
            Task::none()
        }
        // This screen sends no tools, so no tool calls can arrive.
        Message::Chunk(ChatChunk::ToolCall(_)) => Task::none(),
        Message::Chunk(ChatChunk::Done) => {
            state.sending = false;
            state.streaming = false;
            iced::widget::operation::snap_to_end(state.scroll_id())
        }
        Message::Chunk(ChatChunk::Failed(e)) => {
            state.sending = false;
            state.streaming = false;
            // The failed turn stays in the thread so the user can retry or edit
            // context; only the error banner is transient. Any text that arrived
            // before the failure stays too — a truncated reply beats a blank one.
            state.error = Some(e);
            Task::none()
        }
        Message::Clear => {
            state.messages.clear();
            state.md.clear();
            state.reasoning.clear();
            state.reasoning_open.clear();
            state.streaming = false;
            state.error = None;
            Task::none()
        }
        Message::LinkClicked(url) => {
            // Only real web links — a hallucinated file path via explorer would
            // be a surprise.
            if url.starts_with("http://") || url.starts_with("https://") {
                crate::shell::reveal_path(&url);
            }
            Task::none()
        }
        Message::DismissError => {
            state.error = None;
            Task::none()
        }
    }
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Client {
        Client::new("http://127.0.0.1:1", "k")
    }

    #[test]
    fn sending_appends_the_user_turn_and_clears_the_draft() {
        let mut s = State { draft: "  hello  ".into(), ..State::default() };
        let _ = update(&mut s, &client(), Message::Send);
        assert_eq!(s.messages.len(), 1);
        assert_eq!(s.messages[0].content, "hello");
        assert_eq!(s.messages[0].role, "user");
        assert!(s.draft.is_empty());
        assert!(s.sending);
    }

    #[test]
    fn blank_and_in_flight_sends_are_ignored() {
        let mut s = State { draft: "   ".into(), ..State::default() };
        let _ = update(&mut s, &client(), Message::Send);
        assert!(s.messages.is_empty());

        let mut s = State { draft: "hi".into(), sending: true, ..State::default() };
        let _ = update(&mut s, &client(), Message::Send);
        assert!(s.messages.is_empty());
    }

    #[test]
    fn scope_context_leads_the_wire_history_but_not_the_transcript() {
        let mut s = State { draft: "hi".into(), system: Some("run 7".into()), ..State::default() };
        let _ = update(&mut s, &client(), Message::Send);
        let wire = s.wire_messages();
        assert_eq!(wire.len(), 2);
        assert_eq!(wire[0].role, "system");
        assert_eq!(wire[0].content, "run 7");
        assert_eq!(wire[1].content, "hi");
        // The transcript the user reads holds only their own turn.
        assert_eq!(s.messages.len(), 1);
    }

    #[test]
    fn a_failed_turn_stays_in_the_thread() {
        let mut s = State { draft: "hi".into(), ..State::default() };
        let _ = update(&mut s, &client(), Message::Send);
        let _ = update(&mut s, &client(), Message::Chunk(ChatChunk::Failed("boom".into())));
        assert_eq!(s.messages.len(), 1);
        assert!(!s.sending);
        assert_eq!(s.error.as_deref(), Some("boom"));
    }

    #[test]
    fn deltas_accumulate_into_one_assistant_turn() {
        let mut s = State { draft: "hi".into(), ..State::default() };
        let _ = update(&mut s, &client(), Message::Send);
        for part in ["He", "llo", " there"] {
            let _ = update(&mut s, &client(), Message::Chunk(ChatChunk::Delta(part.into())));
        }
        let _ = update(&mut s, &client(), Message::Chunk(ChatChunk::Done));
        assert_eq!(s.messages.len(), 2, "one user turn, one assistant turn");
        assert_eq!(s.messages[1].role, "assistant");
        assert_eq!(s.messages[1].content, "Hello there");
        assert_eq!(s.md.len(), s.messages.len());
        assert!(!s.sending);
    }

    #[test]
    fn reasoning_collects_apart_from_the_reply_and_stays_off_the_wire() {
        let mut s = State { draft: "hi".into(), ..State::default() };
        let _ = update(&mut s, &client(), Message::Send);
        let _ = update(&mut s, &client(), Message::Chunk(ChatChunk::Reasoning("hmm, ".into())));
        // Reasoning alone opens the turn — the bubble exists while it thinks.
        assert_eq!(s.messages.len(), 2);
        assert!(s.reasoning_live(1));
        let _ = update(&mut s, &client(), Message::Chunk(ChatChunk::Reasoning("ok".into())));
        let _ = update(&mut s, &client(), Message::Chunk(ChatChunk::Delta("Answer".into())));
        let _ = update(&mut s, &client(), Message::Chunk(ChatChunk::Done));
        assert_eq!(s.reasoning[1], "hmm, ok");
        assert_eq!(s.messages[1].content, "Answer");
        assert!(!s.reasoning_live(1));
        // The wire history carries the answer only.
        assert!(s.wire_messages().iter().all(|m| !m.content.contains("hmm")));

        let _ = update(&mut s, &client(), Message::ToggleReasoning(1));
        assert!(s.reasoning_open.contains(&1));
        let _ = update(&mut s, &client(), Message::ToggleReasoning(1));
        assert!(!s.reasoning_open.contains(&1));
    }

    #[test]
    fn partial_text_survives_a_mid_stream_failure() {
        let mut s = State { draft: "hi".into(), ..State::default() };
        let _ = update(&mut s, &client(), Message::Send);
        let _ = update(&mut s, &client(), Message::Chunk(ChatChunk::Delta("half".into())));
        let _ = update(&mut s, &client(), Message::Chunk(ChatChunk::Failed("boom".into())));
        assert_eq!(s.messages[1].content, "half");
        assert_eq!(s.error.as_deref(), Some("boom"));
        // A retry must open a new bubble rather than append to the dead one.
        assert!(!s.streaming);
    }
}
