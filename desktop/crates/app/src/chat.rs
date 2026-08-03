//! Chat against the platform's LLM proxy. One thread, kept in memory — the
//! server's chat endpoint is stateless, so the whole history is sent each turn.

use agent_platform_client::types::{ChatCompletionBody, ChatMessage};
use agent_platform_client::Client;
use iced::Task;

#[derive(Default)]
pub struct State {
    pub messages: Vec<ChatMessage>,
    pub draft: String,
    pub model: String,
    pub sending: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    DraftChanged(String),
    ModelChanged(String),
    Send,
    Replied(Result<String, String>),
    Clear,
    DismissError,
}

pub fn update(state: &mut State, client: &Client, message: Message) -> Task<Message> {
    match message {
        Message::DraftChanged(v) => {
            state.draft = v;
            Task::none()
        }
        Message::ModelChanged(v) => {
            state.model = v;
            Task::none()
        }
        Message::Send => {
            let prompt = state.draft.trim().to_string();
            if prompt.is_empty() || state.sending {
                return Task::none();
            }
            state.messages.push(ChatMessage { role: "user".into(), content: prompt });
            state.draft.clear();
            state.sending = true;

            let body = ChatCompletionBody {
                messages: state.messages.clone(),
                model: non_empty(&state.model),
                temperature: None,
                max_tokens: None,
            };
            let client = client.clone();
            Task::perform(
                async move { client.chat(&body).await.map_err(|e| e.to_string()) },
                Message::Replied,
            )
        }
        Message::Replied(Ok(reply)) => {
            state.sending = false;
            state.messages.push(ChatMessage { role: "assistant".into(), content: reply });
            Task::none()
        }
        Message::Replied(Err(e)) => {
            state.sending = false;
            // The failed turn stays in the thread so the user can retry or edit
            // context; only the error banner is transient.
            state.error = Some(e);
            Task::none()
        }
        Message::Clear => {
            state.messages.clear();
            state.error = None;
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
    fn a_failed_turn_stays_in_the_thread() {
        let mut s = State { draft: "hi".into(), ..State::default() };
        let _ = update(&mut s, &client(), Message::Send);
        let _ = update(&mut s, &client(), Message::Replied(Err("boom".into())));
        assert_eq!(s.messages.len(), 1);
        assert!(!s.sending);
        assert_eq!(s.error.as_deref(), Some("boom"));
    }
}
