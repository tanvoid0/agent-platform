//! The LLM proxy's admin surface — `app/llm_proxy/admin_routes.py`, mounted at
//! `/api/v1/llm-proxy`.
//!
//! This is the operator's side of the proxy `llm.rs` serves: the `.env` and
//! `config.yaml` those modules read on every request, the config screen's
//! provider catalog, and the four "does it actually work" probes. It closes the
//! live coupling `plan.md` flagged — Python owned the writes to the two files
//! Rust reads.
//!
//! `POST /config-yaml` was the last route here left with Python, for its
//! `jsonschema` error text; [`crate::config_schema`] reproduces that wording so
//! it lands here too, and the divergence that remains is documented there.
//!
//! One thing is worth restating, because it looks like an oversight:
//!
//! - **`ORCHESTRATOR_INTERNAL_URL`'s default.** The self-calls below go to
//!   `http://127.0.0.1:18410` unless that variable says otherwise, exactly as
//!   Python's module constant does — *not* to whichever port this process
//!   happens to be bound to. Both servers therefore probe the same place, which
//!   is what makes the two comparable when the harness runs them on a spare
//!   port.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
use serde_json::{json, Map, Value};

use crate::auth::Principal;
use crate::error::{ApiError, PathId};
use crate::llm::yaml_str;
use crate::llm_config::{
    config_yaml_path, env_file_path, is_supported_provider, load_config_yaml, provider_configured,
    read_env_file, DEFAULT_LM_STUDIO_BASE, DEFAULT_OLLAMA_BASE,
};
use crate::provider_catalog::{admin_entry, build_admin, persisted_defaults, resolved_defaults};
use crate::upstream_http::{classify_with_context, open_stream, send_with_retry, sse_error_chunk};
use crate::wire::{
    check_len, defaulted_str, lax_bool, lax_int, optional_str, parse_body, required_str,
};
use crate::{env_opt, AppState};

const MASTER_KEY_ENV: &str = "AGENT_PLATFORM_MASTER_KEY";

/// Written back in this order, every key present even when empty — the file is
/// regenerated wholesale rather than patched.
///
/// `SEARCH_API_KEY`/`SEARCH_CX` (ADR 0008's amendment, "results, behind a
/// key") are here even though search has no `ProviderSpec` row: this is the
/// set the `.env` editor (`GET`/`POST /env` below) will actually write, and
/// there is no other list a search key could ride in on.
const ENV_KEYS: [&str; 12] = [
    MASTER_KEY_ENV,
    "GEMINI_API_KEY",
    "AIMLAPI_API_KEY",
    "AIMLAPI_OPENAI_BASE",
    "ANTHROPIC_API_KEY",
    "OLLAMA_API_BASE",
    "LM_STUDIO_API_BASE",
    "LM_STUDIO_API_KEY",
    "DEFAULT_PROVIDER",
    "DEFAULT_MODEL",
    "SEARCH_API_KEY",
    "SEARCH_CX",
];

/// Masked in `GET /env`; every other key returns its plaintext `value`. Also
/// what `dotenv` refuses to take from the committed YAML — hiding a key here
/// and accepting it from a checked-in file would cancel out.
///
/// Every `ProviderSpec::api_key_env` belongs here, whatever registry it is in;
/// `the_provider_table_is_the_source_of_the_key_lists` is the test that says
/// so for the provider table. `SEARCH_API_KEY` is not in that table at all —
/// search is not an LLM provider — so it is a manual addition, checked by
/// `search_credentials_are_present_and_masked_correctly` below instead.
/// `SEARCH_CX` is deliberately **not** here: it names a Programmable Search
/// *engine*, not an account, so sharing it is not a credential leak the way
/// the key is — it still needs `ENV_KEYS` above so the editor can write it,
/// just not masking here or refusal from committed YAML.
pub(crate) const SENSITIVE_ENV_KEYS: [&str; 7] = [
    MASTER_KEY_ENV,
    "GEMINI_API_KEY",
    "AIMLAPI_API_KEY",
    "LM_STUDIO_API_KEY",
    "ANTHROPIC_API_KEY",
    "SPEECH_API_KEY",
    "SEARCH_API_KEY",
];

