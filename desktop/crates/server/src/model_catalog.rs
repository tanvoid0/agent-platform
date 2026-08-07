//! Live model lists from the local backends, and the background cache over them.
//! Port of `app/llm_proxy/services/model_catalog_cache.py`.
//!
//! `/v1/health` is a liveness probe, so it must not block on a catalog fetch —
//! it reads whatever this last saw, plus how old that is. The refresh loop
//! sleeps *before* its first pass, exactly as Python's does, so a freshly started
//! server reports `model_present: null` rather than a wrong answer.
//!
//! The fetchers are shared with `/v1/models`, which wants the same lists live.

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::llm_config::{lm_studio_api_base, lm_studio_api_key, ollama_api_base};
use crate::upstream_http::send_with_retry;

const REFRESH_INTERVAL: Duration = Duration::from_secs(30);

/// Python gives the same fetch a different budget depending on who is waiting:
/// the background cache and `/v1/models` take 8s, the local-backend coercion and
/// `/v1/catalog` 12s, a cloud catalog 15s, Gemini 20s. Callers pass their own,
/// because a cloud provider that answers in 10s is reachable there and would be
/// a timeout here.
pub const QUICK_TIMEOUT: Duration = Duration::from_secs(8);
pub const LOCAL_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Default)]
struct Snapshot {
    ollama_tags: Vec<String>,
    ollama_updated_at: Option<Instant>,
    lm_studio_models: Vec<String>,
    lm_studio_updated_at: Option<Instant>,
}

#[derive(Default)]
pub struct CatalogCache {
    inner: RwLock<Snapshot>,
}

impl CatalogCache {
    /// Refresh both catalogs forever. A failed pass leaves the previous list in
    /// place: a backend that just restarted should not empty the catalog.
    pub fn spawn_refresh(self: Arc<Self>, http: reqwest::Client) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(REFRESH_INTERVAL).await;
                let (tags, models) = tokio::join!(
                    fetch_ollama_tags(&http, QUICK_TIMEOUT),
                    fetch_lm_studio_models(&http, QUICK_TIMEOUT)
                );
                let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
                if !tags.is_empty() {
                    inner.ollama_tags = tags;
                    inner.ollama_updated_at = Some(Instant::now());
                }
                if !models.is_empty() {
                    inner.lm_studio_models = models;
                    inner.lm_studio_updated_at = Some(Instant::now());
                }
            }
        });
    }

    pub fn ollama_tags(&self) -> Vec<String> {
        self.read().ollama_tags.clone()
    }

    pub fn lm_studio_models(&self) -> Vec<String> {
        self.read().lm_studio_models.clone()
    }

    /// Seconds since the last successful fetch; `0` when there has never been one.
    pub fn ollama_tag_age_sec(&self) -> f64 {
        age(self.read().ollama_updated_at)
    }

    pub fn lm_studio_models_age_sec(&self) -> f64 {
        age(self.read().lm_studio_updated_at)
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Snapshot> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }
}

fn age(at: Option<Instant>) -> f64 {
    at.map_or(0.0, |t| t.elapsed().as_secs_f64())
}

// ---------------------------------------------------------------------------
// Fetchers
// ---------------------------------------------------------------------------

/// Ollama's own shape: `{"models": [{"name": "..."}]}`. Empty on any failure —
/// every caller treats "no catalog" and "could not ask" the same way.
pub async fn fetch_ollama_tags(http: &reqwest::Client, timeout: Duration) -> Vec<String> {
    let base = ollama_api_base();
    if base.is_empty() {
        return Vec::new();
    }
    let url = format!("{}/api/tags", base.trim_end_matches('/'));
    let Ok(resp) =
        send_with_retry("catalog_ollama_tags", false, || http.get(&url).timeout(timeout)).await
    else {
        return Vec::new();
    };
    if !resp.is_ok() {
        return Vec::new();
    }
    string_field_list(resp.json().as_ref(), "models", "name")
}

pub async fn fetch_lm_studio_models(http: &reqwest::Client, timeout: Duration) -> Vec<String> {
    let base = lm_studio_api_base();
    if base.is_empty() {
        return Vec::new();
    }
    let url = format!("{}/v1/models", base.trim_end_matches('/'));
    fetch_openai_model_ids(http, &url, &lm_studio_headers(), "catalog_lm_studio_models", timeout)
        .await
}

/// Bearer only when LM Studio is configured to require one.
pub fn lm_studio_headers() -> Vec<(String, String)> {
    let key = lm_studio_api_key();
    if key.is_empty() {
        Vec::new()
    } else {
        vec![("Authorization".into(), format!("Bearer {key}"))]
    }
}

/// The OpenAI list shape: `{"data": [{"id": "..."}]}`, which LM Studio, AIMLAPI
/// and Anthropic all answer with.
pub async fn fetch_openai_model_ids(
    http: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
    context: &str,
    timeout: Duration,
) -> Vec<String> {
    let Ok(resp) = send_with_retry(context, false, || {
        let mut req = http.get(url).timeout(timeout);
        for (name, value) in headers {
            req = req.header(name, value);
        }
        req
    })
    .await
    else {
        return Vec::new();
    };
    if !resp.is_ok() {
        return Vec::new();
    }
    string_field_list(resp.json().as_ref(), "data", "id")
}

