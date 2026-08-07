//! The embedded LLM proxy's `/v1/*` surface, ported from
//! `app/llm_proxy/routes/llm.py` a route at a time (ADR 0007).
//!
//! Two things make this domain unlike the `/api/v1` ones already here:
//!
//! - **Auth is per route.** `app/main.py` mounts this router *without*
//!   `_api_deps`, so `require_token` — which guards exactly `/api/v1/*` — does
//!   not cover it. Routes that need a caller extract [`ProxyPrincipal`]; the two
//!   health routes take no auth at all, and must stay that way because the
//!   desktop probes them before it has a key.
//! - **It owns no tables.** Its whole state is the config files under
//!   `CONFIG_DIR` (see `llm_config`), so Rust and Python can serve it side by
//!   side while the rest is ported.
//!
//! Migrated so far: `/v1/health`, `/v1/health/readiness`, `/v1/models`,
//! `/v1/capabilities`. Everything else still falls through to the proxy.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::auth::ProxyPrincipal;
use crate::byok;
use crate::error::ApiError;
use crate::llm_config::{
    aimlapi_api_key, aimlapi_openai_base, anthropic_api_key, anthropic_openai_base,
    anthropic_version_header, default_model_for_provider, first_configured_provider,
    gemini_api_key, image_api_base, image_default_model, is_configured, is_supported_provider,
    load_config_yaml, lm_studio_api_base, lm_studio_api_key, modality_map, ollama_api_base,
    provider_configured, provider_supports, read_env_file, require_provider_for_capability,
    resolve_provider_for_capability, speech_api_base, speech_api_key, speech_default_format,
    speech_default_model, speech_default_voice, Modality, MODALITIES, PROVIDERS,
};
use crate::model_capabilities::ensure_chat_request_supported;
use crate::model_catalog::{
    coerce_local_model_if_needed, fetch_lm_studio_models, fetch_ollama_tags,
    fetch_openai_model_ids, lm_studio_headers, ollama_tag_matches, QUICK_TIMEOUT,
};
use crate::provider_catalog;
use crate::upstream_http::{classify_with_context, open_stream, send_with_retry, sse_error_chunk};
use crate::usage::normalize_completion_body;
use crate::{env_opt, AppState};

/// Python's health probes use 4s and its catalog reads 8s.
const PROBE_TIMEOUT: Duration = Duration::from_secs(4);

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/health/readiness", get(readiness))
        .route("/v1/models", get(list_models))
        .route("/v1/catalog", get(catalog))
        .route("/v1/capabilities", get(capabilities))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/embeddings", post(embeddings))
        .route("/v1/images/generations", post(images_generations))
        .route("/v1/audio/speech", post(audio_speech))
}

// ---------------------------------------------------------------------------
// Defaults and alias resolution
// ---------------------------------------------------------------------------

/// `DEFAULT_PROVIDER` / `DEFAULT_MODEL` read from `CONFIG_DIR/.env` *before* the
/// process environment — the one place that precedence is inverted.
///
/// The admin UI writes that file for a no-restart switch, and the YAML `env:`
/// block seeds the process environment permanently, so reading the environment
/// first would shadow the file for the life of the server.
pub(crate) fn setting_dotenv_first(key: &str) -> String {
    let from_file = read_env_file().get(key).map(|v| v.trim().to_string()).unwrap_or_default();
    if !from_file.is_empty() {
        return from_file;
    }
    env_opt(key).unwrap_or_default()
}

/// `str(value or "").strip()`, for a YAML field that may not be a string.
pub(crate) fn yaml_str(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) if n.as_f64() != Some(0.0) => n.to_string(),
        Some(Value::Bool(true)) => "True".to_string(),
        _ => String::new(),
    }
}

fn defaults_from_config(data: &Map<String, Value>) -> (String, String) {
    let block = data.get("defaults").and_then(Value::as_object);
    let provider = yaml_str(block.and_then(|d| d.get("provider"))).to_ascii_lowercase();
    let model = yaml_str(block.and_then(|d| d.get("model")));
    (if is_supported_provider(&provider) { provider } else { String::new() }, model)
}

/// The provider and model an unqualified request resolves to.
pub fn effective_defaults() -> (String, String) {
    let data = load_config_yaml();
    let (config_provider, config_model) = defaults_from_config(&data);
    let default_provider = setting_dotenv_first("DEFAULT_PROVIDER").to_ascii_lowercase();
    let default_model = setting_dotenv_first("DEFAULT_MODEL");

    let mut provider = if is_supported_provider(&default_provider) {
        default_provider
    } else {
        config_provider.clone()
    };
    let mut model = if provider == config_provider { config_model } else { String::new() };

    // Python tests `is_supported_provider` twice, but the first branch blanks
    // `p` before the second reads it, so the two collapse into this one.
    if !is_supported_provider(&provider) || !provider_configured(&provider) {
        provider = first_configured_provider().to_string();
        model = String::new();
    }

    if !default_model.is_empty() {
        model = default_model;
    } else if model.is_empty() {
        model = default_model_for_provider(&provider).to_string();
    }
    (provider, model)
}

