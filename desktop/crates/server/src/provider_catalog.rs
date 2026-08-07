//! The provider registry behind `GET /v1/catalog`. Port of the `/v1` half of
//! `app/llm_proxy/services/provider_catalog.py`.
//!
//! One row per chat provider: whether it is configured, whether it answered just
//! now, the model it would pick, and every model it can offer with per-model
//! capabilities attached. It is what the desktop's provider screen renders.
//!
//! Model discovery degrades in a fixed order — live upstream list, then
//! `config.yaml` aliases, then the UI's `fallback_models`, then the provider's
//! built-in default — so the screen always has something to show and each row
//! says which of those it is (`source`) and whether the backend was reachable.
//!
//! The admin surface's *other* catalog shape — `build_provider_catalog`, behind
//! `/api/v1/llm-proxy/ui/*` — is [`build_admin`] further down: a different body
//! for a different screen, over the same discovery.

use std::collections::HashMap;
use std::time::Duration;

use futures::StreamExt;
use serde_json::{json, Map, Value};

use crate::error::ApiError;
use crate::llm::{not_configured, setting_dotenv_first, yaml_str};
use crate::llm_config::{
    aimlapi_api_key, aimlapi_openai_base, anthropic_api_key, anthropic_openai_base,
    anthropic_version_header, default_model_for_provider, first_configured_provider,
    gemini_api_key, is_supported_provider, load_config_yaml, lm_studio_api_base, modality_map,
    ollama_api_base, provider_configured, provider_ids, provider_label, read_env_file,
    read_ui_fallbacks, Registry,
};
use crate::model_capabilities::{provider_default_capabilities, resolve_model_capabilities};
use crate::model_catalog::{fetch_lm_studio_models, fetch_lm_studio_native_keys, fetch_openai_model_ids};
use crate::upstream_http::send_with_retry;

/// Python probes the catalog with a longer budget than the health path: these
/// calls are behind a screen the user is already waiting on.
const CATALOG_TIMEOUT: Duration = Duration::from_secs(12);
const CLOUD_TIMEOUT: Duration = Duration::from_secs(15);
const GEMINI_TIMEOUT: Duration = Duration::from_secs(20);

/// How each provider's model list is discovered, reported verbatim so a client
/// can explain a stale or empty list without guessing.
fn discovery(provider: &str) -> Value {
    let (primary, fallbacks): (&str, &[&str]) = match provider {
        "ollama" => ("ollama_tags", &["config_aliases", "ui_fallback_models", "provider_default"]),
        "lm_studio" => (
            "lm_studio_models",
            &[
                "lm_studio_native_models",
                "config_aliases",
                "ui_fallback_models",
                "provider_default",
            ],
        ),
        _ => ("upstream_models", &["config_aliases", "ui_fallback_models", "provider_default"]),
    };
    json!({ "mode": "dynamic", "primary_source": primary, "fallback_sources": fallbacks })
}

fn provider_capabilities(provider: &str) -> Value {
    json!({
        "streaming": true,
        // Gemini's OpenAI-compatible surface takes no tool definitions.
        "tools": provider != "gemini",
        "json_mode": true,
        "modalities": Value::Object(modality_map(provider)),
        "model_discovery": discovery(provider),
    })
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

/// The `defaults:` block, blanked entirely when it names a provider that is not
/// configured — unlike the route layer's version, which keeps the name and lets
/// the caller's request fail with a 503 that says so.
fn defaults_from_config(data: &Map<String, Value>) -> (String, String) {
    let block = data.get("defaults").and_then(Value::as_object);
    let mut provider = yaml_str(block.and_then(|d| d.get("provider"))).to_ascii_lowercase();
    let model = yaml_str(block.and_then(|d| d.get("model")));
    if !is_supported_provider(&provider) {
        provider = String::new();
    }
    if !provider.is_empty() && !provider_configured(&provider) {
        return (String::new(), String::new());
    }
    (provider, model)
}

/// What the proxy would pick for an unqualified request, as the catalog reports it.
pub fn resolved_defaults() -> Value {
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

    if !is_supported_provider(&provider) || !provider_configured(&provider) {
        provider = first_configured_provider().to_string();
        model = String::new();
    }
    if !default_model.is_empty() {
        model = default_model;
    } else if model.is_empty() {
        model = default_model_for_provider(&provider).to_string();
    }
    json!({ "provider": provider, "model": model })
}

// ---------------------------------------------------------------------------
// Aliases and fallbacks
// ---------------------------------------------------------------------------

fn dedupe(values: Vec<String>) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for value in values {
        let value = value.trim().to_string();
        if !value.is_empty() && !seen.contains(&value) {
            seen.push(value);
        }
    }
    seen
}

