//! LLM provider settings: BYOK keys, local endpoints and the default
//! provider/model pair, all backed by the proxy's `.env`.
//!
//! Secrets are write-only. The server returns `set` + a masked tail, never the
//! value, so an untouched key field means "leave what is stored alone" — an
//! empty string would otherwise read as "clear it".
//!
//! **Web search (ADR 0008's amendment) is deliberately not a [`PROVIDER_META`]
//! row.** That table is shaped around chat models — a models dropdown, a base
//! URL, a launch command — and `SEARCH_API_KEY`/`SEARCH_CX` are two opaque
//! strings with none of that; forcing them in means either widening the row
//! for a shape only they use, or a row that renders mostly empty. It renders
//! instead as its own small card (`providers_view.rs::search_card`), the same
//! way the server's `llm_admin.rs` already treats it as a manual addition to
//! `ENV_KEYS`/`SENSITIVE_ENV_KEYS` rather than a `ProviderSpec` row. See
//! `search_credentials_reach_the_save_body` below for this file's half of the
//! "not walked by a table, so pin it directly" pattern the server's tests use.

use crate::domain::err_string;
use agent_platform_client::types::*;
use agent_platform_client::Client;
use iced::Task;

/// What the catalog does not carry: which `.env` fields belong to a provider,
/// where its key is minted, and how to start it when it runs on this machine.
///
/// The master key is absent on purpose — the shell owns it, and rewriting it
/// would orphan this client.
pub struct ProviderMeta {
    pub id: &'static str,
    /// Env key and label of the write-only API key, when the provider has one.
    pub secret: Option<(&'static str, &'static str)>,
    /// Env key, label and placeholder of the base URL, when it has one.
    pub endpoint: Option<(&'static str, &'static str, &'static str)>,
    /// Where to mint a key. Offered whenever the provider is unconfigured.
    pub key_url: Option<&'static str>,
    /// Command that starts the backend locally, for the "Launch" action.
    pub launch: Option<(&'static str, &'static [&'static str])>,
    /// Whether models can be pulled into this provider (Ollama only).
    pub pullable: bool,
}

pub const PROVIDER_META: [ProviderMeta; 5] = [
    ProviderMeta {
        id: "ollama",
        secret: None,
        endpoint: Some(("OLLAMA_API_BASE", "Base URL", "http://127.0.0.1:11434")),
        key_url: None,
        launch: Some(("ollama", &["serve"])),
        pullable: true,
    },
    ProviderMeta {
        id: "lm_studio",
        secret: Some(("LM_STUDIO_API_KEY", "API key")),
        endpoint: Some(("LM_STUDIO_API_BASE", "Base URL", "http://127.0.0.1:1234/v1")),
        key_url: None,
        // LM Studio's CLI, installed beside the app; `lms` is on PATH once the
        // user has run its bootstrap.
        launch: Some(("lms", &["server", "start"])),
        pullable: false,
    },
    ProviderMeta {
        id: "aimlapi",
        secret: Some(("AIMLAPI_API_KEY", "API key")),
        endpoint: Some(("AIMLAPI_OPENAI_BASE", "Base URL", "https://api.aimlapi.com/v1")),
        key_url: Some("https://aimlapi.com/app/keys"),
        launch: None,
        pullable: false,
    },
    ProviderMeta {
        id: "anthropic",
        secret: Some(("ANTHROPIC_API_KEY", "API key")),
        endpoint: None,
        key_url: Some("https://console.anthropic.com/settings/keys"),
        launch: None,
        pullable: false,
    },
    ProviderMeta {
        id: "gemini",
        secret: Some(("GEMINI_API_KEY", "API key")),
        endpoint: None,
        key_url: Some("https://aistudio.google.com/apikey"),
        launch: None,
        pullable: false,
    },
];

pub fn meta(id: &str) -> Option<&'static ProviderMeta> {
    PROVIDER_META.iter().find(|m| m.id == id)
}

#[derive(Default)]
pub struct State {
    pub env: Option<LlmEnv>,
    pub catalog: Vec<ProviderEntry>,
    /// False until the first catalog fetch settles, so an in-flight load is not
    /// rendered as "no providers exist".
    pub catalog_loaded: bool,
    /// Edits not yet saved, keyed by env name. Absent = untouched.
    pub drafts: Vec<(String, String)>,
    pub default_provider: String,
    pub default_model: String,
    /// Provider id whose settings modal is open.
    pub open: Option<String>,
    /// Draft name for an Ollama pull, in the open modal.
    pub pull_name: String,
    /// Models Ollama has on this machine, with their sizes — the Model ops
    /// card, shown where a model is actually chosen. Empty when Ollama is down;
    /// that is not an error banner, the catalog row already says it is stopped.
    pub local_models: Vec<OllamaModelSummary>,
    pub busy: bool,
    pub error: Option<String>,
    pub notice: crate::domain::Toast,
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
        self.models_of(&self.default_provider)
    }

    pub fn models_of(&self, provider: &str) -> Vec<String> {
        self.catalog
            .iter()
            .find(|p| p.id == provider)
            .map(|p| p.models.options.clone())
            .unwrap_or_default()
    }

    pub fn entry(&self, provider: &str) -> Option<&ProviderEntry> {
        self.catalog.iter().find(|p| p.id == provider)
    }

    /// Whether the *saved* `.env` (not an unsaved draft — same rule the
    /// catalog badges above already follow) has this env key set.
    fn env_set(&self, key: &str) -> bool {
        self.env_key(key).is_some_and(|k| k.set)
    }

    /// `SearchBackend::from_env`'s "both or neither" rule, mirrored client-side
    /// so the badge agrees with what `/api/v1/search` will actually do.
    pub fn search_configured(&self) -> bool {
        self.env_set("SEARCH_API_KEY") && self.env_set("SEARCH_CX")
    }

    /// `None` when search is fully configured or fully unset — there is
    /// nothing more to say in either case. `Some` names the one field still
    /// missing, for the half-configured state `SearchBackend::from_env`
    /// treats as unconfigured (a key with no cx, or a cx with no key).
    pub fn search_missing(&self) -> Option<&'static str> {
        match (self.env_set("SEARCH_API_KEY"), self.env_set("SEARCH_CX")) {
            (true, false) => Some("the search engine ID"),
            (false, true) => Some("the API key"),
            _ => None,
        }
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
                "ANTHROPIC_API_KEY" => body.anthropic_api_key = v,
                "SEARCH_API_KEY" => body.search_api_key = v,
                "SEARCH_CX" => body.search_cx = v,
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
    /// "View logs" on a traced error banner — intercepted in `main::update`
    /// before it reaches here, so this arm exists only to satisfy exhaustiveness.
    TraceLogs(String),
    Refresh,
    EnvLoaded(Result<Box<LlmEnv>, String>),
    CatalogLoaded(Result<Vec<ProviderEntry>, String>),
    LocalModelsLoaded(Result<Vec<OllamaModelSummary>, String>),
    FieldChanged(&'static str, String),
    DefaultProviderChanged(String),
    DefaultModelChanged(String),
    /// A model picked inside a provider's modal: it becomes the default model
    /// *and* makes that provider the default one.
    ProviderModelPicked(String, String),
    /// Open / close the per-provider settings modal.
    Open(String),
    Close,
    /// Start a local backend with the command in its [`ProviderMeta`].
    Launch(&'static str),
    /// Open a provider's key page in the browser.
    OpenUrl(&'static str),
    PullNameChanged(String),
    PullModel,
    Pulled(Result<i64, String>),
    Save,
    Saved(Result<String, String>),
    Dismiss,
}

pub fn refresh(client: &Client) -> Task<Message> {
    let (c1, c2, c3) = (client.clone(), client.clone(), client.clone());
    Task::batch([
        Task::perform(
            async move { err_string(c1.llm_env().await).map(Box::new) },
            Message::EnvLoaded,
        ),
        Task::perform(
            async move { err_string(c2.llm_providers().await).map(|c| c.providers) },
            Message::CatalogLoaded,
        ),
        Task::perform(
            async move { err_string(c3.ollama_models().await).map(|r| r.models) },
            Message::LocalModelsLoaded,
        ),
    ])
}

pub fn update(state: &mut State, client: &Client, message: Message) -> Task<Message> {
    match message {
        Message::TraceLogs(_) => Task::none(),
        Message::Refresh => refresh(client),
        Message::EnvLoaded(Ok(env)) => {
            state.error = None;
            // Reloading is also the post-save path, so drafts are dropped: what
            // the server now reports is the truth.
            state.drafts.clear();
            state.default_provider = env.persisted_defaults.provider.clone();
            state.default_model = env.persisted_defaults.model.clone();
            state.env = Some(*env);
            Task::none()
        }
        Message::CatalogLoaded(Ok(providers)) => {
            state.error = None;
            state.catalog = providers;
            state.catalog_loaded = true;
            Task::none()
        }
        // Ollama being down is the normal case here, not a failure worth a
        // banner: the catalog row already renders it as "stopped".
        Message::LocalModelsLoaded(result) => {
            state.local_models = result.unwrap_or_default();
            Task::none()
        }
        Message::CatalogLoaded(Err(e)) => {
            state.catalog_loaded = true;
            state.error = Some(e);
            Task::none()
        }
        Message::EnvLoaded(Err(e)) => {
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
        Message::ProviderModelPicked(provider, model) => {
            state.default_provider = provider;
            state.default_model = model;
            Task::none()
        }

        Message::Open(provider) => {
            state.pull_name.clear();
            state.open = Some(provider);
            Task::none()
        }
        Message::Close => {
            state.open = None;
            Task::none()
        }
        Message::Launch(provider) => {
            let Some((program, args)) = meta(provider).and_then(|m| m.launch) else {
                return Task::none();
            };
            match crate::shell::spawn_detached(program, args) {
                // The backend needs a moment to bind before it answers a probe,
                // so the catalog is not refreshed here — the user hits Refresh.
                Ok(()) => state.notice.set(format!("Started `{program}`. Refresh once it is up.")),
                Err(e) => state.error = Some(format!("Could not start `{program}`: {e}")),
            }
            Task::none()
        }
        Message::OpenUrl(url) => {
            crate::shell::open_url(url);
            Task::none()
        }
        Message::PullNameChanged(name) => {
            state.pull_name = name;
            Task::none()
        }
        Message::PullModel => {
            let name = state.pull_name.trim().to_string();
            if name.is_empty() {
                state.error = Some("Enter a model name to pull.".into());
                return Task::none();
            }
            state.busy = true;
            let client = client.clone();
            Task::perform(
                async move { err_string(client.pull_ollama_model(&name).await).map(|j| j.id) },
                Message::Pulled,
            )
        }
        Message::Pulled(result) => {
            state.busy = false;
            match result {
                Ok(_) => {
                    state.error = None;
                    state.pull_name.clear();
                    // The job runs on the server; this screen has no poll, so
                    // say so rather than implying the list updates itself.
                    state.notice.set("Pull started in the background. Refresh to see it land.");
                    Task::none()
                }
                Err(e) => {
                    state.error = Some(e);
                    Task::none()
                }
            }
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
                    state.error = None;
                    state.notice.set(message);
                    refresh(client)
                }
                Err(e) => {
                    state.error = Some(e);
                    Task::none()
                }
            }
        }
        Message::Dismiss => {
            state.notice.clear();
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

    /// Every field a modal can render has to reach `EnvUpdate`, or the dialog
    /// silently drops what was typed into it.
    #[test]
    fn every_provider_secret_and_endpoint_is_a_field_the_save_body_carries() {
        for m in &PROVIDER_META {
            for key in m.secret.map(|(k, _)| k).into_iter().chain(m.endpoint.map(|(k, _, _)| k)) {
                let mut s = State::default();
                let _ = update(&mut s, &client(), Message::FieldChanged(key, "v".into()));
                let body = s.pending();
                let json = serde_json::to_value(&body).unwrap();
                assert_eq!(json.get(key).and_then(|v| v.as_str()), Some("v"), "{key} is dropped");
            }
        }
    }

    /// `SEARCH_API_KEY`/`SEARCH_CX` are the non-chat-credential case
    /// `every_provider_secret_and_endpoint_is_a_field_the_save_body_carries`'s
    /// doc comment calls out: they have no `PROVIDER_META` row (search is not
    /// an LLM provider — see this module's doc comment on the placement
    /// call), so nothing walks a table to catch a dropped field here. This
    /// pins both directly, the same way the server's
    /// `search_credentials_are_present_and_masked_correctly` pins its side.
    #[test]
    fn search_credentials_reach_the_save_body() {
        for key in ["SEARCH_API_KEY", "SEARCH_CX"] {
            let mut s = State::default();
            let _ = update(&mut s, &client(), Message::FieldChanged(key, "v".into()));
            let body = s.pending();
            let json = serde_json::to_value(&body).unwrap();
            assert_eq!(json.get(key).and_then(|v| v.as_str()), Some("v"), "{key} is dropped");
        }
    }

    /// Both required together — a key with no cx (or vice versa) must read as
    /// unconfigured and name the missing one, never as silently half-working.
    #[test]
    fn search_is_configured_only_when_both_fields_are_set() {
        let env = |key_set: bool, cx_set: bool| {
            let mut keys = serde_json::Map::new();
            if key_set {
                keys.insert("SEARCH_API_KEY".into(), json!({"set": true, "masked": "****abcd"}));
            }
            if cx_set {
                keys.insert("SEARCH_CX".into(), json!({"set": true, "value": "012345:abc"}));
            }
            let env: LlmEnv = serde_json::from_value(json!({
                "keys": serde_json::Value::Object(keys),
                "persisted_defaults": {"provider": "", "model": ""},
                "resolved_defaults": {"provider": "", "model": ""},
            }))
            .unwrap();
            let mut s = State::default();
            let _ = update(&mut s, &client(), Message::EnvLoaded(Ok(Box::new(env))));
            s
        };

        assert!(!env(false, false).search_configured());
        assert!(env(false, false).search_missing().is_none(), "nothing to name when both are unset");

        assert!(!env(true, false).search_configured());
        assert_eq!(env(true, false).search_missing(), Some("the search engine ID"));

        assert!(!env(false, true).search_configured());
        assert_eq!(env(false, true).search_missing(), Some("the API key"));

        assert!(env(true, true).search_configured());
        assert!(env(true, true).search_missing().is_none());
    }

    /// Ollama being down is the normal case on this screen — the catalog row
    /// already says "stopped", so the model list must not also raise a banner.
    #[test]
    fn a_missing_ollama_does_not_raise_an_error_banner() {
        let mut s = loaded();
        let _ =
            update(&mut s, &client(), Message::LocalModelsLoaded(Err("connection refused".into())));
        assert!(s.error.is_none());
        assert!(s.local_models.is_empty());
    }

    #[test]
    fn picking_a_model_in_a_modal_also_claims_the_default_provider() {
        let mut s = loaded();
        let _ = update(
            &mut s,
            &client(),
            Message::ProviderModelPicked("gemini".into(), "gemini-2.0-flash".into()),
        );
        let body = s.pending();
        assert_eq!(body.default_provider.as_deref(), Some("gemini"));
        assert_eq!(body.default_model.as_deref(), Some("gemini-2.0-flash"));
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