/// Every alias `config.yaml` declares → `(provider, upstream model id)`.
///
/// Two spellings feed it: a `providers:` block whose `models:` are bare strings
/// or `{model_name, model}` pairs, and a flat `model_list:`. Later entries win,
/// which is what a dict assignment does in Python.
fn alias_map_raw(data: &Map<String, Value>) -> HashMap<String, (String, String)> {
    let mut out: HashMap<String, (String, String)> = HashMap::new();
    let array = |v: Option<&Value>| v.and_then(Value::as_array).cloned().unwrap_or_default();

    for block in array(data.get("providers")) {
        let Some(block) = block.as_object() else { continue };
        let provider = yaml_str(block.get("name")).to_ascii_lowercase();
        if !is_supported_provider(&provider) {
            continue;
        }
        for item in array(block.get("models")) {
            match &item {
                Value::String(s) if !s.trim().is_empty() => {
                    let s = s.trim().to_string();
                    out.insert(s.clone(), (provider.clone(), s));
                }
                Value::Object(entry) => {
                    let name = entry.get("model_name").and_then(Value::as_str).unwrap_or("").trim();
                    let model = entry.get("model").and_then(Value::as_str).unwrap_or("").trim();
                    if !name.is_empty() && !model.is_empty() {
                        out.insert(name.to_string(), (provider.clone(), model.to_string()));
                    }
                }
                _ => {}
            }
        }
    }

    for entry in array(data.get("model_list")) {
        let Some(entry) = entry.as_object() else { continue };
        let name = entry.get("model_name").and_then(Value::as_str).unwrap_or("").trim();
        if name.is_empty() {
            continue;
        }
        let provider = yaml_str(entry.get("provider")).to_ascii_lowercase();
        let model = yaml_str(entry.get("model"));
        if !is_supported_provider(&provider) || model.is_empty() {
            continue;
        }
        out.insert(name.to_string(), (provider, model));
    }
    out
}

/// Aliases whose provider is actually usable, sorted case-insensitively by alias
/// — the order the model list is rendered in.
fn alias_rows(data: &Map<String, Value>) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = alias_map_raw(data)
        .into_iter()
        .filter(|(_, (provider, _))| provider_configured(provider))
        .map(|(alias, (provider, _))| (alias, provider))
        .collect();
    rows.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    rows
}

// ---------------------------------------------------------------------------
// GET /v1/models
// ---------------------------------------------------------------------------

/// The four backends that can be asked for a live list. Declaration order is the
/// order their models are appended in.
const LIVE_SOURCES: [&str; 4] = ["ollama", "lm_studio", "aimlapi", "anthropic"];

async fn live_model_ids(http: &reqwest::Client, provider: &str) -> Vec<String> {
    match provider {
        "ollama" => fetch_ollama_tags(http, QUICK_TIMEOUT).await,
        "lm_studio" => fetch_lm_studio_models(http, QUICK_TIMEOUT).await,
        "aimlapi" => {
            let url = format!("{}/models", aimlapi_openai_base());
            let headers = vec![("Authorization".into(), format!("Bearer {}", aimlapi_api_key()))];
            fetch_openai_model_ids(http, &url, &headers, "v1_models_aimlapi", QUICK_TIMEOUT).await
        }
        "anthropic" => {
            // Claude rotates model ids fast enough that pinning them is worse
            // than asking. Its native list wants x-api-key, not Bearer.
            let url = format!("{}/models", anthropic_openai_base());
            let headers = vec![
                ("x-api-key".into(), anthropic_api_key()),
                ("anthropic-version".into(), anthropic_version_header()),
            ];
            fetch_openai_model_ids(http, &url, &headers, "v1_models_anthropic", QUICK_TIMEOUT).await
        }
        _ => Vec::new(),
    }
}

pub(crate) fn not_configured(provider: &str) -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        format!("Provider {provider} is not configured (check environment for this provider)."),
    )
}

/// `allowed = None` means every provider (`providers=all`).
fn parse_model_filter(query: Option<&str>) -> Result<Option<Vec<String>>, ApiError> {
    let mut repeated: Vec<String> = Vec::new();
    let mut single: Option<String> = None;
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        match key.as_ref() {
            "providers" if !value.trim().is_empty() => {
                repeated.push(value.trim().to_ascii_lowercase())
            }
            // FastAPI hands a single-value parameter the first occurrence.
            "provider" if single.is_none() => single = Some(value.trim().to_ascii_lowercase()),
            _ => {}
        }
    }

    let single = single.unwrap_or_default();
    if !single.is_empty() {
        if !is_supported_provider(&single) {
            return Err(ApiError::bad_request(
                "query provider must be ollama, lm_studio, gemini, aimlapi, or omitted",
            ));
        }
        if !provider_configured(&single) {
            return Err(not_configured(&single));
        }
    }

    if !repeated.is_empty() {
        if repeated.iter().any(|p| p == "all") {
            if repeated.len() != 1 {
                return Err(ApiError::bad_request(
                    "providers=all must not be combined with other provider values",
                ));
            }
            return Ok(None);
        }
        for provider in &repeated {
            if !is_supported_provider(provider) {
                return Err(ApiError::bad_request(format!(
                    "unknown provider in providers: {provider}"
                )));
            }
            if !provider_configured(provider) {
                return Err(not_configured(provider));
            }
        }
        return Ok(Some(repeated));
    }

    if !single.is_empty() {
        return Ok(Some(vec![single]));
    }

    // Nothing asked for: just the provider an unqualified request would use.
    let (effective, _) = effective_defaults();
    let provider =
        if is_supported_provider(&effective) { effective } else { "lm_studio".to_string() };
    Ok(Some(vec![provider]))
}

