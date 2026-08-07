//! Per-model capability discovery and the request guards over it. Port of
//! `app/llm_proxy/services/model_capabilities.py`.
//!
//! Ollama answers `POST /api/show` with authoritative flags (`tools`, `vision`,
//! `completion`, `embedding`). Everything else is inferred from the model name.
//! Results are cached in memory and on disk and treated as **sticky**: once a
//! model has been seen to support something it is never downgraded, because a
//! capability does not disappear at runtime but a probe can fail.
//!
//! The point is to refuse an unsupported request with a `501` naming the reason,
//! rather than forwarding a tool call into a model that will answer prose.
//!
//! ponytail: `model_capabilities.json` now has two writers, this and Python.
//! Each write is a temp-file rename so the file is never torn; the only loss
//! from an interleave is one freshly probed entry, which the next probe rewrites.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use axum::http::StatusCode;
use serde_json::{json, Map, Value};

use crate::error::ApiError;
use crate::llm_config::{config_dir, modality_map, ollama_api_base, provider_supports, Modality};
use crate::upstream_http::send_with_retry;

pub const CAPABILITY_KEYS: [&str; 6] =
    ["chat", "tools", "vision_input", "embeddings", "image_generation", "streaming"];

/// A probe result good enough that it is never re-run.
const AUTHORITATIVE: [&str; 2] = ["ollama_show", "cached"];

fn cache() -> &'static Mutex<HashMap<String, Map<String, Value>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Map<String, Value>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(load_disk()))
}

fn cache_path() -> PathBuf {
    config_dir().join("model_capabilities.json")
}

fn cache_key(provider: &str, model: &str) -> String {
    format!("{}::{}", provider.trim().to_ascii_lowercase(), model.trim().to_ascii_lowercase())
}

// ---------------------------------------------------------------------------
// Capability shapes
// ---------------------------------------------------------------------------

/// What a provider can do before anything model-specific is known.
pub fn provider_default_capabilities(provider: &str) -> Map<String, Value> {
    let modalities = modality_map(provider);
    let flag = |key: &str| modalities.get(key).and_then(Value::as_bool).unwrap_or(false);
    let mut caps = Map::new();
    caps.insert("chat".into(), json!(flag("chat")));
    // Gemini's OpenAI-compatible surface does not take tool definitions.
    caps.insert(
        "tools".into(),
        json!(provider_supports(provider, Modality::Chat) && provider != "gemini"),
    );
    caps.insert("vision_input".into(), json!(flag("vision_input")));
    caps.insert("embeddings".into(), json!(flag("embeddings")));
    caps.insert("image_generation".into(), json!(flag("image_generation")));
    caps.insert("streaming".into(), json!(true));
    caps.insert("probe_source".into(), json!("provider_default"));
    caps
}

/// Ollama's `capabilities` array, mapped onto our keys.
fn normalize_ollama_capabilities(raw: &[Value]) -> Map<String, Value> {
    let mut caps: Map<String, Value> =
        CAPABILITY_KEYS.iter().map(|k| ((*k).to_string(), json!(false))).collect();
    caps.insert("streaming".into(), json!(true));
    caps.insert("probe_source".into(), json!("ollama_show"));

    for item in raw {
        let Some(name) = item.as_str() else { continue };
        let mapped = match name.trim().to_ascii_lowercase().as_str() {
            "completion" => "chat",
            "tools" => "tools",
            "vision" => "vision_input",
            "embedding" | "embeddings" => "embeddings",
            _ => continue,
        };
        caps.insert(mapped.into(), json!(true));
    }

    // A model that claims nothing we recognise still answers chat.
    let any = ["chat", "tools", "vision_input", "embeddings"]
        .iter()
        .any(|k| caps.get(*k).and_then(Value::as_bool).unwrap_or(false));
    if !any {
        caps.insert("chat".into(), json!(true));
    }
    caps
}

const TOOL_HINTS: [&str; 12] = [
    "qwen3-coder",
    "qwen2.5-coder",
    "qwen-coder",
    "deepseek-coder",
    "codestral",
    "devstral",
    "mistral-nemo",
    "llama3.1",
    "llama3.2",
    "llama3.3",
    "functionary",
    "hermes-3",
];
const TOOL_HINTS_EXTRA: [&str; 2] = ["firefunction", "command-r"];
// `gemma3.*vision` in Python's pattern is dropped: the bare `vision` alternative
// already matches every id where it would, given the same delimiter rules.
const VISION_HINTS: [&str; 8] = [
    "llava",
    "bakllava",
    "moondream",
    "minicpm-v",
    "vision",
    "llama3.2-vision",
    "qwen2-vl",
    "qwen-vl",
];
const EMBED_HINTS: [&str; 6] =
    ["nomic-embed", "mxbai-embed", "bge-", "embed", "embedding", "text-embedding"];