pub fn routes() -> Router<Arc<AppState>> {
    const BASE: &str = "/api/v1/llm-proxy";
    Router::new()
        .route(&format!("{BASE}/snippet"), get(snippet))
        .route(&format!("{BASE}/env"), get(get_env).post(post_env))
        .route(&format!("{BASE}/config-yaml"), get(get_config_yaml).post(post_config_yaml))
        .route(&format!("{BASE}/health-proxy"), get(health_proxy))
        .route(&format!("{BASE}/health-readiness"), get(health_readiness))
        .route(&format!("{BASE}/ui/providers"), get(ui_providers))
        .route(&format!("{BASE}/ui/env-model-options"), get(ui_env_model_options))
        .route(&format!("{BASE}/ui/providers/{{provider_or_alias}}/models"), get(ui_provider_models))
        .route(&format!("{BASE}/proxy/models"), get(proxy_models))
        .route(&format!("{BASE}/test/model-options"), get(test_model_options))
        .route(&format!("{BASE}/test-chat"), post(test_chat))
        .route(&format!("{BASE}/test-chat-stream"), post(test_chat_stream))
        .route(&format!("{BASE}/test-embeddings"), post(test_embeddings))
}

/// `require_master_key`. Reading or writing the server's `.env` / `config.yaml`,
/// or printing the master key, is an operator action; see
/// [`Principal::require_master_key`].
const NOT_A_TENANT: &str = "This endpoint requires the platform master key, not a workspace token.";

// ---------------------------------------------------------------------------
// The dotenv file
// ---------------------------------------------------------------------------

/// `CONFIG_DIR/.env` wins (live-editable, no restart); the process environment
/// is the fallback.
///
/// Without the fallback every self-call below sends `Authorization: Bearer `,
/// which is an illegal header value, whenever the master key comes from the
/// process environment instead of having been saved through the config UI.
fn master_key_from_env(env: &HashMap<String, String>) -> String {
    let from_file = env.get(MASTER_KEY_ENV).map(|v| v.trim()).unwrap_or_default();
    if from_file.is_empty() {
        env_opt(MASTER_KEY_ENV).unwrap_or_default()
    } else {
        from_file.to_string()
    }
}

fn mask_secret(value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else if value.chars().count() <= 4 {
        "****".to_string()
    } else {
        format!("****{}", &value[value.len() - 4..])
    }
}

fn write_env_file(values: &HashMap<String, String>) -> Result<(), ApiError> {
    let mut lines = vec![
        "# Generated / updated by Agent Platform (LLM proxy settings). Do not commit to git."
            .to_string(),
        String::new(),
    ];
    for key in ENV_KEYS {
        let value = values.get(key).cloned().unwrap_or_default();
        if value.contains([' ', '\n', '\t', '"', '\'', '\\']) {
            let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
            lines.push(format!("{key}=\"{escaped}\""));
        } else {
            lines.push(format!("{key}={value}"));
        }
    }
    lines.push(String::new());

    let path = env_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_error)?;
    }
    // `Path.write_text` translates `\n` to `os.linesep`, so the file Python
    // leaves on Windows is CRLF. Nothing parses this byte-for-byte, but the two
    // servers writing different bytes to the same file is exactly the kind of
    // difference this port exists to avoid.
    std::fs::write(&path, lines.join(NEWLINE)).map_err(io_error)
}

const NEWLINE: &str = if cfg!(windows) { "\r\n" } else { "\n" };

/// `Path.read_text` in its default (universal-newline) mode, which is how
/// Python reads `config.yaml` back out for the editor.
fn read_text_universal(raw: &str) -> String {
    if raw.contains('\r') {
        raw.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        raw.to_string()
    }
}

fn io_error(e: std::io::Error) -> ApiError {
    logd!("llm-proxy config write failed: {e}");
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "An unexpected error occurred.")
}

// ---------------------------------------------------------------------------
// Self-calls
// ---------------------------------------------------------------------------

/// The `/v1` surface this process also serves, reached over HTTP so the request
/// goes through auth and the proxy's own routing rather than around them.
fn internal_url() -> String {
    env_opt("ORCHESTRATOR_INTERNAL_URL")
        .unwrap_or_else(|| "http://127.0.0.1:18410".into())
        .trim_end_matches('/')
        .to_string()
}

fn public_base() -> String {
    env_opt("PROXY_PUBLIC_URL")
        .unwrap_or_else(|| "http://127.0.0.1:18410".into())
        .trim_end_matches('/')
        .to_string()
}

/// `{"status_code": …, "body": …}` — the raw upstream answer, truncated to
/// `limit` **characters**, which is what Python's `text[:n]` slices.
fn probe_body(response: &crate::upstream_http::UpstreamResponse, limit: usize) -> Value {
    let text = response.text();
    let truncated: String = text.chars().take(limit).collect();
    json!({ "status_code": response.status.as_u16(), "body": truncated })
}

// ---------------------------------------------------------------------------
// Config routes
// ---------------------------------------------------------------------------