async fn list_models(
    _principal: ProxyPrincipal,
    State(state): State<Arc<AppState>>,
    RawQuery(query): RawQuery,
) -> Result<Json<Value>, ApiError> {
    let allowed = parse_model_filter(query.as_deref())?;
    let permitted = |provider: &str| allowed.as_ref().is_none_or(|a| a.iter().any(|p| p == provider));

    let mut rows: Vec<Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (alias, provider) in alias_rows(&load_config_yaml()) {
        if !permitted(&provider) {
            continue;
        }
        seen.push(alias.clone());
        rows.push(json!({ "id": alias, "object": "model", "owned_by": provider }));
    }

    for provider in LIVE_SOURCES {
        if !permitted(provider) || !provider_configured(provider) {
            continue;
        }
        for id in live_model_ids(&state.http, provider).await {
            if seen.contains(&id) {
                continue;
            }
            seen.push(id.clone());
            rows.push(json!({ "id": id, "object": "model", "owned_by": provider }));
        }
    }

    Ok(Json(json!({ "object": "list", "data": rows })))
}

// ---------------------------------------------------------------------------
// GET /v1/health
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct HealthQuery {
    provider: Option<String>,
    model: Option<String>,
}

fn respond(code: StatusCode, status: &str, mut body: Map<String, Value>, started: Instant) -> Response {
    body.insert("status".into(), json!(status));
    body.insert("elapsed_ms".into(), json!(started.elapsed().as_millis() as u64));
    (code, Json(Value::Object(body))).into_response()
}

fn unhealthy(mut body: Map<String, Value>, detail: impl Into<String>, started: Instant) -> Response {
    body.insert("detail".into(), json!(detail.into()));
    respond(StatusCode::SERVICE_UNAVAILABLE, "unhealthy", body, started)
}

/// Python slices the upstream's body to 500 *characters*.
fn head_500(text: &str) -> String {
    text.chars().take(500).collect()
}

/// LLM liveness: is this provider reachable?
///
/// Unauthenticated, like Python's. It never blocks on a catalog fetch — model
/// presence comes from the background cache, with its age alongside so a caller
/// can tell "absent" from "not looked at yet".
async fn health(State(state): State<Arc<AppState>>, Query(query): Query<HealthQuery>) -> Response {
    let (default_provider, default_model) = effective_defaults();
    let provider = {
        let asked = query.provider.unwrap_or_default().trim().to_ascii_lowercase();
        if asked.is_empty() { default_provider } else { asked }
    };
    let model = {
        let asked = query.model.unwrap_or_default().trim().to_string();
        if asked.is_empty() { default_model } else { asked }
    };

    if !is_supported_provider(&provider) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "error",
                "detail": "provider must be ollama, lm_studio, gemini, or aimlapi",
            })),
        )
            .into_response();
    }

    let started = Instant::now();
    let mut body = Map::new();
    body.insert("provider".into(), json!(provider));
    body.insert("model".into(), json!(model));

    match provider.as_str() {
        "ollama" | "lm_studio" => local_health(&state, &provider, &model, body, started).await,
        "aimlapi" | "anthropic" => keyed_health(&state, &provider, body, started).await,
        _ => gemini_health(&state, &model, body, started).await,
    }
}

/// The two loopback backends: probe, then answer from the cached catalog.
async fn local_health(
    state: &AppState,
    provider: &str,
    model: &str,
    mut body: Map<String, Value>,
    started: Instant,
) -> Response {
    let ollama = provider == "ollama";
    let base = if ollama { ollama_api_base() } else { lm_studio_api_base() };
    if base.is_empty() {
        let key = if ollama { "OLLAMA_API_BASE" } else { "LM_STUDIO_API_BASE" };
        return unhealthy(body, format!("{key} is not set"), started);
    }

    let base = base.trim_end_matches('/').to_string();
    let url = if ollama { format!("{base}/api/version") } else { format!("{base}/v1/models") };
    let headers = if ollama { Vec::new() } else { lm_studio_headers() };
    let context = if ollama { "health_ollama_version" } else { "health_lm_studio_version" };

    let response = send_with_retry(context, false, || {
        let mut req = state.http.get(&url).timeout(PROBE_TIMEOUT);
        for (name, value) in &headers {
            req = req.header(name, value);
        }
        req
    })
    .await;

    let response = match response {
        Ok(r) => r,
        Err(e) => return unhealthy(body, e.message, started),
    };
    body.insert("upstream_status".into(), json!(response.status.as_u16()));
    if !response.is_ok() {
        return unhealthy(body, head_500(&response.text()), started);
    }

    let (known, age, present) = if ollama {
        let tags = state.catalog.ollama_tags();
        let present = ollama_tag_matches(&tags, model);
        (!tags.is_empty(), state.catalog.ollama_tag_age_sec(), present)
    } else {
        let ids = state.catalog.lm_studio_models();
        let present = ids.iter().any(|id| id == model);
        (!ids.is_empty(), state.catalog.lm_studio_models_age_sec(), present)
    };
    if known {
        body.insert("model_present".into(), json!(present));
        body.insert("model_list_age_sec".into(), json!(age as i64));
    } else {
        // Never fetched, or the last fetch failed: "unknown", not "absent".
        body.insert("model_present".into(), Value::Null);
        body.insert("model_list_age_sec".into(), Value::Null);
    }
    respond(StatusCode::OK, "ok", body, started)
}