/// Alias *names* per provider — the catalog shows what a caller may ask for, not
/// what it maps to upstream.
fn aliases_by_provider() -> HashMap<String, Vec<String>> {
    let data = load_config_yaml();
    let mut out: HashMap<String, Vec<String>> =
        provider_ids(Registry::Chat).into_iter().map(|id| (id.to_string(), Vec::new())).collect();
    let array = |v: Option<&Value>| v.and_then(Value::as_array).cloned().unwrap_or_default();

    for block in array(data.get("providers")) {
        let Some(block) = block.as_object() else { continue };
        let provider = yaml_str(block.get("name")).to_ascii_lowercase();
        if !is_supported_provider(&provider) {
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
                out.entry(provider.clone()).or_default().push(name);
            }
        }
    }

    for entry in array(data.get("model_list")) {
        let Some(entry) = entry.as_object() else { continue };
        let provider = yaml_str(entry.get("provider")).to_ascii_lowercase();
        let name = entry.get("model_name").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let model = yaml_str(entry.get("model"));
        if is_supported_provider(&provider) && !name.is_empty() && !model.is_empty() {
            out.entry(provider).or_default().push(name);
        }
    }

    out.into_iter().map(|(provider, names)| (provider, dedupe(names))).collect()
}

/// `orchestrator_ui.yaml`'s `fallback_models`, keyed by provider *or* by any of
/// its aliases — the file is written by hand and both spellings appear.
fn fallback_models(provider: &str, aliases: &[String]) -> Vec<String> {
    let map = read_ui_fallbacks();
    let mut values: Vec<String> = map.get(provider).cloned().unwrap_or_default();
    for alias in aliases {
        if let Some(extra) = map.get(alias) {
            values.extend(extra.clone());
        }
    }
    dedupe(values)
}

fn alias_rows(names: &[String]) -> Vec<Value> {
    names.iter().map(|id| json!({ "id": id, "source": "alias" })).collect()
}

// ---------------------------------------------------------------------------
// Live discovery
// ---------------------------------------------------------------------------

/// Ollama's tags with the metadata its list carries. The version ping comes
/// first so an unreachable backend is reported as such rather than as "no
/// models" — the two look identical in the tag list alone.
async fn ollama_entries(http: &reqwest::Client) -> (Vec<Value>, bool) {
    let base = ollama_api_base();
    if base.is_empty() {
        return (Vec::new(), false);
    }
    let base = base.trim_end_matches('/').to_string();

    let version = send_with_retry("v1_catalog_ollama_version", false, || {
        http.get(format!("{base}/api/version")).timeout(CATALOG_TIMEOUT)
    })
    .await;
    let reachable = matches!(&version, Ok(r) if r.is_ok());
    if !reachable {
        return (Vec::new(), false);
    }

    let Ok(response) = send_with_retry("v1_catalog_ollama_tags", false, || {
        http.get(format!("{base}/api/tags")).timeout(CATALOG_TIMEOUT)
    })
    .await
    else {
        return (Vec::new(), reachable);
    };
    if !response.is_ok() {
        return (Vec::new(), reachable);
    }

    let rows = response
        .json()
        .as_ref()
        .and_then(|v| v.get("models"))
        .and_then(Value::as_array)
        .map(|models| {
            models
                .iter()
                .filter_map(|item| {
                    let name = item.get("name").and_then(Value::as_str)?.trim();
                    if name.is_empty() {
                        return None;
                    }
                    let details = item.get("details").and_then(Value::as_object);
                    let mut metadata = Map::new();
                    for key in ["family", "parameter_size", "quantization_level"] {
                        if let Some(value) = details.and_then(|d| d.get(key)) {
                            metadata.insert(key.into(), value.clone());
                        }
                    }
                    if let Some(size) = item.get("size") {
                        metadata.insert("size".into(), size.clone());
                    }
                    Some(json!({ "id": name, "source": "live", "metadata": metadata }))
                })
                .collect()
        })
        .unwrap_or_default();
    (rows, reachable)
}