/// `(?:^|[/_-])needle(?:[:\-_/]|$)`, without a regex engine — the patterns are
/// alternations of literals, so the only work is the delimiter check.
fn name_hints_match(name: &str, needles: &[&str]) -> bool {
    let bytes = name.as_bytes();
    needles.iter().any(|needle| {
        name.match_indices(needle).any(|(start, matched)| {
            let before_ok = start == 0 || matches!(bytes[start - 1], b'/' | b'_' | b'-');
            let end = start + matched.len();
            let after_ok = end == bytes.len() || matches!(bytes[end], b':' | b'-' | b'_' | b'/');
            before_ok && after_ok
        })
    })
}

fn infer_capabilities_from_model_name(model_id: &str, provider: &str) -> Map<String, Value> {
    let mut caps = provider_default_capabilities(provider);
    caps.insert("probe_source".into(), json!("heuristic"));
    let name = model_id.trim().to_ascii_lowercase();
    if name_hints_match(&name, &TOOL_HINTS) || name_hints_match(&name, &TOOL_HINTS_EXTRA) {
        caps.insert("tools".into(), json!(true));
    }
    if name_hints_match(&name, &VISION_HINTS) {
        caps.insert("vision_input".into(), json!(true));
    }
    if name_hints_match(&name, &EMBED_HINTS) {
        caps.insert("embeddings".into(), json!(true));
    }
    caps
}

fn merge_capabilities(base: &Map<String, Value>, over: &Map<String, Value>) -> Map<String, Value> {
    let mut merged = base.clone();
    for key in CAPABILITY_KEYS {
        if let Some(value) = over.get(key) {
            merged.insert(key.into(), json!(truthy(value)));
        }
    }
    if let Some(source) = over.get("probe_source").and_then(Value::as_str).filter(|s| !s.is_empty())
    {
        merged.insert("probe_source".into(), json!(source));
    }
    merged
}

/// Confirmed capabilities are never lost: a positive stays positive.
fn merge_capabilities_sticky(
    previous: Option<&Map<String, Value>>,
    fresh: &Map<String, Value>,
) -> Map<String, Value> {
    let Some(previous) = previous.filter(|p| !p.is_empty()) else {
        return fresh.clone();
    };
    let mut merged = merge_capabilities(fresh, previous);
    for key in CAPABILITY_KEYS {
        if previous.get(key).is_some_and(truthy) {
            merged.insert(key.into(), json!(true));
        }
    }
    let authoritative = |m: &Map<String, Value>| {
        m.get("probe_source").and_then(Value::as_str).is_some_and(|s| AUTHORITATIVE.contains(&s))
    };
    if authoritative(previous) {
        merged.insert("probe_source".into(), previous["probe_source"].clone());
    } else if authoritative(fresh) {
        merged.insert("probe_source".into(), fresh["probe_source"].clone());
    }
    if let Some(at) = previous.get("probed_at").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        merged.insert("probed_at".into(), json!(at));
    }
    merged
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Null => false,
        _ => true,
    }
}

// ---------------------------------------------------------------------------
// Disk cache
// ---------------------------------------------------------------------------

fn load_disk() -> HashMap<String, Map<String, Value>> {
    let Ok(raw) = std::fs::read_to_string(cache_path()) else {
        return HashMap::new();
    };
    let Ok(Value::Object(payload)) = serde_json::from_str::<Value>(&raw) else {
        return HashMap::new();
    };
    let Some(entries) = payload.get("entries").and_then(Value::as_object) else {
        return HashMap::new();
    };
    entries
        .iter()
        .filter_map(|(key, record)| {
            let caps = record.get("capabilities")?.as_object()?;
            let mut out = caps.clone();
            if let Some(at) = record.get("probed_at").and_then(Value::as_str).filter(|s| !s.is_empty())
            {
                out.insert("probed_at".into(), json!(at));
            }
            if !out.get("probe_source").is_some_and(truthy) {
                out.insert("probe_source".into(), json!("cached"));
            }
            Some((key.clone(), out))
        })
        .collect()
}

/// Read-modify-write, then rename over the old file so a reader never sees a
/// half-written cache — including Python, which is still writing this file too.
fn persist(key: &str, caps: &Map<String, Value>) {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut entries = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.get("entries").and_then(Value::as_object).cloned())
        .unwrap_or_default();

    let stored: Map<String, Value> = CAPABILITY_KEYS
        .iter()
        .filter_map(|k| caps.get(*k).map(|v| ((*k).to_string(), v.clone())))
        .chain(
            caps.get("probe_source")
                .filter(|s| truthy(s))
                .map(|s| ("probe_source".to_string(), s.clone())),
        )
        .collect();
    entries.insert(
        key.to_string(),
        json!({
            "capabilities": stored,
            "probed_at": caps.get("probed_at").cloned().unwrap_or_else(|| json!(now_iso())),
        }),
    );

    let body = serde_json::to_string_pretty(&json!({ "version": 1, "entries": entries }));
    let Ok(body) = body else { return };
    let temp = path.with_extension("json.tmp");
    if std::fs::write(&temp, body).is_ok() {
        let _ = std::fs::rename(&temp, &path);
    }
}

fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

fn should_skip_probe(cached: Option<&Map<String, Value>>) -> bool {
    let Some(cached) = cached.filter(|c| !c.is_empty()) else { return false };
    if cached.get("probe_source").and_then(Value::as_str).is_some_and(|s| AUTHORITATIVE.contains(&s))
    {
        return true;
    }
    cached.get("probed_at").and_then(Value::as_str).is_some_and(|s| !s.is_empty())
}

async fn fetch_ollama_show(http: &reqwest::Client, base: &str, model: &str) -> Option<Map<String, Value>> {
    let url = format!("{}/api/show", base.trim_end_matches('/'));
    let body = json!({ "name": model });
    let response = send_with_retry("v1_catalog_ollama_show", false, || {
        http.post(&url).json(&body).timeout(std::time::Duration::from_secs(10))
    })
    .await
    .ok()?;
    if !response.is_ok() {
        return None;
    }
    let raw = response.json()?;
    let list = raw.get("capabilities")?.as_array()?.clone();
    Some(normalize_ollama_capabilities(&list))
}

/// Capabilities for a provider/model pair, probing Ollama when it can.
pub async fn resolve_model_capabilities(
    http: &reqwest::Client,
    provider: &str,
    model_id: &str,
    probe: bool,
) -> Map<String, Value> {
    let provider = provider.trim().to_ascii_lowercase();
    let model = model_id.trim().to_string();
    if provider.is_empty() || model.is_empty() {
        let fallback = if provider.is_empty() { "ollama" } else { &provider };
        return provider_default_capabilities(fallback);
    }

    let key = cache_key(&provider, &model);
    {
        let cache = cache().lock().unwrap_or_else(|e| e.into_inner());
        if should_skip_probe(cache.get(&key)) {
            return cache[&key].clone();
        }
    }

    let fresh = if probe && provider == "ollama" {
        let base = ollama_api_base();
        match fetch_ollama_show(http, &base, &model).await.filter(|_| !base.is_empty()) {
            Some(shown) => merge_capabilities(&provider_default_capabilities(&provider), &shown),
            None => infer_capabilities_from_model_name(&model, &provider),
        }
    } else {
        infer_capabilities_from_model_name(&model, &provider)
    };

    let mut caps = {
        let cache = cache().lock().unwrap_or_else(|e| e.into_inner());
        merge_capabilities_sticky(cache.get(&key), &fresh)
    };
    if !caps.get("probed_at").is_some_and(truthy) {
        caps.insert("probed_at".into(), json!(now_iso()));
    }
    cache().lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone(), caps.clone());
    persist(&key, &caps);
    caps
}

// ---------------------------------------------------------------------------
// Request guards
// ---------------------------------------------------------------------------

pub fn request_uses_tools(body: &Map<String, Value>) -> bool {
    if body.get("tools").and_then(Value::as_array).is_some_and(|t| !t.is_empty()) {
        return true;
    }
    match body.get("tool_choice") {
        None | Some(Value::Null) => false,
        Some(Value::String(choice)) => !matches!(choice.trim().to_ascii_lowercase().as_str(), "" | "none"),
        Some(_) => true,
    }
}

pub fn messages_contain_vision(messages: Option<&Value>) -> bool {
    let Some(messages) = messages.and_then(Value::as_array) else { return false };
    messages.iter().any(|message| match message.get("content") {
        Some(Value::Array(parts)) => parts
            .iter()
            .any(|part| part.get("type").and_then(Value::as_str) == Some("image_url")),
        Some(Value::String(text)) => text.contains("data:image/"),
        _ => false,
    })
}

fn capability_label(capability: &str) -> &'static str {
    match capability {
        "tools" => "tool / function calling",
        "vision_input" => "vision (image input)",
        "embeddings" => "embeddings",
        "image_generation" => "image generation",
        _ => "chat",
    }
}