/// AIMLAPI and Anthropic: reachable is the whole check, no catalog involved.
async fn keyed_health(
    state: &AppState,
    provider: &str,
    mut body: Map<String, Value>,
    started: Instant,
) -> Response {
    let aimlapi = provider == "aimlapi";
    if !provider_configured(provider) {
        let key = if aimlapi { "AIMLAPI_API_KEY" } else { "ANTHROPIC_API_KEY" };
        return unhealthy(body, format!("{key} is not set"), started);
    }

    let (url, headers, context) = if aimlapi {
        (
            format!("{}/models", aimlapi_openai_base()),
            vec![("Authorization".to_string(), format!("Bearer {}", aimlapi_api_key()))],
            "health_aimlapi_models",
        )
    } else {
        (
            format!("{}/models", anthropic_openai_base()),
            vec![
                ("x-api-key".to_string(), anthropic_api_key()),
                ("anthropic-version".to_string(), anthropic_version_header()),
            ],
            "health_anthropic_models",
        )
    };

    let response = send_with_retry(context, false, || {
        let mut req = state.http.get(&url).timeout(PROBE_TIMEOUT);
        for (name, value) in &headers {
            req = req.header(name, value);
        }
        req
    })
    .await;

    match response {
        Err(e) => unhealthy(body, e.message, started),
        Ok(r) => {
            body.insert("upstream_status".into(), json!(r.status.as_u16()));
            if r.is_ok() {
                respond(StatusCode::OK, "ok", body, started)
            } else {
                respond(StatusCode::SERVICE_UNAVAILABLE, "unhealthy", body, started)
            }
        }
    }
}

async fn gemini_health(
    state: &AppState,
    model: &str,
    mut body: Map<String, Value>,
    started: Instant,
) -> Response {
    if !provider_configured("gemini") {
        return unhealthy(body, "GEMINI_API_KEY is not set", started);
    }
    // Model metadata rather than the model list: it answers the sharper question
    // of whether *this* model exists for *this* key.
    let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{model}");
    let key = gemini_api_key();

    let response = send_with_retry("health_gemini_meta", false, || {
        state.http.get(&url).query(&[("key", key.as_str())]).timeout(PROBE_TIMEOUT)
    })
    .await;

    match response {
        Err(e) => unhealthy(body, e.message, started),
        Ok(r) => {
            body.insert("upstream_status".into(), json!(r.status.as_u16()));
            if r.is_ok() {
                respond(StatusCode::OK, "ok", body, started)
            } else {
                respond(StatusCode::SERVICE_UNAVAILABLE, "unhealthy", body, started)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// POST /v1/chat/completions, POST /v1/embeddings
// ---------------------------------------------------------------------------

/// A request body must be a JSON object; every field below is read off it.
fn parse_object(raw: &[u8]) -> Result<Map<String, Value>, ApiError> {
    // ponytail: Python's message quotes its own decoder ("Expecting value: line
    // 1 column 1"). Status and code match; the text is serde's.
    match serde_json::from_slice::<Value>(raw) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(ApiError::bad_request("Invalid JSON: expected an object")),
        Err(e) => Err(ApiError::bad_request(format!("Invalid JSON: {e}"))),
    }
}

/// `body.get(key)` as a string, rejecting a non-string with `{key} must be a
/// string` the way the routes do. Absent and `null` both read as empty.
fn string_field(body: &Map<String, Value>, key: &'static str) -> Result<String, ApiError> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(s)) => Ok(s.trim().to_string()),
        Some(_) => Err(ApiError::bad_request(format!("{key} must be a string"))),
    }
}

fn is_truthy(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        _ => false,
    }
}

/// An alias resolves to its `(provider, upstream model)`; anything else is sent
/// to the default provider under the name the caller asked for.
fn resolve_model(requested: Option<&str>) -> Result<(String, String), ApiError> {
    let (default_provider, default_model) = effective_defaults();
    let Some(requested) = requested.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok((default_provider, default_model));
    };
    // Declared-but-unusable is a 503, not a silent fall-through to the default:
    // the caller asked for a specific backend and deserves to hear it is off.
    if let Some((provider, model)) = alias_map_raw(&load_config_yaml()).get(requested) {
        if !provider_configured(provider) {
            return Err(not_configured(provider));
        }
        return Ok((provider.clone(), model.clone()));
    }
    Ok((default_provider, requested.to_string()))
}

fn gemini_openai_base() -> String {
    env_opt("GEMINI_OPENAI_BASE")
        .unwrap_or_else(|| "https://generativelanguage.googleapis.com/v1beta/openai".into())
        .trim_end_matches('/')
        .to_string()
}

fn missing_base(code: &'static str, key: &str) -> ApiError {
    ApiError::coded(StatusCode::SERVICE_UNAVAILABLE, code, format!("{key} is not set."))
}