/// LM Studio's native registry, used only when its OpenAI list comes back empty.
/// Keyed entries, filtered to `type: llm`.
pub async fn fetch_lm_studio_native_keys(
    http: &reqwest::Client,
    base: &str,
    timeout: Duration,
) -> Vec<String> {
    let url = format!("{}/api/v1/models", base.trim_end_matches('/'));
    let headers = lm_studio_headers();
    let Ok(resp) = send_with_retry("local_backends_lm_studio_native", false, || {
        let mut req = http.get(&url).timeout(timeout);
        for (name, value) in &headers {
            req = req.header(name, value);
        }
        req
    })
    .await
    else {
        return Vec::new();
    };
    if !resp.is_ok() {
        return Vec::new();
    }
    resp.json()
        .as_ref()
        .and_then(|v| v.get("models"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| {
                    item.get("type").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref()
                        == Some("llm")
                })
                .filter_map(|item| item.get("key").and_then(Value::as_str))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// `POST /api/v1/models/load` — LM Studio 0.4+ pulls the model into memory.
/// Best effort: a backend that does not support it just answers 404.
async fn lm_studio_load_model(http: &reqwest::Client, base: &str, model: &str) {
    let url = format!("{}/api/v1/models/load", base.trim_end_matches('/'));
    let headers = lm_studio_headers();
    let timeout = Duration::from_secs(
        crate::env_opt("LM_STUDIO_LOAD_TIMEOUT_SEC")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(600)
            .max(30),
    );
    let body = serde_json::json!({ "model": model });
    let _ = send_with_retry("lm_studio_models_load", true, || {
        let mut req = http.post(&url).json(&body).timeout(timeout);
        for (name, value) in &headers {
            req = req.header(name, value);
        }
        req
    })
    .await;
}

fn env_flag(name: &str, default: bool) -> bool {
    match crate::env_opt(name) {
        None => default,
        Some(v) => matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
    }
}

/// Whether an Ollama tag list contains `model`, comparing bare names either side
/// of the `:` as well as the full tag.
pub fn ollama_tag_matches(tags: &[String], model: &str) -> bool {
    let base = |s: &str| s.split(':').next().unwrap_or("").to_string();
    !model.trim().is_empty() && tags.iter().any(|tag| tag == model || base(tag) == base(model))
}

/// If the requested model is not in the local catalog, use the first one that is.
///
/// A local backend that has never pulled the configured default would otherwise
/// fail every request; answering with *a* model the user has is the friendlier
/// wrong answer, and it is logged. Off with `LOCAL_LLM_MODEL_FALLBACK=0`.
pub async fn coerce_local_model_if_needed(
    http: &reqwest::Client,
    provider: &str,
    model: &str,
) -> String {
    if !env_flag("LOCAL_LLM_MODEL_FALLBACK", true) {
        return model.to_string();
    }

    if provider == "ollama" {
        let base = ollama_api_base();
        if base.is_empty() {
            return model.to_string();
        }
        let tags = fetch_ollama_tags(http, LOCAL_TIMEOUT).await;
        if tags.is_empty() || ollama_tag_matches(&tags, model) {
            return model.to_string();
        }
        eprintln!(
            "[agent-platformd] Ollama model {model:?} not in local tags; falling back to {:?}",
            tags[0]
        );
        return tags[0].clone();
    }

    if provider == "lm_studio" {
        let base = lm_studio_api_base();
        if base.is_empty() {
            return model.to_string();
        }
        let mut ids = fetch_lm_studio_models(http, LOCAL_TIMEOUT).await;
        if ids.is_empty() {
            ids = fetch_lm_studio_native_keys(http, &base, LOCAL_TIMEOUT).await;
        }
        if ids.is_empty() {
            return model.to_string();
        }
        let want = model.trim();
        if ids.iter().any(|id| id == want) {
            if env_flag("LM_STUDIO_TRY_LOAD_MODEL", true)
                && env_flag("LM_STUDIO_PRELOAD_MATCHED_MODEL", false)
            {
                lm_studio_load_model(http, &base, want).await;
            }
            return want.to_string();
        }
        eprintln!(
            "[agent-platformd] LM Studio model {model:?} not listed; falling back to {:?}",
            ids[0]
        );
        if env_flag("LM_STUDIO_TRY_LOAD_MODEL", true) {
            lm_studio_load_model(http, &base, &ids[0]).await;
        }
        return ids[0].clone();
    }

    model.to_string()
}

/// `body[list_key][*][field]`, keeping the non-empty strings in order.
fn string_field_list(body: Option<&Value>, list_key: &str, field: &str) -> Vec<String> {
    body.and_then(|v| v.get(list_key))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get(field).and_then(Value::as_str))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn both_upstream_shapes_parse_and_bad_ones_are_empty() {
        let ollama = json!({"models": [{"name": "llama3:8b"}, {"name": "  "}, {"nope": 1}]});
        assert_eq!(string_field_list(Some(&ollama), "models", "name"), vec!["llama3:8b"]);

        let openai = json!({"data": [{"id": "gpt-4.1-mini"}, {"id": "qwen3"}]});
        assert_eq!(
            string_field_list(Some(&openai), "data", "id"),
            vec!["gpt-4.1-mini", "qwen3"]
        );

        assert!(string_field_list(None, "data", "id").is_empty());
        assert!(string_field_list(Some(&json!({"data": "nope"})), "data", "id").is_empty());
    }

    #[test]
    fn age_is_zero_until_the_first_successful_fetch() {
        let cache = CatalogCache::default();
        assert_eq!(cache.ollama_tag_age_sec(), 0.0);
        assert!(cache.ollama_tags().is_empty());
        assert_eq!(cache.lm_studio_models_age_sec(), 0.0);
    }
}