async fn snippet(principal: Principal) -> Result<Response, ApiError> {
    principal.require_master_key(NOT_A_TENANT)?;
    let master = master_key_from_env(&read_env_file());
    let base = public_base();
    let snippet = format!(
        "export OPENAI_BASE_URL={base}/v1\nexport OPENAI_API_KEY=\"{master}\"\n\
         # model: config.yaml aliases; raw Ollama tags or LM Studio ids (defaults.provider)"
    );
    Ok(Json(json!({ "public_base": base, "snippet": snippet })).into_response())
}

async fn get_env(principal: Principal) -> Result<Response, ApiError> {
    principal.require_master_key(NOT_A_TENANT)?;
    let env = read_env_file();
    let mut keys = Map::new();
    for key in ENV_KEYS {
        let value = env.get(key).cloned().unwrap_or_default();
        let set = !value.is_empty();
        let entry = if SENSITIVE_ENV_KEYS.contains(&key) {
            json!({ "set": set, "masked": if set { mask_secret(&value) } else { String::new() } })
        } else {
            json!({ "set": set, "value": value })
        };
        keys.insert(key.to_string(), entry);
    }
    Ok(Json(json!({
        "keys": keys,
        "effective_defaults": {
            "OLLAMA_API_BASE": DEFAULT_OLLAMA_BASE,
            "LM_STUDIO_API_BASE": DEFAULT_LM_STUDIO_BASE,
            "AIMLAPI_OPENAI_BASE": "https://api.aimlapi.com/v1",
        },
        "persisted_defaults": persisted_defaults(),
        "resolved_defaults": resolved_defaults(),
    }))
    .into_response())
}

async fn post_env(principal: Principal, body: Bytes) -> Result<Response, ApiError> {
    principal.require_master_key(NOT_A_TENANT)?;
    let body = parse_body(&body)?;

    // `EnvUpdate` is nine optional strings and pydantic ignores the rest, so a
    // key that is present must still be a string (or null) to be accepted.
    let mut errors = Vec::new();
    let mut updates: HashMap<&str, String> = HashMap::new();
    for key in ENV_KEYS {
        match body.get(key) {
            None | Some(Value::Null) => {}
            Some(Value::String(s)) => {
                updates.insert(key, s.trim().to_string());
            }
            Some(_) => errors.push(ApiError::field_error(
                key,
                "string_type",
                "Input should be a valid string",
            )),
        }
    }
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    let mut merged: HashMap<String, String> = (*read_env_file()).clone();
    for (key, value) in updates {
        // A blank secret is "leave it alone", not "clear it": the config screen
        // renders these masked and posts the mask back untouched.
        if SENSITIVE_ENV_KEYS.contains(&key) && value.is_empty() {
            continue;
        }
        merged.insert(key.to_string(), value);
    }
    write_env_file(&merged)?;

    Ok(Json(json!({
        "ok": true,
        "message": "Saved .env. Restart the Agent Platform process (or container) to apply \
                    env-based auth and providers.",
    }))
    .into_response())
}

async fn get_config_yaml(principal: Principal) -> Result<Response, ApiError> {
    principal.require_master_key(NOT_A_TENANT)?;
    let path = config_yaml_path();
    if !path.is_file() {
        return Err(ApiError::not_found("config.yaml not found"));
    }
    let content = std::fs::read_to_string(&path).map_err(io_error)?;
    Ok(Json(json!({ "content": read_text_universal(&content) })).into_response())
}

/// `api_post_yaml`. Three failure modes, all 400: unparseable YAML, a root that
/// is not a mapping, and a document the schema rejects.
///
/// The file is written **verbatim** — `body.content`, not a re-serialization of
/// the parsed tree. Comments and formatting in an operator's config survive a
/// round trip through this route, which they would not if it saved the parse.
async fn post_config_yaml(principal: Principal, body: Bytes) -> Result<Response, ApiError> {
    principal.require_master_key(NOT_A_TENANT)?;
    let body = parse_body(&body)?;

    let content = match body.get("content") {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        Some(Value::String(_)) => {
            return Err(ApiError::validation(vec![ApiError::field_error(
                "content",
                "string_too_short",
                "String should have at least 1 character",
            )]))
        }
        None | Some(Value::Null) => {
            return Err(ApiError::validation(vec![ApiError::field_error(
                "content",
                "missing",
                "Field required",
            )]))
        }
        Some(_) => {
            return Err(ApiError::validation(vec![ApiError::field_error(
                "content",
                "string_type",
                "Input should be a valid string",
            )]))
        }
    };

    // `yaml.safe_load` returning `None` for an empty document is `parsed = {}`,
    // not a rejection — saving a config.yaml back to empty is allowed.
    let parsed: Value = serde_yaml::from_str(&content)
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, &format!("Invalid YAML: {e}")))?;
    let parsed = if parsed.is_null() { json!({}) } else { parsed };

    if !parsed.is_object() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "config root must be a mapping"));
    }
    if let Err(message) = crate::config_schema::validate(&parsed) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, &format!("Config schema: {message}")));
    }

    let path = config_yaml_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_error)?;
    }
    std::fs::write(&path, content.as_bytes()).map_err(io_error)?;

    Ok(Json(json!({
        "ok": true,
        "message": "Saved config.yaml. Restart the Agent Platform process if your deployment \
                    caches YAML.",
    }))
    .into_response())
}