/// `(chat url, embeddings url)` for a provider.
fn upstream_urls(provider: &str) -> Result<(String, String), ApiError> {
    let openai_pair = |base: &str| {
        (format!("{base}/chat/completions"), format!("{base}/embeddings"))
    };
    match provider {
        "ollama" | "lm_studio" => {
            let ollama = provider == "ollama";
            let base = if ollama { ollama_api_base() } else { lm_studio_api_base() };
            let base = base.trim_end_matches('/').to_string();
            if base.is_empty() {
                return Err(if ollama {
                    missing_base("ollama_base_missing", "OLLAMA_API_BASE")
                } else {
                    missing_base("lm_studio_base_missing", "LM_STUDIO_API_BASE")
                });
            }
            // Both speak OpenAI on a `/v1` prefix of their own.
            Ok((format!("{base}/v1/chat/completions"), format!("{base}/v1/embeddings")))
        }
        "gemini" => Ok(openai_pair(&gemini_openai_base())),
        "aimlapi" => Ok(openai_pair(aimlapi_openai_base().trim_end_matches('/'))),
        // Anthropic's OpenAI-compatible surface has no embeddings endpoint; the
        // embeddings route rejects this provider before reaching the URL.
        "anthropic" => Ok(openai_pair(anthropic_openai_base().trim_end_matches('/'))),
        _ => Err(ApiError::coded(
            StatusCode::INTERNAL_SERVER_ERROR,
            "invalid_provider",
            "Invalid provider routing (internal).",
        )),
    }
}

fn missing_key(code: &'static str, message: &str) -> ApiError {
    ApiError::coded(StatusCode::SERVICE_UNAVAILABLE, code, message.to_string())
}

fn outbound_headers(provider: &str) -> Result<Vec<(String, String)>, ApiError> {
    let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
    let bearer = |key: String| ("Authorization".to_string(), format!("Bearer {key}"));

    match provider {
        "gemini" => {
            let key = gemini_api_key();
            if key.is_empty() {
                return Err(missing_key(
                    "gemini_key_missing",
                    "GEMINI_API_KEY is not configured for Gemini routes.",
                ));
            }
            headers.push(bearer(key));
        }
        "aimlapi" => {
            let key = aimlapi_api_key();
            if key.is_empty() {
                return Err(missing_key(
                    "aimlapi_key_missing",
                    "AIMLAPI_API_KEY is not configured for AIMLAPI routes.",
                ));
            }
            headers.push(bearer(key));
        }
        "anthropic" => {
            let key = anthropic_api_key();
            if key.is_empty() {
                return Err(missing_key(
                    "anthropic_key_missing",
                    "ANTHROPIC_API_KEY is not configured for Claude routes.",
                ));
            }
            // The OpenAI-compat chat endpoint takes Bearer; the version header
            // pins which schema it answers with.
            headers.push(bearer(key));
            headers.push(("anthropic-version".to_string(), anthropic_version_header()));
        }
        "lm_studio" => {
            let key = lm_studio_api_key();
            if !key.is_empty() {
                headers.push(bearer(key));
            }
        }
        _ => {}
    }
    Ok(headers)
}

fn apply(mut req: reqwest::RequestBuilder, headers: &[(String, String)]) -> reqwest::RequestBuilder {
    for (name, value) in headers {
        req = req.header(name, value);
    }
    req
}

/// The resolved upstream for a request: where to POST, and with what headers.
struct Target {
    url: String,
    headers: Vec<(String, String)>,
}

/// Where a platform-credentialed chat body goes: alias resolution, local-model
/// coercion and the capability guard, with `model` rewritten in place to the id
/// the upstream will actually be sent. BYOK does none of this — the key and the
/// model id are the caller's — so that branch stays in the handler.
async fn chat_target(state: &AppState, body: &mut Map<String, Value>) -> Result<Target, ApiError> {
    let requested_model = string_field(body, "model")?;
    let hint = string_field(body, "provider")?.to_ascii_lowercase();
    let (provider, resolved) = if hint.is_empty() {
        resolve_model(Some(&requested_model).filter(|m| !m.is_empty()).map(String::as_str))?
    } else {
        if !is_supported_provider(&hint) {
            return Err(ApiError::bad_request(
                "provider must be ollama, lm_studio, gemini, aimlapi, or anthropic",
            ));
        }
        if !provider_configured(&hint) {
            return Err(not_configured(&hint));
        }
        let (_, default_model) = effective_defaults();
        let model = if !requested_model.is_empty() {
            requested_model
        } else if !default_model_for_provider(&hint).is_empty() {
            default_model_for_provider(&hint).to_string()
        } else {
            default_model
        };
        (hint, model)
    };

    let resolved = coerce_local_model_if_needed(&state.http, &provider, &resolved).await;
    body.remove("provider");
    body.insert("model".into(), json!(resolved));
    ensure_chat_request_supported(&state.http, &provider, &resolved, body).await?;

    let (chat_url, _) = upstream_urls(&provider)?;
    Ok(Target { url: chat_url, headers: outbound_headers(&provider)? })
}

/// One buffered chat completion for a handler in this process, skipping the
/// loopback HTTP hop Python's `llm_client` takes back to `/v1/chat/completions`.
///
/// Same resolution, coercion, capability guard, retry policy and usage
/// normalisation as the public route — it is the same code — so a caller gets
/// what it would have got over the wire, minus a socket and an auth round trip.
/// Streaming has no internal caller and is not offered.
///
/// Every failure carries the status the public route would have answered with;
/// callers that mirror a Python handler's own error mapping read `.status` and
/// rewrite it (Python sees these as HTTP responses, not exceptions).
pub(crate) async fn complete_internal(
    state: &AppState,
    mut body: Map<String, Value>,
) -> Result<Value, ApiError> {
    let target = chat_target(state, &mut body).await?;
    let payload = Value::Object(body);

    let response = send_with_retry("chat_completions", true, || {
        apply(state.http.post(&target.url), &target.headers)
            .json(&payload)
            .timeout(Duration::from_secs(300))
    })
    .await?;

    if !response.is_ok() {
        return Err(ApiError::new(
            response.status,
            format!("LLM proxy returned HTTP {}", response.status.as_u16()),
        ));
    }
    let normalized =
        normalize_completion_body(&response.body, payload.get("messages").filter(|v| v.is_array()));
    serde_json::from_slice(&normalized).map_err(|e| {
        ApiError::new(StatusCode::BAD_GATEWAY, format!("Upstream returned invalid JSON: {e}"))
    })
}