fn require_model_capability(
    provider: &str,
    model: &str,
    caps: &Map<String, Value>,
    capability: &str,
) -> Result<(), ApiError> {
    if caps.get(capability).is_some_and(truthy) {
        return Ok(());
    }
    let label = capability_label(capability);
    let source = caps.get("probe_source").and_then(Value::as_str).unwrap_or("unknown");
    let reported: Map<String, Value> = CAPABILITY_KEYS
        .iter()
        .map(|k| ((*k).to_string(), caps.get(*k).cloned().unwrap_or(Value::Null)))
        .collect();

    Err(ApiError::coded(
        StatusCode::NOT_IMPLEMENTED,
        "capability_unavailable",
        format!(
            "Model '{model}' on {provider} does not support {label}. \
             Choose a model that supports this operation (catalog probe_source={source}). \
             For Ollama coding models like qwen3-coder:30b, run `ollama show <model>` and \
             confirm Capabilities includes 'tools' (requires Ollama 0.12+ and RENDERER/PARSER qwen3-coder in the Modelfile)."
        ),
    )
    .with_extra(json!({
        "capability": capability,
        "provider": provider,
        "model": model,
        "model_capabilities": reported,
        "probe_source": source,
    })))
}

/// Validate a chat body against the model's resolved capabilities.
pub async fn ensure_chat_request_supported(
    http: &reqwest::Client,
    provider: &str,
    model: &str,
    body: &Map<String, Value>,
) -> Result<(), ApiError> {
    let caps = resolve_model_capabilities(http, provider, model, true).await;
    if request_uses_tools(body) {
        require_model_capability(provider, model, &caps, "tools")?;
    }
    if messages_contain_vision(body.get("messages")) {
        require_model_capability(provider, model, &caps, "vision_input")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_flags_map_onto_our_keys() {
        let caps = normalize_ollama_capabilities(&[json!("completion"), json!("tools")]);
        assert!(truthy(&caps["chat"]));
        assert!(truthy(&caps["tools"]));
        assert!(!truthy(&caps["vision_input"]));
        assert_eq!(caps["probe_source"], json!("ollama_show"));

        // Nothing recognised still means it answers chat.
        let caps = normalize_ollama_capabilities(&[json!("thinking")]);
        assert!(truthy(&caps["chat"]));
    }

    #[test]
    fn name_hints_respect_delimiters() {
        assert!(name_hints_match("qwen3-coder:30b", &TOOL_HINTS));
        assert!(name_hints_match("hf.co/qwen3-coder", &TOOL_HINTS));
        assert!(!name_hints_match("myqwen3-coderx", &TOOL_HINTS), "needs a delimiter each side");
        assert!(name_hints_match("llava:13b", &VISION_HINTS));
        assert!(name_hints_match("nomic-embed-text", &EMBED_HINTS));
        assert!(!name_hints_match("mistral:7b", &TOOL_HINTS));
    }

    #[test]
    fn a_confirmed_capability_is_never_downgraded() {
        let previous = normalize_ollama_capabilities(&[json!("completion"), json!("tools")]);
        // A later probe answers without `tools` — a model re-created from a
        // Modelfile that dropped its parser, or a truncated /api/show.
        let fresh = normalize_ollama_capabilities(&[json!("completion")]);
        assert!(!truthy(&fresh["tools"]));

        let merged = merge_capabilities_sticky(Some(&previous), &fresh);
        assert!(truthy(&merged["tools"]), "sticky: a confirmed capability stays");

        // A heuristic guess never overwrites what a real probe established.
        let guessed = infer_capabilities_from_model_name("mystery-model", "ollama");
        assert_eq!(guessed["probe_source"], json!("heuristic"));
        let merged = merge_capabilities_sticky(Some(&previous), &guessed);
        assert_eq!(merged["probe_source"], json!("ollama_show"), "authoritative source wins");
    }

    #[test]
    fn guards_read_the_shapes_clients_actually_send() {
        let body = |v: Value| v.as_object().unwrap().clone();

        assert!(request_uses_tools(&body(json!({"tools": [{"type": "function"}]}))));
        assert!(!request_uses_tools(&body(json!({"tools": []}))));
        assert!(!request_uses_tools(&body(json!({"tool_choice": "none"}))));
        assert!(request_uses_tools(&body(json!({"tool_choice": "auto"}))));
        assert!(request_uses_tools(&body(json!({"tool_choice": {"type": "function"}}))));
        assert!(!request_uses_tools(&body(json!({}))));

        let vision = json!([{"role": "user", "content": [{"type": "image_url"}]}]);
        assert!(messages_contain_vision(Some(&vision)));
        let inline = json!([{"role": "user", "content": "look: data:image/png;base64,x"}]);
        assert!(messages_contain_vision(Some(&inline)));
        assert!(!messages_contain_vision(Some(&json!([{"role": "user", "content": "hi"}]))));
        assert!(!messages_contain_vision(None));
    }

    #[test]
    fn gemini_never_advertises_tools() {
        assert!(!truthy(&provider_default_capabilities("gemini")["tools"]));
        assert!(truthy(&provider_default_capabilities("ollama")["tools"]));
        assert!(truthy(&provider_default_capabilities("lm_studio")["embeddings"]));
        assert!(!truthy(&provider_default_capabilities("anthropic")["embeddings"]));
    }
}
