//! LLM provider settings: BYOK keys, local endpoints and the default
//! provider/model pair, all backed by the proxy's `.env`.
//!
//! Secrets are write-only. The server returns `set` + a masked tail, never the
//! value, so an untouched key field means "leave what is stored alone" — an
//! empty string would otherwise read as "clear it".

use agent_platform_client::types::*;
use agent_platform_client::Client;
use iced::Task;

/// `.env` keys this screen edits, in render order. The master key is absent on
/// purpose: the shell owns it, and rewriting it would orphan this client.
pub const SECRET_FIELDS: [(&str, &str); 3] = [
    ("GEMINI_API_KEY", "Gemini API key"),
    ("AIMLAPI_API_KEY", "AI/ML API key"),
    ("LM_STUDIO_API_KEY", "LM Studio API key"),
];

pub const ENDPOINT_FIELDS: [(&str, &str, &str); 3] = [
    ("OLLAMA_API_BASE", "Ollama base URL", "http://127.0.0.1:11434"),
    ("LM_STUDIO_API_BASE", "LM Studio base URL", "http://127.0.0.1:1234/v1"),
    ("AIMLAPI_OPENAI_BASE", "AI/ML API base URL", "https://api.aimlapi.com/v1"),
];

#[derive(Default)]
pub struct State {
    pub env: Option<LlmEnv>,
    pub catalog: Vec<ProviderEntry>,
    /// Edits not yet saved, keyed by env name. Absent = untouched.
    pub drafts: Vec<(String, String)>,
    pub default_provider: String,
    pub default_model: String,
    pub busy: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
}

impl State {
    pub fn draft(&self, key: &str) -> &str {
        self.drafts
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .unwrap_or_default()
    }

    pub fn edited(&self, key: &str) -> bool {
        self.drafts.iter().any(|(k, _)| k == key)
    }

    pub fn env_key(&self, key: &str) -> Option<&EnvKey> {
        self.env.as_ref().and_then(|e| e.keys.get(key))
    }

    pub fn provider_ids(&self) -> Vec<String> {
        self.catalog.iter().map(|p| p.id.clone()).collect()
    }

    /// Models offered for the currently chosen default provider.
    pub fn model_options(&self) -> Vec<String> {
        self.catalog
            .iter()
            .find(|p| p.id == self.default_provider)
            .map(|p| p.models.options.clone())
            .unwrap_or_default()
    }