pub(crate) async fn chat_completions(
    principal: ProxyPrincipal,
    State(state): State<Arc<AppState>>,
    request_headers: HeaderMap,
    raw: axum::body::Bytes,
) -> Result<Response, ApiError> {
    principal.0.require_scope("chat:write")?;
    let mut body = parse_object(&raw)?;
    let requested_model = string_field(&body, "model")?;

    let target = match byok::parse(&request_headers)? {
        // The client brought its own key: forward with the caller's credential
        // and spend none of the platform's quota. No alias resolution and no
        // local coercion — the model id is theirs to name.
        Some(route) => {
            route.require(Modality::Chat)?;
            if requested_model.is_empty() {
                return Err(ApiError::bad_request("model is required for BYOK requests"));
            }
            body.remove("provider");
            body.insert("model".into(), json!(requested_model));
            Target {
                url: route.url(Modality::Chat),
                headers: route
                    .outbound_headers()
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
            }
        }
        None => chat_target(&state, &mut body).await?,
    };

    let streaming = is_truthy(body.get("stream"));
    let payload = Value::Object(body);

    if !streaming {
        let response = send_with_retry("chat_completions", true, || {
            apply(state.http.post(&target.url), &target.headers)
                .json(&payload)
                .timeout(Duration::from_secs(300))
        })
        .await?;

        let content_type = response.content_type("application/json");
        let mut content = response.body;
        if response.status == StatusCode::OK {
            content = normalize_completion_body(
                &content,
                payload.get("messages").filter(|v| v.is_array()),
            );
        }
        return Ok((
            response.status,
            [(axum::http::header::CONTENT_TYPE, content_type)],
            content,
        )
            .into_response());
    }

    let response = open_stream("chat_completions_stream", || {
        apply(state.http.post(&target.url), &target.headers)
            .json(&payload)
            .timeout(Duration::from_secs(300))
    })
    .await?;

    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    if status.is_client_error() || status.is_server_error() {
        // The upstream refused before streaming anything, so answer with its
        // body rather than opening an event stream that only carries an error.
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/json")
            .to_string();
        let body = response.bytes().await.unwrap_or_default();
        return Ok((status, [(axum::http::header::CONTENT_TYPE, content_type)], body).into_response());
    }

    // A failure once the stream is open cannot change the status, so it goes out
    // as one more `data:` frame — which is what an SSE client is already reading.
    let stream = response.bytes_stream().map(|chunk| match chunk {
        Ok(bytes) => Ok::<Bytes, std::convert::Infallible>(bytes),
        Err(e) => {
            let (code, message) = classify_with_context(&e, "chat_completions_stream");
            Ok(Bytes::from(sse_error_chunk(code, &message)))
        }
    });
    Ok((
        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
        axum::body::Body::from_stream(stream),
    )
        .into_response())
}