// ---------------------------------------------------------------------------
// Probes
// ---------------------------------------------------------------------------

async fn health_proxy(State(state): State<Arc<AppState>>) -> Result<Response, ApiError> {
    let url = format!("{}/v1/health", internal_url());
    let response = send_with_retry("health_proxy", false, || {
        state.http.get(&url).timeout(Duration::from_secs(10))
    })
    .await?;
    Ok(Json(probe_body(&response, 2000)).into_response())
}

async fn health_readiness(State(state): State<Arc<AppState>>) -> Result<Response, ApiError> {
    let url = format!("{}/v1/health/readiness", internal_url());
    let response = send_with_retry("health_readiness", false, || {
        state.http.get(&url).timeout(Duration::from_secs(10))
    })
    .await?;
    Ok(Json(probe_body(&response, 2000)).into_response())
}

/// `GET /v1/models` as the operator's own key sees it, query string and all.
async fn proxy_models(
    State(state): State<Arc<AppState>>,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let master = master_key_from_env(&read_env_file());
    let mut url = format!("{}/v1/models", internal_url());
    if let Some(query) = query.filter(|q| !q.is_empty()) {
        url = format!("{url}?{query}");
    }
    let response = send_with_retry("proxy_models", false, || {
        state
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {master}"))
            .timeout(Duration::from_secs(30))
    })
    .await?;
    Ok(Json(probe_body(&response, 64000)).into_response())
}

// ---------------------------------------------------------------------------
// Catalog routes
// ---------------------------------------------------------------------------

async fn ui_providers(State(state): State<Arc<AppState>>) -> Response {
    Json(build_admin(&state.http).await).into_response()
}

/// Every alias `config.yaml` declares under a provider that is both supported
/// and configured — the model picker's option list, with no live probe behind it.
async fn ui_env_model_options() -> Response {
    Json(json!({ "models": config_yaml_model_names() })).into_response()
}

async fn ui_provider_models(
    State(state): State<Arc<AppState>>,
    PathId(provider_or_alias): PathId<String>,
) -> Result<Response, ApiError> {
    let entry = admin_entry(&state.http, &provider_or_alias).await?;
    let models = &entry["models"];
    Ok(Json(json!({
        "provider": entry["id"],
        "label": entry["label"],
        "configured": entry["configured"],
        "capabilities": entry["capabilities"],
        "models": models["options"],
        "default": models["default_model"],
        "source": models["source"],
        "warning": models["warning"],
        "fallback_note": models["fallback_note"],
    }))
    .into_response())
}

async fn test_model_options(
    State(state): State<Arc<AppState>>,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let catalog = build_admin(&state.http).await;
    let resolved = catalog["resolved_defaults"]["provider"].as_str().unwrap_or("").to_string();

    let requested = query_param(query.as_deref(), "provider").unwrap_or_default();
    let mut selected =
        if requested.is_empty() { resolved.clone() } else { requested }.trim().to_ascii_lowercase();
    if !is_supported_provider(&selected) {
        selected = resolved;
    }

    let rows = catalog["providers"].as_array().cloned().unwrap_or_default();
    let entry = rows
        .iter()
        .find(|row| row["id"] == json!(selected))
        .ok_or_else(|| ApiError::not_found("Unknown provider"))?;
    let models = &entry["models"];

    let providers: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "id": row["id"],
                "label": row["label"],
                "configured": row["configured"],
                "local": row["local"],
                "capabilities": row["capabilities"],
            })
        })
        .collect();

    Ok(Json(json!({
        "source": models["source"],
        "models": models["options"],
        "default": models["default_model"],
        "warning": models["warning"],
        "fallback_note": models["fallback_note"],
        "selected_provider": entry["id"],
        "resolved_defaults": catalog["resolved_defaults"],
        "persisted_defaults": catalog["persisted_defaults"],
        "providers": providers,
    }))
    .into_response())
}