    fn set_draft(&mut self, key: &str, value: String) {
        match self.drafts.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => self.drafts.push((key.to_string(), value)),
        }
    }

    /// Only fields the user actually touched, plus the defaults pair when it
    /// differs from what is persisted.
    fn pending(&self) -> EnvUpdate {
        let mut body = EnvUpdate::default();
        for (key, value) in &self.drafts {
            let v = Some(value.trim().to_string());
            match key.as_str() {
                "GEMINI_API_KEY" => body.gemini_api_key = v,
                "AIMLAPI_API_KEY" => body.aimlapi_api_key = v,
                "LM_STUDIO_API_KEY" => body.lm_studio_api_key = v,
                "OLLAMA_API_BASE" => body.ollama_api_base = v,
                "LM_STUDIO_API_BASE" => body.lm_studio_api_base = v,
                "AIMLAPI_OPENAI_BASE" => body.aimlapi_openai_base = v,
                _ => {}
            }
        }
        let persisted = self.env.as_ref().map(|e| &e.persisted_defaults);
        if persisted.is_none_or(|p| p.provider != self.default_provider) {
            body.default_provider = Some(self.default_provider.clone());
        }
        if persisted.is_none_or(|p| p.model != self.default_model) {
            body.default_model = Some(self.default_model.clone());
        }
        body
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Refresh,
    EnvLoaded(Result<Box<LlmEnv>, String>),
    CatalogLoaded(Result<Vec<ProviderEntry>, String>),
    FieldChanged(&'static str, String),
    DefaultProviderChanged(String),
    DefaultModelChanged(String),
    Save,
    Saved(Result<String, String>),
    Dismiss,
}

fn err_string<T>(r: agent_platform_client::Result<T>) -> Result<T, String> {
    r.map_err(|e| e.to_string())
}

pub fn refresh(client: &Client) -> Task<Message> {
    let (c1, c2) = (client.clone(), client.clone());
    Task::batch([
        Task::perform(
            async move { err_string(c1.llm_env().await).map(Box::new) },
            Message::EnvLoaded,
        ),
        Task::perform(
            async move { err_string(c2.llm_providers().await).map(|c| c.providers) },
            Message::CatalogLoaded,
        ),
    ])
}

pub fn update(state: &mut State, client: &Client, message: Message) -> Task<Message> {
    match message {
        Message::Refresh => refresh(client),
        Message::EnvLoaded(Ok(env)) => {
            // Reloading is also the post-save path, so drafts are dropped: what
            // the server now reports is the truth.
            state.drafts.clear();
            state.default_provider = env.persisted_defaults.provider.clone();
            state.default_model = env.persisted_defaults.model.clone();
            state.env = Some(*env);
            Task::none()
        }
        Message::CatalogLoaded(Ok(providers)) => {
            state.catalog = providers;
            Task::none()
        }
        Message::EnvLoaded(Err(e)) | Message::CatalogLoaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }

        Message::FieldChanged(key, value) => {
            state.set_draft(key, value);
            Task::none()
        }
        Message::DefaultProviderChanged(provider) => {
            // The stored model belongs to the old provider; keep it only if the
            // new provider also offers it.
            state.default_provider = provider;
            if !state.model_options().iter().any(|m| m == &state.default_model) {
                state.default_model.clear();
            }
            Task::none()
        }
        Message::DefaultModelChanged(model) => {
            state.default_model = model;
            Task::none()
        }

        Message::Save => {
            let body = state.pending();
            state.busy = true;
            let client = client.clone();
            Task::perform(
                async move { err_string(client.save_llm_env(&body).await).map(|r| r.message) },
                Message::Saved,
            )
        }
        Message::Saved(result) => {
            state.busy = false;
            match result {
                Ok(message) => {
                    state.notice = Some(message);
                    refresh(client)
                }
                Err(e) => {
                    state.error = Some(e);
                    Task::none()
                }
            }
        }
        Message::Dismiss => {
            state.notice = None;
            state.error = None;
            Task::none()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn client() -> Client {
        Client::new("http://127.0.0.1:1", "k")
    }

    fn loaded() -> State {
        let mut s = State::default();
        let env: LlmEnv = serde_json::from_value(json!({
            "keys": {
                "GEMINI_API_KEY": {"set": true, "masked": "****abcd"},
                "OLLAMA_API_BASE": {"set": true, "value": "http://127.0.0.1:11434"},
            },
            "persisted_defaults": {"provider": "ollama", "model": "qwen2.5:7b"},
            "resolved_defaults": {"provider": "ollama", "model": "qwen2.5:7b"},
        }))
        .unwrap();
        // `capabilities` and `default_model` are real response fields this
        // screen does not read; keeping them proves they stay ignorable.
        let providers: Vec<ProviderEntry> = serde_json::from_value(json!([
            {"id": "ollama", "label": "Ollama", "configured": true, "local": true,
             "capabilities": {"streaming": true, "tools": true},
             "models": {"options": ["qwen2.5:7b"], "selected_model": "qwen2.5:7b",
                        "default_model": "qwen2.5:7b", "source": "ollama_tags",
                        "warning": null, "fallback_note": null}},
            {"id": "gemini", "label": "Gemini", "configured": true, "local": false,
             "capabilities": {"streaming": true, "tools": false},
             "models": {"options": ["gemini-2.0-flash"], "selected_model": "gemini-2.0-flash",
                        "default_model": "gemini-2.0-flash", "source": "discovery",
                        "warning": null, "fallback_note": null}},
        ]))
        .unwrap();
        let _ = update(&mut s, &client(), Message::EnvLoaded(Ok(Box::new(env))));
        let _ = update(&mut s, &client(), Message::CatalogLoaded(Ok(providers)));
        s
    }

    #[test]
    fn untouched_secrets_are_not_sent() {
        let s = loaded();
        let body = s.pending();
        assert!(body.gemini_api_key.is_none());
        assert!(body.default_provider.is_none(), "defaults match what is persisted");
        assert!(body.default_model.is_none());
    }

    #[test]
    fn only_edited_fields_are_sent() {
        let mut s = loaded();
        let _ = update(&mut s, &client(), Message::FieldChanged("GEMINI_API_KEY", " new-key ".into()));
        let body = s.pending();
        assert_eq!(body.gemini_api_key.as_deref(), Some("new-key"));
        assert!(body.ollama_api_base.is_none());
    }

    #[test]
    fn switching_provider_drops_a_model_it_does_not_offer() {
        let mut s = loaded();
        let _ = update(&mut s, &client(), Message::DefaultProviderChanged("gemini".into()));
        assert!(s.default_model.is_empty());
        let body = s.pending();
        assert_eq!(body.default_provider.as_deref(), Some("gemini"));
        assert_eq!(body.default_model.as_deref(), Some(""));
    }

    #[test]
    fn reload_after_save_discards_drafts() {
        let mut s = loaded();
        let _ = update(&mut s, &client(), Message::FieldChanged("GEMINI_API_KEY", "x".into()));
        assert!(s.edited("GEMINI_API_KEY"));
        let env: LlmEnv = serde_json::from_value(json!({
            "keys": {}, "persisted_defaults": {"provider": "", "model": ""},
            "resolved_defaults": {"provider": "", "model": ""},
        }))
        .unwrap();
        let _ = update(&mut s, &client(), Message::EnvLoaded(Ok(Box::new(env))));
        assert!(!s.edited("GEMINI_API_KEY"));
    }
}