/// Model ids from a provider that is not Ollama, or `None` when it did not answer.
async fn discovered_models(http: &reqwest::Client, provider: &str) -> Option<Vec<String>> {
    discovered_with_source(http, provider).await.0
}

/// Ollama's tag names alone. `_fetch_ollama_models`, which — unlike the `/v1`
/// catalog's [`ollama_entries`] — does not ping `/api/version` first, so an
/// unreachable backend and an empty library are the same answer here.
async fn ollama_tag_names(http: &reqwest::Client) -> Vec<String> {
    let base = ollama_api_base();
    if base.is_empty() {
        return Vec::new();
    }
    let Ok(response) = send_with_retry("provider_catalog_ollama", false, || {
        http.get(format!("{}/api/tags", base.trim_end_matches('/'))).timeout(CATALOG_TIMEOUT)
    })
    .await
    else {
        return Vec::new();
    };
    if !response.is_ok() {
        return Vec::new();
    }
    response
        .json()
        .as_ref()
        .and_then(|v| v.get("models"))
        .and_then(Value::as_array)
        .map(|models| {
            models
                .iter()
                .filter_map(|item| item.get("name").and_then(Value::as_str))
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// `_fetch_provider_models` — the ids, plus the name of the discovery that
/// produced them.
///
/// The source is reported **even when the fetch came back empty**, because that
/// is what Python returns and what the admin catalog's `source` field then
/// shows for a configured provider whose list is unavailable and which has no
/// aliases, no UI fallbacks and no default model to fall back to.
async fn discovered_with_source(
    http: &reqwest::Client,
    provider: &str,
) -> (Option<Vec<String>>, Option<&'static str>) {
    let (ids, source): (Vec<String>, Option<&'static str>) = match provider {
        "ollama" => (ollama_tag_names(http).await, Some("ollama_tags")),
        "lm_studio" => {
            // Both URLs are the same host and port, so probing together costs one
            // spare local request when LM Studio is up and halves the stall when
            // it is not. The OpenAI shape still wins.
            let base = lm_studio_api_base();
            let (openai, native) = futures::join!(
                fetch_lm_studio_models(http, CATALOG_TIMEOUT),
                fetch_lm_studio_native_keys(http, &base, CATALOG_TIMEOUT)
            );
            if !openai.is_empty() {
                (openai, Some("lm_studio_models"))
            } else if !native.is_empty() {
                (native, Some("lm_studio_native_models"))
            } else {
                (Vec::new(), None)
            }
        }
        "aimlapi" if provider_configured("aimlapi") => {
            let url = format!("{}/models", aimlapi_openai_base());
            let headers = vec![("Authorization".into(), format!("Bearer {}", aimlapi_api_key()))];
            let ids = fetch_openai_model_ids(
                http,
                &url,
                &headers,
                "provider_catalog_aimlapi",
                CLOUD_TIMEOUT,
            )
            .await;
            (ids, Some("upstream_models"))
        }
        "anthropic" if provider_configured("anthropic") => {
            let url = format!("{}/models", anthropic_openai_base());
            let headers = vec![
                ("x-api-key".into(), anthropic_api_key()),
                ("anthropic-version".into(), anthropic_version_header()),
            ];
            let ids = fetch_openai_model_ids(
                http,
                &url,
                &headers,
                "provider_catalog_anthropic",
                CLOUD_TIMEOUT,
            )
            .await;
            (ids, Some("upstream_models"))
        }
        "gemini" => (gemini_models(http).await, Some("upstream_models")),
        _ => (Vec::new(), None),
    };
    ((!ids.is_empty()).then(|| dedupe(ids)), source)
}

/// Gemini's native list, which prefixes ids with `models/` and includes
/// embedding models the chat catalog has no use for.
async fn gemini_models(http: &reqwest::Client) -> Vec<String> {
    let key = gemini_api_key();
    if key.is_empty() {
        return Vec::new();
    }
    let Ok(response) = send_with_retry("provider_catalog_gemini", false, || {
        http.get("https://generativelanguage.googleapis.com/v1beta/models")
            .query(&[("key", key.as_str())])
            .timeout(GEMINI_TIMEOUT)
    })
    .await
    else {
        return Vec::new();
    };
    if !response.is_ok() {
        return Vec::new();
    }
    response
        .json()
        .as_ref()
        .and_then(|v| v.get("models"))
        .and_then(Value::as_array)
        .map(|models| {
            models
                .iter()
                .filter_map(|item| item.get("name").and_then(Value::as_str))
                .filter_map(|name| name.strip_prefix("models/"))
                .map(str::trim)
                .filter(|id| !id.is_empty() && !id.to_ascii_lowercase().contains("embed"))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Attach `capabilities` to each row, four probes at a time. Only a `live` row is
/// probed — an alias names something that may not be pulled yet.
async fn enrich(http: &reqwest::Client, provider: &str, rows: &mut [Value], probe: bool) {
    // Owned pairs, not borrowed rows: a future that borrows the slice it is
    // about to write back into does not satisfy the stream's lifetime bounds.
    let jobs: Vec<(String, bool)> = rows
        .iter()
        .map(|row| {
            let id = row.get("id").and_then(Value::as_str).unwrap_or("").to_string();
            let live = row.get("source").and_then(Value::as_str) == Some("live");
            (id, live)
        })
        .collect();

    let resolved: Vec<Map<String, Value>> = futures::stream::iter(jobs)
        .map(|(id, live)| async move {
            resolve_model_capabilities(http, provider, &id, probe && live).await
        })
        .buffered(4)
        .collect()
        .await;

    for (row, caps) in rows.iter_mut().zip(resolved) {
        if let Some(row) = row.as_object_mut() {
            row.insert("capabilities".into(), Value::Object(caps));
        }
    }
}

// ---------------------------------------------------------------------------
// Assembly
// ---------------------------------------------------------------------------

pub struct CatalogOptions {
    pub allowed: Option<Vec<String>>,
    pub include_live: bool,
    pub probe_capabilities: bool,
}

pub async fn build(http: &reqwest::Client, options: CatalogOptions) -> Value {
    let aliases_by_provider = aliases_by_provider();
    let defaults = resolved_defaults();
    let default_provider = defaults["provider"].as_str().unwrap_or("").to_string();
    let default_model = defaults["model"].as_str().unwrap_or("").trim().to_string();

    let mut providers = Vec::new();
    for provider in provider_ids(Registry::Chat) {
        if options.allowed.as_ref().is_some_and(|a| !a.iter().any(|p| p == provider)) {
            continue;
        }
        let configured = provider_configured(provider);
        let aliases = aliases_by_provider.get(provider).cloned().unwrap_or_default();
        // `None` means "not asked"; it only becomes a boolean once something has
        // actually tried to reach the backend.
        let mut reachable: Option<bool> = if configured { None } else { Some(false) };
        let mut models: Vec<Value> = Vec::new();
        let mut default_for_row = default_model_for_provider(provider).to_string();

        if configured && options.include_live {
            if provider == "ollama" {
                let (rows, ok) = ollama_entries(http).await;
                models = rows;
                reachable = Some(ok);
                if !models.is_empty() {
                    enrich(http, provider, &mut models, options.probe_capabilities).await;
                }
            } else {
                let discovered = discovered_models(http, provider).await;
                reachable = Some(discovered.is_some());
                if let Some(ids) = discovered {
                    models = ids.iter().map(|id| json!({ "id": id, "source": "live" })).collect();
                    enrich(http, provider, &mut models, options.probe_capabilities).await;
                }
            }
        }

        // `live=false` skips the probe entirely, so rows still need a capability
        // block; the provider's declared set is the honest answer.
        if models.first().is_some_and(|row| row.get("capabilities").is_none()) {
            let caps = Value::Object(provider_default_capabilities(provider));
            for row in &mut models {
                if let Some(row) = row.as_object_mut() {
                    row.insert("capabilities".into(), caps.clone());
                }
            }
        }

        if models.is_empty() {
            models = alias_rows(&aliases);
            if configured && reachable.is_none() {
                reachable = Some(false);
            }
        }
        if models.is_empty() {
            models = alias_rows(&fallback_models(provider, &aliases));
        }
        if models.is_empty() && !default_for_row.is_empty() {
            models = vec![json!({ "id": default_for_row, "source": "alias" })];
        }

        if default_provider == provider && !default_model.is_empty() {
            default_for_row = default_model.clone();
        } else if let Some(first) = models.first().and_then(|m| m.get("id")).and_then(Value::as_str)
        {
            default_for_row = first.to_string();
        }

        providers.push(json!({
            "id": provider,
            "label": provider_label(provider),
            "configured": configured,
            "reachable": reachable,
            "default_model": default_for_row,
            "capabilities": provider_capabilities(provider),
            "models": models,
        }));
    }

    json!({ "object": "catalog", "resolved_defaults": defaults, "providers": providers })
}

// ---------------------------------------------------------------------------
// The admin shape
// ---------------------------------------------------------------------------

/// What an operator has actually *saved*, as opposed to what the proxy would
/// resolve — `get_persisted_defaults`. The dotenv file only; the process
/// environment is deliberately not consulted, because this is the pair the
/// config screen renders back into its own fields.
pub fn persisted_defaults() -> Value {
    let data = load_config_yaml();
    let (config_provider, config_model) = defaults_from_config(&data);
    let env = read_env_file();
    let from_env =
        env.get("DEFAULT_PROVIDER").map(|v| v.trim().to_ascii_lowercase()).unwrap_or_default();
    let mut provider = if from_env.is_empty() { config_provider.clone() } else { from_env };
    if !is_supported_provider(&provider) {
        provider = String::new();
    }
    let mut model = env.get("DEFAULT_MODEL").map(|v| v.trim().to_string()).unwrap_or_default();
    if model.is_empty() && provider == config_provider {
        model = config_model;
    }
    json!({ "provider": provider, "model": model })
}

/// `build_provider_catalog` — the admin/config surface's registry.
///
/// A different body from [`build`] for a different screen: one flat model list
/// per provider with the *reason* it is that list (`source`), plus the two
/// notes the config UI shows when a backend did not answer. Providers are
/// probed together, and only the configured ones are probed at all — serially,
/// one unreachable backend added its whole retry budget to every other row's
/// wait, and the caller renders nothing until this returns.
pub async fn build_admin(http: &reqwest::Client) -> Value {
    let aliases_by_provider = aliases_by_provider();
    let defaults = resolved_defaults();
    let default_provider = defaults["provider"].as_str().unwrap_or("").to_string();
    let default_model = defaults["model"].as_str().unwrap_or("").trim().to_string();

    let ids = provider_ids(Registry::Chat);
    let configured_by_provider: Vec<bool> = ids.iter().map(|p| provider_configured(p)).collect();
    // Every fetcher swallows its own transport errors, so no single provider can
    // fail the join.
    let probed: Vec<(Option<Vec<String>>, Option<&'static str>)> = futures::future::join_all(
        ids.iter().zip(&configured_by_provider).map(|(provider, configured)| async move {
            if *configured {
                discovered_with_source(http, provider).await
            } else {
                (None, None)
            }
        }),
    )
    .await;

    let mut providers = Vec::new();
    for ((provider, configured), (discovered, source)) in
        ids.iter().zip(&configured_by_provider).zip(probed)
    {
        let configured = *configured;
        let aliases = aliases_by_provider.get(*provider).cloned().unwrap_or_default();
        let mut model_source = source.map(str::to_string);
        let mut warning: Option<&str> = None;
        let mut fallback_note: Option<&str> = None;

        let discovered_any = discovered.as_ref().is_some_and(|ids| !ids.is_empty());
        let mut models: Vec<String> = discovered.unwrap_or_default();
        if models.is_empty() && !aliases.is_empty() {
            models = aliases.clone();
            model_source = Some("config_aliases".into());
            fallback_note = Some("Provider catalog unavailable; using config.yaml aliases.");
        }
        if models.is_empty() {
            models = fallback_models(provider, &aliases);
            if !models.is_empty() {
                model_source = Some("ui_fallback_models".into());
                fallback_note =
                    Some("Provider catalog unavailable; using configured UI fallback models.");
            }
        }
        let default_model_for_row = default_model_for_provider(provider);
        if models.is_empty() && !default_model_for_row.is_empty() {
            models = vec![default_model_for_row.to_string()];
            model_source = Some("provider_default".into());
            fallback_note = Some("Provider catalog unavailable; using the provider default model.");
        }
        if configured && !discovered_any && fallback_note.is_none() {
            warning =
                Some("Provider did not return a model catalog; fallback values are being used.");
        }
        let model_source = model_source.unwrap_or_else(|| "unavailable".into());

        let mut selected = default_model_for_row.to_string();
        if default_provider == *provider && !default_model.is_empty() {
            selected = default_model.clone();
        } else if let Some(first) = models.first() {
            selected = first.clone();
        }
        let mut options = vec![selected.clone()];
        options.extend(models);

        providers.push(json!({
            "id": provider,
            "label": provider_label(provider),
            "configured": configured,
            "local": matches!(*provider, "ollama" | "lm_studio"),
            "capabilities": provider_capabilities(provider),
            "models": {
                "options": dedupe(options),
                "selected_model": selected,
                "default_model": selected,
                "source": model_source,
                "warning": warning,
                "fallback_note": fallback_note,
            },
        }));
    }

    json!({
        "persisted_defaults": persisted_defaults(),
        "resolved_defaults": defaults,
        "providers": providers,
    })
}

/// One row of [`build_admin`], addressed by provider id **or by any model alias
/// declared under it** — `get_provider_catalog_entry`.
pub async fn admin_entry(http: &reqwest::Client, provider_or_alias: &str) -> Result<Value, ApiError> {
    let token = provider_or_alias.trim();
    let provider_id = if is_supported_provider(token) {
        token.to_ascii_lowercase()
    } else {
        aliases_by_provider()
            .into_iter()
            .find(|(_, values)| values.iter().any(|v| v == token))
            .map(|(provider, _)| provider)
            .unwrap_or_default()
    };
    if provider_id.is_empty() {
        return Err(ApiError::not_found("Unknown provider"));
    }
    let catalog = build_admin(http).await;
    catalog["providers"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["id"] == json!(provider_id)))
        .cloned()
        .ok_or_else(|| ApiError::not_found("Unknown provider"))
}

/// The catalog's provider filter. Its rejection message names `anthropic`, which
/// `/v1/models` — written earlier, against four providers — still does not.
pub fn parse_filter(query: Option<&str>) -> Result<Option<Vec<String>>, ApiError> {
    let mut repeated: Vec<String> = Vec::new();
    let mut single: Option<String> = None;
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        match key.as_ref() {
            "providers" if !value.trim().is_empty() => {
                repeated.push(value.trim().to_ascii_lowercase())
            }
            "provider" if single.is_none() => single = Some(value.trim().to_ascii_lowercase()),
            _ => {}
        }
    }

    let single = single.unwrap_or_default();
    if !single.is_empty() {
        if !is_supported_provider(&single) {
            return Err(ApiError::bad_request(
                "query provider must be ollama, lm_studio, gemini, aimlapi, anthropic, or omitted",
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

    let effective = resolved_defaults()["provider"].as_str().unwrap_or("").to_string();
    let provider =
        if is_supported_provider(&effective) { effective } else { "lm_studio".to_string() };
    Ok(Some(vec![provider]))
}

/// `?live=false`, the way FastAPI reads a `bool` query parameter.
pub fn query_flag(query: Option<&str>, name: &str, default: bool) -> bool {
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        if key == name {
            return matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            );
        }
    }
    default
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_order_is_reported_per_provider() {
        assert_eq!(discovery("ollama")["primary_source"], json!("ollama_tags"));
        // LM Studio has an extra rung: its own registry, when the OpenAI list is empty.
        assert_eq!(
            discovery("lm_studio")["fallback_sources"][0],
            json!("lm_studio_native_models")
        );
        assert_eq!(discovery("gemini")["primary_source"], json!("upstream_models"));
    }

    #[test]
    fn only_gemini_declares_no_tools() {
        assert_eq!(provider_capabilities("gemini")["tools"], json!(false));
        assert_eq!(provider_capabilities("ollama")["tools"], json!(true));
        assert_eq!(provider_capabilities("ollama")["json_mode"], json!(true));
    }

    #[test]
    fn flags_and_filters_read_the_query_string() {
        assert!(query_flag(Some("live=true"), "live", false));
        assert!(!query_flag(Some("live=false"), "live", true));
        assert!(!query_flag(Some("live=0"), "live", true));
        assert!(query_flag(None, "live", true), "absent keeps the default");

        // The message names anthropic, unlike the one on /v1/models.
        let err = parse_filter(Some("provider=banana")).unwrap_err();
        assert!(err.message.contains("anthropic"), "{}", err.message);
        assert!(parse_filter(Some("providers=all")).unwrap().is_none());
    }

    #[test]
    fn duplicate_aliases_collapse_in_order() {
        assert_eq!(
            dedupe(vec!["b".into(), "a".into(), " b ".into(), "".into()]),
            vec!["b".to_string(), "a".to_string()]
        );
    }
}