/// The first value of `name` in a query string, the way FastAPI reads a single
/// optional `str` parameter.
fn query_param(query: Option<&str>, name: &str) -> Option<String> {
    url::form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

/// `_model_names_from_config_yaml`: alias names from both spellings of the file,
/// dropping any whose provider is unsupported or unconfigured.
///
/// Duplicates are **kept**, unlike everywhere else in the catalog — Python does
/// not dedupe this list, and the picker renders it verbatim.
fn config_yaml_model_names() -> Vec<String> {
    let data = load_config_yaml();
    let array = |v: Option<&Value>| v.and_then(Value::as_array).cloned().unwrap_or_default();
    let mut out: Vec<String> = Vec::new();

    for block in array(data.get("providers")) {
        let Some(block) = block.as_object() else { continue };
        let provider = yaml_str(block.get("name")).to_ascii_lowercase();
        if !is_supported_provider(&provider) || !provider_configured(&provider) {
            continue;
        }
        for item in array(block.get("models")) {
            let name = match &item {
                Value::String(s) => s.trim().to_string(),
                Value::Object(entry) => {
                    entry.get("model_name").and_then(Value::as_str).unwrap_or("").trim().to_string()
                }
                _ => String::new(),
            };
            if !name.is_empty() {
                out.push(name);
            }
        }
    }

    for entry in array(data.get("model_list")) {
        let Some(entry) = entry.as_object() else { continue };
        let name = entry.get("model_name").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if name.is_empty() {
            continue;
        }
        let provider = yaml_str(entry.get("provider")).to_ascii_lowercase();
        let backend = yaml_str(entry.get("model"));
        let kind = if is_supported_provider(&provider) && !backend.is_empty() {
            provider
        } else {
            let litellm = entry
                .get("litellm_params")
                .and_then(Value::as_object)
                .map(|p| yaml_str(p.get("model")))
                .unwrap_or_default();
            infer_provider_kind(&litellm)
        };
        if is_supported_provider(&kind) && !provider_configured(&kind) {
            continue;
        }
        out.push(name);
    }
    out
}

/// The provider behind a litellm-style `model` string. `"other"` for anything
/// unrecognised, which then passes the configured check because it is not a
/// supported provider id at all.
fn infer_provider_kind(backend_model: &str) -> String {
    let m = backend_model.trim().to_ascii_lowercase();
    let kind = if m.starts_with("ollama/") || m.starts_with("ollama_chat/") {
        "ollama"
    } else if m.starts_with("lm_studio/") || m.starts_with("lmstudio/") {
        "lm_studio"
    } else if m.starts_with("gemini") {
        "gemini"
    } else if m.starts_with("aimlapi/") || m.starts_with("openai/") {
        "aimlapi"
    } else {
        "other"
    };
    kind.to_string()
}

// ---------------------------------------------------------------------------
// Test routes
// ---------------------------------------------------------------------------

/// `ProxyTestBody`, validated the way pydantic validates it.
#[derive(Debug)]
struct TestBody {
    model: String,
    message: String,
    system: Option<String>,
    thinking: bool,
    messages: Option<Vec<Value>>,
    tools: Option<Vec<Value>>,
    tool_choice: Option<Value>,
    max_tokens: Option<i64>,
}

impl TestBody {
    fn parse(raw: &Bytes) -> Result<Self, ApiError> {
        let body = parse_body(raw)?;
        let mut errors = Vec::new();

        let model = required_str(&mut errors, &body, "model");
    // `Field(..., min_length=1)`, and only ever on an actual string:
    // pydantic reports one failure per field.
    if body.get("model").is_some_and(Value::is_string) {
        check_len(&mut errors, &["model"], Some(model.as_str()), 1, usize::MAX);
    }
        let message = defaulted_str(&mut errors, &body, "message", "Say OK in one word.");
        let system = optional_str(&mut errors, &body, "system");
        let thinking = lax_bool(&mut errors, &body, "thinking");
        let messages = object_list(&mut errors, body.get("messages"), "messages");
        let tools = object_list(&mut errors, body.get("tools"), "tools");

        // `str | dict | None`. A union reports **one failure per member**, both
        // under the member's own `loc` segment — not a single failure against
        // the first member.
        let tool_choice = match body.get("tool_choice") {
            None | Some(Value::Null) => None,
            Some(v @ (Value::String(_) | Value::Object(_))) => Some(v.clone()),
            Some(_) => {
                errors.push(ApiError::field_error_at(
                    &["tool_choice", "str"],
                    "string_type",
                    "Input should be a valid string",
                ));
                errors.push(ApiError::field_error_at(
                    &["tool_choice", "dict[str,any]"],
                    "dict_type",
                    "Input should be a valid dictionary",
                ));
                None
            }
        };

        let max_tokens = lax_int(&mut errors, &body, "max_tokens").inspect(|v| {
            if *v < 1 {
                errors.push(ApiError::field_error(
                    "max_tokens",
                    "greater_than_equal",
                    "Input should be greater than or equal to 1",
                ));
            } else if *v > 128_000 {
                errors.push(ApiError::field_error(
                    "max_tokens",
                    "less_than_equal",
                    "Input should be less than or equal to 128000",
                ));
            }
        });

        if !errors.is_empty() {
            return Err(ApiError::validation(errors));
        }
        Ok(Self { model, message, system, thinking, messages, tools, tool_choice, max_tokens })
    }

    /// `_test_chat_messages` — an explicit `messages` list wins outright, and
    /// `thinking` prefixes the system prompt rather than adding a message.
    fn chat_messages(&self) -> Vec<Value> {
        if let Some(messages) = &self.messages {
            return messages.clone();
        }
        let mut system = self.system.clone().unwrap_or_default().trim().to_string();
        if self.thinking && !system.starts_with("<|think|>") {
            system = format!("<|think|>{system}");
        }
        let mut out = Vec::new();
        if !system.is_empty() {
            out.push(json!({ "role": "system", "content": system }));
        }
        out.push(json!({ "role": "user", "content": self.message }));
        out
    }

    /// A plain one-shot "say OK" gets 32 tokens; anything the caller shaped
    /// itself, and every stream, gets 512.
    fn max_tokens_for(&self, stream: bool) -> i64 {
        if let Some(explicit) = self.max_tokens {
            return explicit;
        }
        let advanced = self.messages.is_some()
            || self.tools.as_ref().is_some_and(|t| !t.is_empty())
            || self.thinking
            || self.system.as_ref().is_some_and(|s| !s.trim().is_empty());
        if stream || advanced {
            512
        } else {
            32
        }
    }

    fn payload(&self, stream: bool) -> Value {
        let mut payload = json!({
            "model": self.model,
            "messages": self.chat_messages(),
            "max_tokens": self.max_tokens_for(stream),
            "stream": stream,
        });
        let tools = self.tools.as_ref().filter(|t| !t.is_empty());
        if let Some(tools) = tools {
            payload["tools"] = Value::Array(tools.to_vec());
        }
        if let Some(choice) = &self.tool_choice {
            payload["tool_choice"] = choice.clone();
        } else if tools.is_some() {
            payload["tool_choice"] = json!("auto");
        }
        payload
    }
}

/// `list[dict] | None`, rejecting a non-list and a non-object entry the way
/// pydantic does, entry by entry.
fn object_list(errors: &mut Vec<Value>, value: Option<&Value>, field: &str) -> Option<Vec<Value>> {
    match value {
        None | Some(Value::Null) => None,
        Some(Value::Array(items)) => {
            for (index, item) in items.iter().enumerate() {
                if !item.is_object() {
                    errors.push(ApiError::field_error_at(
                        &[field, &index.to_string()],
                        "dict_type",
                        "Input should be a valid dictionary",
                    ));
                }
            }
            Some(items.clone())
        }
        Some(_) => {
            errors.push(ApiError::field_error(field, "list_type", "Input should be a valid list"));
            None
        }
    }
}

async fn test_chat(State(state): State<Arc<AppState>>, raw: Bytes) -> Result<Response, ApiError> {
    let body = TestBody::parse(&raw)?;
    let master = master_key_from_env(&read_env_file());
    let payload = body.payload(false);
    let url = format!("{}/v1/chat/completions", internal_url());
    let response = send_with_retry("test_chat", false, || {
        state
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {master}"))
            .json(&payload)
            .timeout(Duration::from_secs(120))
    })
    .await?;
    Ok(Json(probe_body(&response, 16000)).into_response())
}

async fn test_embeddings(
    State(state): State<Arc<AppState>>,
    raw: Bytes,
) -> Result<Response, ApiError> {
    let body = parse_body(&raw)?;
    let mut errors = Vec::new();
    let model = required_str(&mut errors, &body, "model");
    // `Field(..., min_length=1)`, and only ever on an actual string:
    // pydantic reports one failure per field.
    if body.get("model").is_some_and(Value::is_string) {
        check_len(&mut errors, &["model"], Some(model.as_str()), 1, usize::MAX);
    }
    let input = defaulted_str(&mut errors, &body, "input", "hello");
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    let master = master_key_from_env(&read_env_file());
    let payload = json!({ "model": model, "input": input });
    let url = format!("{}/v1/embeddings", internal_url());
    let response = send_with_retry("test_embeddings", false, || {
        state
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {master}"))
            .json(&payload)
            .timeout(Duration::from_secs(60))
    })
    .await?;
    Ok(Json(probe_body(&response, 16000)).into_response())
}

async fn test_chat_stream(
    State(state): State<Arc<AppState>>,
    raw: Bytes,
) -> Result<Response, ApiError> {
    let body = TestBody::parse(&raw)?;
    let master = master_key_from_env(&read_env_file());
    let payload = body.payload(true);
    let url = format!("{}/v1/chat/completions", internal_url());

    let response = open_stream("test_chat_stream", || {
        state
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {master}"))
            .json(&payload)
            .timeout(Duration::from_secs(120))
    })
    .await?;

    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    if status.is_client_error() || status.is_server_error() {
        // The proxy refused before streaming anything, so answer with its body
        // rather than opening an event stream that only carries an error.
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/json")
            .to_string();
        let bytes = response.bytes().await.unwrap_or_default();
        return Ok((status, [(axum::http::header::CONTENT_TYPE, content_type)], bytes)
            .into_response());
    }

    let stream = response.bytes_stream().map(|chunk| match chunk {
        Ok(bytes) => Ok::<Bytes, std::convert::Infallible>(bytes),
        Err(e) => {
            let (code, message) = classify_with_context(&e, "test_chat_stream");
            Ok(Bytes::from(sse_error_chunk(code, &message)))
        }
    });
    Ok((
        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
        axum::body::Body::from_stream(stream),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_master_key_reaches_operator_config() {
        assert!(Principal::unrestricted().require_master_key(NOT_A_TENANT).is_ok());
        let tenant =
            Principal { workspace_id: Some(1), token_id: Some(2), scopes: vec!["*".into()], ..Principal::unrestricted() };
        assert_eq!(tenant.require_master_key(NOT_A_TENANT).unwrap_err().status, StatusCode::FORBIDDEN);
    }

    #[test]
    fn secrets_show_their_last_four_characters_only() {
        assert_eq!(mask_secret(""), "");
        assert_eq!(mask_secret("abcd"), "****");
        assert_eq!(mask_secret("sk-secret-1234"), "****1234");
    }

    #[test]
    fn a_plain_test_chat_is_capped_lower_than_a_shaped_one() {
        let plain = |body: Value| {
            TestBody::parse(&Bytes::from(serde_json::to_vec(&body).unwrap())).unwrap()
        };
        assert_eq!(plain(json!({"model": "m"})).max_tokens_for(false), 32);
        // A stream, a system prompt, tools or thinking all raise it.
        assert_eq!(plain(json!({"model": "m"})).max_tokens_for(true), 512);
        assert_eq!(plain(json!({"model": "m", "thinking": true})).max_tokens_for(false), 512);
        assert_eq!(plain(json!({"model": "m", "max_tokens": 7})).max_tokens_for(false), 7);
    }

    #[test]
    fn thinking_prefixes_the_system_prompt_rather_than_adding_a_message() {
        let body = TestBody::parse(&Bytes::from(
            serde_json::to_vec(&json!({"model": "m", "thinking": true, "system": "be terse"}))
                .unwrap(),
        ))
        .unwrap();
        let messages = body.chat_messages();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["content"], json!("<|think|>be terse"));
    }

    #[test]
    fn tools_imply_auto_tool_choice() {
        let body = TestBody::parse(&Bytes::from(
            serde_json::to_vec(&json!({"model": "m", "tools": [{"type": "function"}]})).unwrap(),
        ))
        .unwrap();
        assert_eq!(body.payload(false)["tool_choice"], json!("auto"));
        // No tools, no key at all.
        let bare =
            TestBody::parse(&Bytes::from(serde_json::to_vec(&json!({"model": "m"})).unwrap()))
                .unwrap();
        assert!(bare.payload(false).get("tool_choice").is_none());
    }

    #[test]
    fn booleans_are_coerced_the_way_pydantic_coerces_them() {
        let parse = |v: Value| {
            let body = json!({ "model": "m", "thinking": v });
            TestBody::parse(&Bytes::from(serde_json::to_vec(&body).unwrap()))
        };
        // Strings and 0/1 are booleans; `"yes"` reaching a Rust `bool` parser
        // as a 422 was a real divergence the cross-render caught.
        assert!(parse(json!("yes")).unwrap().thinking);
        assert!(parse(json!("ON")).unwrap().thinking);
        assert!(!parse(json!("off")).unwrap().thinking);
        assert!(parse(json!(1.0)).unwrap().thinking);
        assert!(!parse(json!(0)).unwrap().thinking);
        // Readable-but-wrong and wrong-type are different failures.
        assert_eq!(
            parse(json!("maybe")).unwrap_err().extra.unwrap()["errors"][0]["type"],
            json!("bool_parsing")
        );
        assert_eq!(
            parse(json!(2)).unwrap_err().extra.unwrap()["errors"][0]["type"],
            json!("bool_parsing")
        );
        assert_eq!(
            parse(json!(2.5)).unwrap_err().extra.unwrap()["errors"][0]["type"],
            json!("bool_type")
        );
        // A non-Optional field with a default still rejects an explicit null.
        assert_eq!(
            parse(json!(null)).unwrap_err().extra.unwrap()["errors"][0]["type"],
            json!("bool_type")
        );
    }

    #[test]
    fn config_yaml_is_read_with_universal_newlines() {
        // `Path.read_text` translates; `fs::read_to_string` does not, and the
        // file Python itself wrote on Windows is CRLF.
        assert_eq!(read_text_universal("a\r\nb\rc\n"), "a\nb\nc\n");
        assert_eq!(read_text_universal("a\nb"), "a\nb");
    }

    /// A sixth provider added to `llm_config::PROVIDERS` and nowhere else is the
    /// failure this catches: its key would come back in plaintext from
    /// `GET /env`, be accepted out of the committed YAML, and — for a chat
    /// provider — have no field on the desktop's Providers screen to set it in.
    /// All three lists follow the one table now, and this is what enforces that.
    ///
    /// Base URLs are deliberately not checked: `ENV_KEYS` is also the set of
    /// keys `write_env_file` owns and rewrites, and an override like
    /// `ANTHROPIC_OPENAI_BASE` is an operator escape hatch, not something the
    /// app should be managing the lifetime of.
    #[test]
    fn the_provider_table_is_the_source_of_the_key_lists() {
        for p in crate::llm_config::PROVIDERS {
            let Some(key) = p.api_key_env else { continue };
            assert!(
                SENSITIVE_ENV_KEYS.contains(&key),
                "{}'s {key} is not masked in GET /env, so it is also not refused from the YAML",
                p.id
            );
            if p.registry == crate::llm_config::Registry::Chat {
                assert!(
                    ENV_KEYS.contains(&key),
                    "{}'s {key} has no field on the Providers screen",
                    p.id
                );
            }
        }
    }

    /// `SEARCH_API_KEY`/`SEARCH_CX` are not driven by `llm_config::PROVIDERS` —
    /// search is not an LLM provider, so there is no `ProviderSpec` row for
    /// `the_provider_table_is_the_source_of_the_key_lists` to walk. This is
    /// the manual case that test's own doc comment calls for: `SPEECH_API_KEY`
    /// is the precedent (masked, no `ProviderSpec` requires it to be),
    /// `SEARCH_API_KEY` follows it exactly. `SEARCH_CX` breaks from the
    /// precedent on purpose — see `SENSITIVE_ENV_KEYS`'s doc comment for why —
    /// so this pins both directions rather than just the one that would fail
    /// silently.
    #[test]
    fn search_credentials_are_present_and_masked_correctly() {
        assert!(ENV_KEYS.contains(&"SEARCH_API_KEY"), "the .env editor must accept the search key");
        assert!(ENV_KEYS.contains(&"SEARCH_CX"), "the .env editor must accept the search engine id");
        assert!(SENSITIVE_ENV_KEYS.contains(&"SEARCH_API_KEY"), "the search key is a credential");
        assert!(
            !SENSITIVE_ENV_KEYS.contains(&"SEARCH_CX"),
            "the search engine id is not a credential"
        );
    }

    #[test]
    fn a_litellm_model_string_names_its_provider() {
        assert_eq!(infer_provider_kind("ollama_chat/llama3"), "ollama");
        assert_eq!(infer_provider_kind("lmstudio/qwen"), "lm_studio");
        assert_eq!(infer_provider_kind("gemini-2.0-flash"), "gemini");
        assert_eq!(infer_provider_kind("openai/gpt-4.1-mini"), "aimlapi");
        assert_eq!(infer_provider_kind("something/else"), "other");
    }
}