async fn embeddings(
    principal: ProxyPrincipal,
    State(state): State<Arc<AppState>>,
    request_headers: HeaderMap,
    raw: axum::body::Bytes,
) -> Result<Response, ApiError> {
    principal.0.require_scope("chat:write")?;
    let mut body = parse_object(&raw)?;
    let requested_model = string_field(&body, "model")?;
    if requested_model.is_empty() {
        return Err(ApiError::bad_request("model is required"));
    }

    let target = match byok::parse(&request_headers)? {
        Some(route) => {
            route.require(Modality::Embeddings)?;
            body.insert("model".into(), json!(requested_model));
            Target {
                url: route.url(Modality::Embeddings),
                headers: route
                    .outbound_headers()
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
            }
        }
        None => {
            let (provider, resolved) = resolve_model(Some(&requested_model))?;
            if !provider_supports(&provider, Modality::Embeddings) {
                // A structured 501 rather than forwarding into a backend with no
                // embeddings endpoint, so the client learns which can serve it.
                return Err(ApiError::coded(
                    StatusCode::NOT_IMPLEMENTED,
                    "capability_unavailable",
                    format!(
                        "Provider {provider} does not expose an embeddings endpoint; \
                         use a provider that supports embeddings."
                    ),
                )
                .with_extra(json!({ "capability": "embeddings", "provider": provider })));
            }
            let resolved = coerce_local_model_if_needed(&state.http, &provider, &resolved).await;
            body.insert("model".into(), json!(resolved));

            let (_, embeddings_url) = upstream_urls(&provider)?;
            Target { url: embeddings_url, headers: outbound_headers(&provider)? }
        }
    };

    let payload = Value::Object(body);
    let response = send_with_retry("embeddings", true, || {
        apply(state.http.post(&target.url), &target.headers)
            .json(&payload)
            .timeout(Duration::from_secs(120))
    })
    .await?;

    Ok((
        response.status,
        [(axum::http::header::CONTENT_TYPE, response.content_type("application/json"))],
        response.body,
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// POST /v1/images/generations, POST /v1/audio/speech
// ---------------------------------------------------------------------------

/// The upstream for a capability backend, which lives in its own small registry
/// rather than among the chat providers.
fn backend_url(provider: &str, capability: Modality) -> Result<String, ApiError> {
    let (id, base, code, key, path) = match capability {
        Modality::ImageGeneration => (
            "image_local",
            image_api_base(),
            "image_base_missing",
            "IMAGE_API_BASE",
            "/v1/images/generations",
        ),
        _ => (
            "speech_local",
            speech_api_base(),
            "speech_base_missing",
            "SPEECH_API_BASE",
            "/v1/audio/speech",
        ),
    };
    if provider != id {
        // Reachable only through a `provider` hint naming something the
        // capability router let through as forward-compatible.
        let code = if capability == Modality::ImageGeneration {
            "invalid_image_provider"
        } else {
            "invalid_speech_provider"
        };
        let what = if capability == Modality::ImageGeneration { "image" } else { "speech" };
        return Err(ApiError::coded(
            StatusCode::INTERNAL_SERVER_ERROR,
            code,
            format!("Invalid {what} provider routing (internal)."),
        ));
    }
    if base.is_empty() {
        return Err(missing_base(code, key));
    }
    Ok(format!("{base}{path}"))
}

/// OpenAI-style image generation, routed to a configured image backend.
///
/// An optional `provider` hint pins one; otherwise the first configured image
/// backend answers. None configured is a structured 501, not a 500.
async fn images_generations(
    principal: ProxyPrincipal,
    State(state): State<Arc<AppState>>,
    request_headers: HeaderMap,
    raw: axum::body::Bytes,
) -> Result<Response, ApiError> {
    principal.0.require_scope("chat:write")?;
    let mut body = parse_object(&raw)?;

    let target = match byok::parse(&request_headers)? {
        Some(route) => {
            route.require(Modality::ImageGeneration)?;
            body.remove("provider");
            let model = string_field(&body, "model")?;
            if model.is_empty() {
                return Err(ApiError::bad_request("model is required for BYOK requests"));
            }
            body.insert("model".into(), json!(model));
            Target {
                url: route.url(Modality::ImageGeneration),
                headers: route
                    .outbound_headers()
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
            }
        }
        None => {
            let hint = string_field(&body, "provider")?.to_ascii_lowercase();
            let provider = require_provider_for_capability(
                Modality::ImageGeneration,
                Some(&hint).filter(|h| !h.is_empty()).map(String::as_str),
            )?;
            body.remove("provider");
            let model = string_field(&body, "model")?;
            body.insert(
                "model".into(),
                json!(if model.is_empty() { image_default_model() } else { model }),
            );
            Target {
                url: backend_url(&provider, Modality::ImageGeneration)?,
                headers: vec![("Content-Type".into(), "application/json".into())],
            }
        }
    };

    let payload = Value::Object(body);
    // Diffusion is slow enough that the usual budget would time out a good run.
    let response = send_with_retry("images_generations", true, || {
        apply(state.http.post(&target.url), &target.headers)
            .json(&payload)
            .timeout(Duration::from_secs(600))
    })
    .await?;

    Ok((
        response.status,
        [(axum::http::header::CONTENT_TYPE, response.content_type("application/json"))],
        response.body,
    )
        .into_response())
}

/// OpenAI-style text-to-speech, routed to the configured speech backend.
///
/// The body is audio, so it is returned as-is under the upstream's content type
/// rather than parsed. No backend configured is a structured 501 — which is what
/// the desktop reads as "use your own voice engine".
async fn audio_speech(
    principal: ProxyPrincipal,
    State(state): State<Arc<AppState>>,
    raw: axum::body::Bytes,
) -> Result<Response, ApiError> {
    principal.0.require_scope("chat:write")?;
    let mut body = parse_object(&raw)?;

    // A non-string `input` reports the same thing as a missing one: there is
    // nothing to say.
    let has_input = body.get("input").and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty());
    if !has_input {
        return Err(ApiError::bad_request("input is required"));
    }

    let hint = string_field(&body, "provider")?.to_ascii_lowercase();
    body.remove("provider");
    let provider = require_provider_for_capability(
        Modality::Speech,
        Some(&hint).filter(|h| !h.is_empty()).map(String::as_str),
    )?;

    for (field, default) in [
        ("model", speech_default_model()),
        ("voice", speech_default_voice()),
        ("response_format", speech_default_format()),
    ] {
        let value = string_field(&body, field)?;
        body.insert(field.into(), json!(if value.is_empty() { default } else { value }));
    }

    let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
    // A hosted upstream needs a key; a local Piper or Kokoro server takes none.
    let key = speech_api_key();
    if !key.is_empty() {
        headers.push(("Authorization".to_string(), format!("Bearer {key}")));
    }

    let url = backend_url(&provider, Modality::Speech)?;
    let payload = Value::Object(body);
    let response = send_with_retry("audio_speech", true, || {
        apply(state.http.post(&url), &headers).json(&payload).timeout(Duration::from_secs(120))
    })
    .await?;

    Ok((
        response.status,
        [(axum::http::header::CONTENT_TYPE, response.content_type("audio/mpeg"))],
        response.body,
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// GET /v1/catalog
// ---------------------------------------------------------------------------

/// Provider registry with models and upstream metadata.
///
/// Configured providers, whether each answered just now, its default model, and
/// per-model metadata and capabilities. Model `id` values are what a client
/// passes to `POST /v1/chat/completions`.
///
/// `live=false` returns only the `config.yaml` aliases, with no upstream call at
/// all — which is what a caller wants when a backend is known to be down and the
/// screen still has to render.
async fn catalog(
    _principal: ProxyPrincipal,
    State(state): State<Arc<AppState>>,
    RawQuery(query): RawQuery,
) -> Result<Json<Value>, ApiError> {
    let query = query.as_deref();
    let allowed = provider_catalog::parse_filter(query)?;
    let include_live = provider_catalog::query_flag(query, "live", true);
    let probe_capabilities =
        provider_catalog::query_flag(query, "probe_capabilities", true) && include_live;

    Ok(Json(
        provider_catalog::build(
            &state.http,
            provider_catalog::CatalogOptions { allowed, include_live, probe_capabilities },
        )
        .await,
    ))
}

// ---------------------------------------------------------------------------
// The two pure reads
// ---------------------------------------------------------------------------

/// Dependency-light readiness probe for the proxy's configuration.
///
/// Always 200: `first_configured_provider` falls back to `lm_studio`, and both
/// local backends carry a loopback default, so "no provider can be resolved" is
/// unreachable. Python has the 503 branch anyway; it answers the same as this.
async fn readiness() -> Json<Value> {
    let provider = first_configured_provider();
    Json(json!({
        "status": "ok",
        "checks": [{
            "name": "provider_config",
            "ok": true,
            "detail": format!("default provider can resolve to {provider}"),
        }],
    }))
}

/// Capability (modality) map plus the resolved provider per capability.
///
/// Lets a client ask "who can do X" up front instead of discovering an
/// unsupported route through a failed request. `resolved` is the provider the
/// proxy would pick for an unqualified request of that capability, or null when
/// none is configured.
async fn capabilities(_principal: ProxyPrincipal) -> Json<Value> {
    let providers: Map<String, Value> = PROVIDERS
        .iter()
        .map(|spec| {
            let mut row = modality_map(spec.id);
            row.insert("configured".into(), Value::Bool(is_configured(spec.id)));
            (spec.id.to_string(), Value::Object(row))
        })
        .collect();

    let resolved: Map<String, Value> = MODALITIES
        .iter()
        .map(|m| {
            let provider = resolve_provider_for_capability(*m);
            (m.as_str().to_string(), provider.map_or(Value::Null, Value::from))
        })
        .collect();

    Json(json!({
        "object": "capabilities",
        "modalities": MODALITIES.iter().map(|m| m.as_str()).collect::<Vec<_>>(),
        "providers": providers,
        "resolved": resolved,
        "byok": byok::discovery(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml(text: &str) -> Map<String, Value> {
        match serde_yaml::from_str::<Value>(text) {
            Ok(Value::Object(map)) => map,
            _ => Map::new(),
        }
    }

    #[test]
    fn aliases_come_from_both_spellings_and_later_entries_win() {
        let data = yaml(
            "providers:\n\
             \x20 - name: ollama\n\
             \x20   models:\n\
             \x20     - llama3\n\
             \x20     - model_name: fast\n\
             \x20       model: llama3:8b\n\
             \x20 - name: not_a_provider\n\
             \x20   models: [ignored]\n\
             model_list:\n\
             \x20 - model_name: fast\n\
             \x20   provider: gemini\n\
             \x20   model: gemini-2.0-flash\n\
             \x20 - model_name: skipped\n\
             \x20   provider: gemini\n",
        );
        let raw = alias_map_raw(&data);

        assert_eq!(raw["llama3"], ("ollama".into(), "llama3".into()));
        // model_list is applied after providers, so it takes the alias over.
        assert_eq!(raw["fast"], ("gemini".into(), "gemini-2.0-flash".into()));
        // An unregistered provider and an entry with no model are both dropped.
        assert!(!raw.contains_key("ignored"));
        assert!(!raw.contains_key("skipped"));
    }

    #[test]
    fn ollama_tags_match_on_the_bare_name() {
        let tags = vec!["llama3:8b".to_string(), "qwen3-coder:30b".to_string()];
        assert!(ollama_tag_matches(&tags, "llama3:8b"));
        assert!(ollama_tag_matches(&tags, "llama3"), "bare name matches any tag of it");
        assert!(ollama_tag_matches(&tags, "llama3:70b"), "python compares only the bare names");
        assert!(!ollama_tag_matches(&tags, "mistral"));
        assert!(!ollama_tag_matches(&[], "llama3"));
    }

    #[test]
    fn model_filter_rejects_what_python_rejects() {
        let err = parse_model_filter(Some("provider=banana")).unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err.message.starts_with("query provider must be"), "{}", err.message);

        let err = parse_model_filter(Some("providers=all&providers=ollama")).unwrap_err();
        assert_eq!(err.message, "providers=all must not be combined with other provider values");

        let err = parse_model_filter(Some("providers=banana")).unwrap_err();
        assert_eq!(err.message, "unknown provider in providers: banana");

        // `all` means no filter; a repeated list keeps its order.
        assert!(parse_model_filter(Some("providers=all")).unwrap().is_none());
        assert_eq!(
            parse_model_filter(Some("providers=ollama&providers=lm_studio")).unwrap(),
            Some(vec!["ollama".to_string(), "lm_studio".to_string()])
        );
        assert_eq!(parse_model_filter(Some("provider=Ollama")).unwrap(), Some(vec!["ollama".into()]));
    }
}
