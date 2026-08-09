//! Provider configuration, cached config-file reads, and the capability matrix.
//!
//! Port of `app/llm_proxy/core/{provider_config,config_cache,capabilities}.py`
//! and the two one-backend registries in `services/{image,speech}_backends.py` —
//! step 1 of the `llm_proxy/` migration (see plan.md). Read-only: this module
//! answers "which providers exist, are they configured, and what can they do".
//! The `/v1/*` handlers land on top of it.
//!
//! Python keeps three registries because a chat provider implies an
//! OpenAI-compatible chat/embeddings surface while an image or speech backend
//! does not. That distinction survives here as `Registry` on one table rather
//! than three tables: every caller either filters by registry or asks a question
//! the whole table answers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use axum::http::StatusCode;
use serde_json::{json, Map, Value};

use crate::error::ApiError;
use crate::env_opt;

// ---------------------------------------------------------------------------
// Capability (modality) contract
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modality {
    Chat,
    VisionInput,
    Embeddings,
    ImageGeneration,
    Speech,
}

/// Declaration order is the `modalities` array `GET /v1/capabilities` returns.
pub const MODALITIES: [Modality; 5] = [
    Modality::Chat,
    Modality::VisionInput,
    Modality::Embeddings,
    Modality::ImageGeneration,
    Modality::Speech,
];

impl Modality {
    pub fn as_str(self) -> &'static str {
        match self {
            Modality::Chat => "chat",
            Modality::VisionInput => "vision_input",
            Modality::Embeddings => "embeddings",
            Modality::ImageGeneration => "image_generation",
            Modality::Speech => "speech",
        }
    }
}

// ---------------------------------------------------------------------------
// Provider registry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Registry {
    Chat,
    Image,
    Speech,
}

pub struct ProviderSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub registry: Registry,
    /// Canonical first model when `DEFAULT_MODEL` and config are both empty.
    /// Empty for Claude, whose ids rotate too fast to pin.
    pub default_model: &'static str,
    pub modalities: &'static [Modality],
    /// `PROVIDER_LOCAL_SORT_ORDER`; 99 for the ids that map omits.
    pub sort_order: u8,
    /// The env var holding this provider's credential, `None` for a local
    /// backend that wants none. Whatever is named here is a secret by
    /// definition: `llm_admin` masks it and `dotenv` refuses to take it from the
    /// committed YAML, and the test at the bottom of `llm_admin` is what makes
    /// those two follow this column instead of a hand-written copy of it.
    pub api_key_env: Option<&'static str>,
    /// The env var overriding this provider's base URL, `None` for a hosted one
    /// that is only ever reached at its own address.
    pub base_url_env: Option<&'static str>,
}

use Modality::{Chat, Embeddings, ImageGeneration, Speech, VisionInput};

/// Declaration order is `SUPPORTED_PROVIDER_IDS ++ IMAGE_PROVIDER_IDS ++
/// SPEECH_PROVIDER_IDS`, which is what Python's capability router sorts — stably
/// — by `sort_order`, so the two share a preference order for free.
pub const PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        id: "ollama",
        api_key_env: None,
        base_url_env: Some("OLLAMA_API_BASE"),
        label: "Ollama",
        registry: Registry::Chat,
        default_model: "llama3",
        modalities: &[Chat, VisionInput],
        sort_order: 0,
    },
    ProviderSpec {
        id: "lm_studio",
        api_key_env: Some("LM_STUDIO_API_KEY"),
        base_url_env: Some("LM_STUDIO_API_BASE"),
        label: "LM Studio",
        registry: Registry::Chat,
        default_model: "google/gemma-4-e4b",
        modalities: &[Chat, VisionInput, Embeddings],
        sort_order: 1,
    },
    ProviderSpec {
        id: "aimlapi",
        api_key_env: Some("AIMLAPI_API_KEY"),
        base_url_env: Some("AIMLAPI_OPENAI_BASE"),
        label: "AIMLAPI",
        registry: Registry::Chat,
        default_model: "openai/gpt-4.1-mini",
        modalities: &[Chat, Embeddings],
        sort_order: 2,
    },
    ProviderSpec {
        // Claude's OpenAI-compatible surface has no embeddings endpoint; it does
        // take image inputs.
        id: "anthropic",
        api_key_env: Some("ANTHROPIC_API_KEY"),
        base_url_env: Some("ANTHROPIC_OPENAI_BASE"),
        label: "Claude",
        registry: Registry::Chat,
        default_model: "",
        modalities: &[Chat, VisionInput],
        sort_order: 3,
    },
    ProviderSpec {
        id: "gemini",
        api_key_env: Some("GEMINI_API_KEY"),
        base_url_env: Some("GEMINI_OPENAI_BASE"),
        label: "Cloud",
        registry: Registry::Chat,
        default_model: "gemini-2.0-flash",
        modalities: &[Chat, VisionInput, Embeddings],
        sort_order: 4,
    },
    ProviderSpec {
        id: "image_local",
        api_key_env: None,
        base_url_env: Some("IMAGE_API_BASE"),
        label: "Image (local)",
        registry: Registry::Image,
        default_model: DEFAULT_IMAGE_MODEL,
        modalities: &[ImageGeneration],
        sort_order: 99,
    },
    ProviderSpec {
        id: "speech_local",
        api_key_env: Some("SPEECH_API_KEY"),
        base_url_env: Some("SPEECH_API_BASE"),
        label: "Speech (local)",
        registry: Registry::Speech,
        default_model: DEFAULT_SPEECH_MODEL,
        modalities: &[Speech],
        sort_order: 99,
    },
];

pub fn spec(provider: &str) -> Option<&'static ProviderSpec> {
    let name = provider.trim().to_ascii_lowercase();
    PROVIDERS.iter().find(|p| p.id == name)
}

fn spec_in(provider: &str, registry: Registry) -> Option<&'static ProviderSpec> {
    spec(provider).filter(|p| p.registry == registry)
}

/// Ids of one registry, in declaration order.
pub fn provider_ids(registry: Registry) -> Vec<&'static str> {
    PROVIDERS.iter().filter(|p| p.registry == registry).map(|p| p.id).collect()
}

/// A chat provider the proxy knows how to route (`is_supported_provider`).
/// Image and speech backends are deliberately *not* supported provider ids.
pub fn is_supported_provider(provider: &str) -> bool {
    spec_in(provider, Registry::Chat).is_some()
}

pub fn provider_label(provider: &str) -> String {
    spec(provider).map(|p| p.label.to_string()).unwrap_or_else(|| provider.to_string())
}

pub fn default_model_for_provider(provider: &str) -> &'static str {
    spec_in(provider, Registry::Chat).map_or("google/gemma-4-e4b", |p| p.default_model)
}

// ---------------------------------------------------------------------------
// Configured checks
// ---------------------------------------------------------------------------

/// Standard loopback URLs when env and dotenv omit these keys. They are also
/// what `LOCAL_LLM_AUTO_DISCOVER` probes and then sets, so the discovery
/// override Python keeps in a module global can only ever restate this default —
/// which is why there is no runtime-override tier here.
pub const DEFAULT_OLLAMA_BASE: &str = "http://127.0.0.1:11434";
pub const DEFAULT_LM_STUDIO_BASE: &str = "http://127.0.0.1:1234";
pub const DEFAULT_IMAGE_MODEL: &str = "flux.1-schnell";
pub const DEFAULT_SPEECH_MODEL: &str = "tts-1";
pub const DEFAULT_SPEECH_VOICE: &str = "alloy";
pub const DEFAULT_SPEECH_FORMAT: &str = "mp3";

/// Process env wins, then `CONFIG_DIR/.env`.
///
/// Note the inversion at the route layer: `DEFAULT_PROVIDER` / `DEFAULT_MODEL`
/// read the dotenv file *first*, because the admin UI writes there for a
/// no-restart switch and YAML defaults seed `os.environ` permanently.
pub fn from_env_or_dotenv(key: &str) -> String {
    if let Some(v) = env_opt(key) {
        return v;
    }
    read_env_file().get(key).map(|v| v.trim().to_string()).unwrap_or_default()
}

pub fn ollama_api_base() -> String {
    let explicit = from_env_or_dotenv("OLLAMA_API_BASE");
    let base = if explicit.is_empty() { DEFAULT_OLLAMA_BASE } else { &explicit };
    rewrite_upstream_localhost_for_docker(base)
}

/// Explicit `OLLAMA_API_BASE` only — no default. What the local-backend probe
/// checks before deciding whether to discover one.
pub fn ollama_api_base_from_env_only() -> String {
    from_env_or_dotenv("OLLAMA_API_BASE")
}

pub fn lm_studio_api_base() -> String {
    let explicit = from_env_or_dotenv("LM_STUDIO_API_BASE");
    let base = if explicit.is_empty() { DEFAULT_LM_STUDIO_BASE } else { &explicit };
    rewrite_upstream_localhost_for_docker(base)
}

pub fn lm_studio_api_base_from_env_only() -> String {
    from_env_or_dotenv("LM_STUDIO_API_BASE")
}

/// Optional, for an LM Studio configured to require Bearer auth.
pub fn lm_studio_api_key() -> String {
    from_env_or_dotenv("LM_STUDIO_API_KEY")
}

pub fn gemini_api_key() -> String {
    from_env_or_dotenv("GEMINI_API_KEY")
}

pub fn aimlapi_api_key() -> String {
    from_env_or_dotenv("AIMLAPI_API_KEY")
}

pub fn aimlapi_openai_base() -> String {
    let base = from_env_or_dotenv("AIMLAPI_OPENAI_BASE");
    if base.is_empty() {
        "https://api.aimlapi.com/v1".to_string()
    } else {
        base.trim_end_matches('/').to_string()
    }
}

pub fn anthropic_api_key() -> String {
    from_env_or_dotenv("ANTHROPIC_API_KEY")
}

/// Anthropic's OpenAI-compatible surface (ends with `/v1`, no trailing slash).
pub fn anthropic_openai_base() -> String {
    let base = from_env_or_dotenv("ANTHROPIC_OPENAI_BASE");
    if base.is_empty() {
        "https://api.anthropic.com/v1".to_string()
    } else {
        base.trim_end_matches('/').to_string()
    }
}

pub fn anthropic_version_header() -> String {
    let v = from_env_or_dotenv("ANTHROPIC_VERSION");
    if v.is_empty() { "2023-06-01".to_string() } else { v }
}

/// Base URL of the local image service. Empty means not configured — unlike the
/// chat backends there is no loopback default, so image generation only lights
/// up once an operator points at something.
pub fn image_api_base() -> String {
    let base = from_env_or_dotenv("IMAGE_API_BASE");
    if base.is_empty() {
        String::new()
    } else {
        rewrite_upstream_localhost_for_docker(base.trim_end_matches('/'))
    }
}

pub fn image_default_model() -> String {
    let v = from_env_or_dotenv("IMAGE_DEFAULT_MODEL");
    if v.is_empty() { DEFAULT_IMAGE_MODEL.to_string() } else { v }
}

/// Same rule as the image base: empty until an operator sets `SPEECH_API_BASE`.
pub fn speech_api_base() -> String {
    let base = from_env_or_dotenv("SPEECH_API_BASE");
    if base.is_empty() {
        String::new()
    } else {
        rewrite_upstream_localhost_for_docker(base.trim_end_matches('/'))
    }
}

/// Bearer for whatever `SPEECH_API_BASE` points at; empty for a local Piper or
/// Kokoro server, which wants none.
pub fn speech_api_key() -> String {
    from_env_or_dotenv("SPEECH_API_KEY")
}

pub fn speech_default_model() -> String {
    let v = from_env_or_dotenv("SPEECH_DEFAULT_MODEL");
    if v.is_empty() { DEFAULT_SPEECH_MODEL.to_string() } else { v }
}

pub fn speech_default_voice() -> String {
    let v = from_env_or_dotenv("SPEECH_DEFAULT_VOICE");
    if v.is_empty() { DEFAULT_SPEECH_VOICE.to_string() } else { v }
}

/// A Piper server writes WAV and carries no transcoder, so it wants `wav` here;
/// hosted providers default to mp3.
pub fn speech_default_format() -> String {
    let v = from_env_or_dotenv("SPEECH_DEFAULT_FORMAT");
    if v.is_empty() { DEFAULT_SPEECH_FORMAT.to_string() } else { v }
}

/// Whether this specific backend's requirements are met.
fn spec_configured(spec: &ProviderSpec) -> bool {
    match spec.id {
        // Both local chat backends have a loopback default, so these are always
        // true — matching Python, where "configured" means "we know a URL to
        // try", not "something is listening". Reachability is a separate probe.
        "ollama" => !ollama_api_base().is_empty(),
        "lm_studio" => !lm_studio_api_base().is_empty(),
        "aimlapi" => !aimlapi_api_key().is_empty(),
        "anthropic" => !anthropic_api_key().is_empty(),
        "gemini" => !gemini_api_key().is_empty(),
        "image_local" => !image_api_base().is_empty(),
        "speech_local" => !speech_api_base().is_empty(),
        _ => true,
    }
}

/// The *chat*-registry configured check, as `llm.py` calls it.
///
/// Empty, `"other"`, and any name not in the chat registry answer `true`: a
/// provider named in `config.yaml` but not yet implemented here keeps showing up
/// in the model list rather than silently vanishing. That deliberately includes
/// `image_local` and `speech_local` — use [`is_configured`] for a question about
/// a capability provider.
pub fn provider_configured(provider: &str) -> bool {
    let name = provider.trim().to_ascii_lowercase();
    if name.is_empty() || name == "other" {
        return true;
    }
    spec_in(&name, Registry::Chat).is_none_or(spec_configured)
}

/// The capability router's configured check: dispatches across all three
/// registries, so `image_local` and `speech_local` answer for themselves.
pub fn is_configured(provider: &str) -> bool {
    match spec(provider) {
        Some(s) if s.registry != Registry::Chat => spec_configured(s),
        _ => provider_configured(provider),
    }
}

/// Prefer local backends, then cloud. Falls back to `lm_studio` so callers
/// always have a name to route with.
pub fn first_configured_provider() -> &'static str {
    PROVIDERS
        .iter()
        .filter(|p| p.registry == Registry::Chat)
        .find(|p| spec_configured(p))
        .map_or("lm_studio", |p| p.id)
}

// ---------------------------------------------------------------------------
// Capability routing
// ---------------------------------------------------------------------------

/// Declared modalities, chat-only for anything unregistered — the same
/// forward-compatible default `provider_configured` takes.
pub fn provider_modalities(provider: &str) -> &'static [Modality] {
    spec(provider).map_or(&[Chat], |p| p.modalities)
}

pub fn provider_supports(provider: &str, capability: Modality) -> bool {
    provider_modalities(provider).contains(&capability)
}

/// Flat `{modality: bool}` map for the capability catalog surface.
pub fn modality_map(provider: &str) -> Map<String, Value> {
    let declared = provider_modalities(provider);
    MODALITIES
        .iter()
        .map(|m| (m.as_str().to_string(), Value::Bool(declared.contains(m))))
        .collect()
}

/// Every capability provider, local backends first. `sort_order` ties are broken
/// by declaration order, matching Python's stable sort.
pub fn providers_by_local_preference() -> Vec<&'static ProviderSpec> {
    let mut all: Vec<&'static ProviderSpec> = PROVIDERS.iter().collect();
    all.sort_by_key(|p| p.sort_order);
    all
}

/// First *configured* provider declaring `capability`, or `None`. Local backends
/// are preferred so requests stay on-box when they can.
pub fn resolve_provider_for_capability(capability: Modality) -> Option<&'static str> {
    providers_by_local_preference()
        .into_iter()
        .find(|p| p.modalities.contains(&capability) && spec_configured(p))
        .map(|p| p.id)
}

/// All configured providers declaring `capability`, in preference order.
pub fn providers_for_capability(capability: Modality) -> Vec<&'static str> {
    providers_by_local_preference()
        .into_iter()
        .filter(|p| p.modalities.contains(&capability) && spec_configured(p))
        .map(|p| p.id)
        .collect()
}

/// Resolve a provider for `capability` or fail with the structured `501`/`503`.
///
/// `preferred` pins a caller's choice, and is honoured only when that provider
/// both declares the capability and is configured — otherwise the error names
/// what *is* available. An unregistered name passes both checks (chat-only
/// default, forward-compatible "configured"), exactly as in Python; the upstream
/// URL lookup is what rejects it.
pub fn require_provider_for_capability(
    capability: Modality,
    preferred: Option<&str>,
) -> Result<String, ApiError> {
    let pref = preferred.unwrap_or_default().trim().to_ascii_lowercase();
    if !pref.is_empty() {
        if !provider_supports(&pref, capability) {
            return Err(ApiError::coded(
                StatusCode::NOT_IMPLEMENTED,
                "capability_unavailable",
                format!("Provider {pref} does not support {}.", capability.as_str()),
            )
            .with_extra(capability_extra(capability)));
        }
        if !is_configured(&pref) {
            return Err(ApiError::coded(
                StatusCode::SERVICE_UNAVAILABLE,
                "provider_not_configured",
                format!(
                    "Provider {pref} is not configured (check environment for this provider)."
                ),
            ));
        }
        return Ok(pref);
    }

    resolve_provider_for_capability(capability).map(str::to_string).ok_or_else(|| {
        ApiError::coded(
            StatusCode::NOT_IMPLEMENTED,
            "capability_unavailable",
            format!("No configured provider supports {}.", capability.as_str()),
        )
        .with_extra(capability_extra(capability))
    })
}

fn capability_extra(capability: Modality) -> Value {
    let declaring: Vec<&str> = providers_by_local_preference()
        .into_iter()
        .filter(|p| p.modalities.contains(&capability))
        .map(|p| p.id)
        .collect();
    json!({
        "capability": capability.as_str(),
        "providers_with_capability": declaring,
        "configured_providers_with_capability": providers_for_capability(capability),
    })
}

// ---------------------------------------------------------------------------
// Config files
// ---------------------------------------------------------------------------

/// Directory holding the proxy's `config.yaml`, `.env` and caches. Compose and
/// the desktop shell both set `CONFIG_DIR`; the relative default matches the
/// `AGENT_PLATFORM_DB_PATH` one and resolves against the working directory.
pub fn config_dir() -> PathBuf {
    env_opt("CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("data/llm"))
}

pub fn config_yaml_path() -> PathBuf {
    env_opt("CONFIG_PATH").map(PathBuf::from).unwrap_or_else(|| config_dir().join("config.yaml"))
}

pub fn env_file_path() -> PathBuf {
    config_dir().join(".env")
}

/// Legacy filename on disk: the proxy UI's `fallback_models` live in
/// `orchestrator_ui.yaml`.
pub fn ui_yaml_path() -> PathBuf {
    config_dir().join("orchestrator_ui.yaml")
}

/// Path plus mtime and size. Python fingerprints on mtime+size alone and keeps
/// one slot per file *kind*, so pointing `CONFIG_DIR` somewhere else mid-process
/// can serve the old parse; including the path costs nothing and cannot.
type Fingerprint = (PathBuf, i64, u64);

fn fingerprint(path: &Path) -> Option<Fingerprint> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as i64);
    Some((path.to_path_buf(), mtime, meta.len()))
}

type Slot<T> = OnceLock<Mutex<Option<(Fingerprint, Arc<T>)>>>;

/// Parse `path` through `slot`, re-reading only when the file changes.
fn cached<T: Default>(slot: &Slot<T>, path: &Path, parse: impl FnOnce(&str) -> T) -> Arc<T> {
    let cell = slot.get_or_init(|| Mutex::new(None));
    let mut guard = cell.lock().unwrap_or_else(|e| e.into_inner());

    let Some(fp) = fingerprint(path) else {
        *guard = None;
        return Arc::new(T::default());
    };
    if let Some((cached_fp, value)) = guard.as_ref() {
        if *cached_fp == fp {
            return Arc::clone(value);
        }
    }

    let value = Arc::new(std::fs::read_to_string(path).map(|raw| parse(&raw)).unwrap_or_default());
    // Re-stat after reading: a write that lands mid-read must not be cached
    // under the pre-write fingerprint and then served until the next change.
    if let Some(fp_after) = fingerprint(path) {
        *guard = Some((fp_after, Arc::clone(&value)));
    } else {
        *guard = None;
    }
    value
}

static YAML_SLOT: Slot<Map<String, Value>> = OnceLock::new();
static ENV_SLOT: Slot<HashMap<String, String>> = OnceLock::new();
static UI_SLOT: Slot<HashMap<String, Vec<String>>> = OnceLock::new();

/// Parsed `config.yaml`, empty when it is missing, malformed, or not a mapping —
/// a broken config must not take the proxy down with it.
pub fn load_config_yaml() -> Arc<Map<String, Value>> {
    cached(&YAML_SLOT, &config_yaml_path(), |raw| match serde_yaml::from_str::<Value>(raw) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    })
}

/// Parsed `CONFIG_DIR/.env`.
pub fn read_env_file() -> Arc<HashMap<String, String>> {
    cached(&ENV_SLOT, &env_file_path(), parse_env_text)
}

/// Not a full dotenv implementation: `KEY=value`, `#` comments, and one layer of
/// matching surrounding quotes, same as `config_cache._parse_env_file`.
pub fn parse_env_text(raw: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        out.insert(k.trim().to_string(), unquote(v.trim()).to_string());
    }
    out
}

fn unquote(v: &str) -> &str {
    let bytes = v.as_bytes();
    if bytes.len() >= 2 && (bytes[0] == b'"' || bytes[0] == b'\'') && bytes[0] == bytes[bytes.len() - 1]
    {
        &v[1..v.len() - 1]
    } else {
        v
    }
}

/// `orchestrator_ui.yaml`'s `fallback_models` map. A bare string is read as a
/// one-element list, which is how the file is written by hand.
pub fn read_ui_fallbacks() -> Arc<HashMap<String, Vec<String>>> {
    cached(&UI_SLOT, &ui_yaml_path(), |raw| {
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        let Ok(parsed) = serde_yaml::from_str::<Value>(raw) else { return out };
        let Some(map) = parsed.get("fallback_models").and_then(Value::as_object) else {
            return out;
        };
        for (key, value) in map {
            let key = key.trim();
            if key.is_empty() {
                continue;
            }
            let values: Vec<String> = match value {
                Value::Array(items) => items
                    .iter()
                    .filter_map(stringify_scalar)
                    .filter(|s| !s.is_empty())
                    .collect(),
                other => stringify_scalar(other).filter(|s| !s.is_empty()).into_iter().collect(),
            };
            if !values.is_empty() {
                out.insert(key.to_string(), values);
            }
        }
        out
    })
}

/// YAML scalars reach us as whatever JSON type they parsed to; Python calls
/// `str()` on each, so a bare `7` is the model id `"7"`.
fn stringify_scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Docker
// ---------------------------------------------------------------------------

/// Inside a container, a loopback LM Studio / Ollama base means the *container*,
/// not the machine running it. Compose keeps `AGENT_PLATFORM_LOCAL_LLM_DOCKER_FIX`
/// separate from the self-call fix so it can disable one without the other.
pub fn rewrite_upstream_localhost_for_docker(url: &str) -> String {
    let url = url.trim();
    if url.is_empty() || flag_disabled("AGENT_PLATFORM_LOCAL_LLM_DOCKER_FIX") {
        return url.to_string();
    }
    if !Path::new("/.dockerenv").exists() {
        return url.to_string();
    }
    let Some((scheme, rest)) = url.split_once("://") else { return url.to_string() };
    let (authority, path) = rest.split_once('/').map_or((rest, ""), |(a, p)| (a, p));
    let (host, port) = authority.rsplit_once(':').map_or((authority, ""), |(h, p)| (h, p));
    if host != "127.0.0.1" && host != "localhost" {
        return url.to_string();
    }
    let mut out = format!("{scheme}://host.docker.internal");
    if !port.is_empty() {
        out.push(':');
        out.push_str(port);
    }
    if !path.is_empty() {
        out.push('/');
        out.push_str(path);
    }
    out.trim_end_matches('/').to_string()
}

fn flag_disabled(name: &str) -> bool {
    matches!(
        env_opt(name).unwrap_or_default().to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;

    /// Every test here moves `CONFIG_DIR`, and the file caches are process-wide.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct Fixture {
        dir: PathBuf,
        _guard: MutexGuard<'static, ()>,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let dir = std::env::temp_dir().join(format!("agp-llm-config-{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::env::set_var("CONFIG_DIR", &dir);
            std::env::remove_var("CONFIG_PATH");
            Self { dir, _guard: guard }
        }

        fn write(&self, name: &str, body: &str) {
            std::fs::write(self.dir.join(name), body).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::env::remove_var("CONFIG_DIR");
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn dotenv_fills_in_for_process_env_and_reparses_on_change() {
        let fx = Fixture::new("dotenv");
        std::env::remove_var("AIMLAPI_API_KEY");
        fx.write(".env", "# comment\nAIMLAPI_API_KEY = \"from-file\"\nbroken line\n");
        assert_eq!(from_env_or_dotenv("AIMLAPI_API_KEY"), "from-file");
        assert!(provider_configured("aimlapi"));

        // Process env wins over the file, and a shorter rewrite invalidates the
        // cache on size alone (mtime granularity is not assumed).
        std::env::set_var("AIMLAPI_API_KEY", "from-env");
        assert_eq!(from_env_or_dotenv("AIMLAPI_API_KEY"), "from-env");
        std::env::remove_var("AIMLAPI_API_KEY");

        fx.write(".env", "AIMLAPI_API_KEY=v2\n");
        assert_eq!(from_env_or_dotenv("AIMLAPI_API_KEY"), "v2");

        std::fs::remove_file(fx.dir.join(".env")).unwrap();
        assert_eq!(from_env_or_dotenv("AIMLAPI_API_KEY"), "");
        assert!(!provider_configured("aimlapi"));
    }

    #[test]
    fn unconfigured_and_unknown_providers_answer_python_s_way() {
        let _fx = Fixture::new("configured");
        for key in ["AIMLAPI_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY", "IMAGE_API_BASE"] {
            std::env::remove_var(key);
        }
        // Local chat backends carry a loopback default, so they are always
        // "configured"; the cloud ones need a key.
        assert!(provider_configured("ollama"));
        assert!(provider_configured("lm_studio"));
        assert!(!provider_configured("gemini"));
        // Empty, "other" and unregistered names stay visible.
        assert!(provider_configured(""));
        assert!(provider_configured("other"));
        assert!(provider_configured("some-future-vendor"));
        // ...including the image backend, when asked through the *chat* check.
        assert!(provider_configured("image_local"));
        assert!(!is_configured("image_local"));

        assert!(is_supported_provider("anthropic"));
        assert!(!is_supported_provider("image_local"));
        assert_eq!(first_configured_provider(), "ollama");
    }

    #[test]
    fn capability_routing_prefers_local_and_reports_what_is_available() {
        let _fx = Fixture::new("capability");
        for key in ["IMAGE_API_BASE", "SPEECH_API_BASE", "GEMINI_API_KEY"] {
            std::env::remove_var(key);
        }

        // Ollama declares chat and sorts first; nothing serves embeddings but
        // LM Studio, which also has a loopback default.
        assert_eq!(resolve_provider_for_capability(Modality::Chat), Some("ollama"));
        assert_eq!(providers_for_capability(Modality::Embeddings), vec!["lm_studio"]);

        // No image backend configured => 501 naming the one that could serve it.
        let err = require_provider_for_capability(Modality::ImageGeneration, None).unwrap_err();
        assert_eq!(err.status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(err.code, "capability_unavailable");
        let extra = err.extra.unwrap();
        assert_eq!(extra["providers_with_capability"], json!(["image_local"]));
        assert_eq!(extra["configured_providers_with_capability"], json!([]));

        std::env::set_var("IMAGE_API_BASE", "http://127.0.0.1:9000/");
        assert_eq!(image_api_base(), "http://127.0.0.1:9000");
        assert_eq!(
            require_provider_for_capability(Modality::ImageGeneration, None).unwrap(),
            "image_local"
        );
        // A pin at a provider that cannot serve the capability is a 501, not a
        // silent fallback to the one that can.
        let err = require_provider_for_capability(Modality::ImageGeneration, Some("Ollama"))
            .unwrap_err();
        assert_eq!(err.status, StatusCode::NOT_IMPLEMENTED);
        std::env::remove_var("IMAGE_API_BASE");

        let map = modality_map("anthropic");
        assert_eq!(map["chat"], json!(true));
        assert_eq!(map["vision_input"], json!(true));
        assert_eq!(map["embeddings"], json!(false));
        assert_eq!(map.len(), MODALITIES.len());
    }

    #[test]
    fn yaml_reads_survive_a_malformed_file() {
        let fx = Fixture::new("yaml");
        assert!(load_config_yaml().is_empty());

        fx.write("config.yaml", "defaults:\n  provider: ollama\n  model: llama3\n");
        let data = load_config_yaml();
        assert_eq!(data["defaults"]["provider"], json!("ollama"));

        fx.write("config.yaml", "defaults: [unclosed\n");
        assert!(load_config_yaml().is_empty());

        fx.write("orchestrator_ui.yaml", "fallback_models:\n  ollama:\n    - llama3\n  gemini: gemini-2.0-flash\n");
        let fb = read_ui_fallbacks();
        assert_eq!(fb["ollama"], vec!["llama3".to_string()]);
        assert_eq!(fb["gemini"], vec!["gemini-2.0-flash".to_string()]);
    }

    #[test]
    fn docker_rewrite_is_off_outside_a_container() {
        let _fx = Fixture::new("docker");
        std::env::remove_var("AGENT_PLATFORM_LOCAL_LLM_DOCKER_FIX");
        // The rewrite is gated on /.dockerenv, which does not exist on a dev box;
        // this asserts the pass-through, since the container path cannot be
        // exercised from here.
        assert!(!Path::new("/.dockerenv").exists());
        assert_eq!(
            rewrite_upstream_localhost_for_docker("http://127.0.0.1:11434"),
            "http://127.0.0.1:11434"
        );
        std::env::set_var("AGENT_PLATFORM_LOCAL_LLM_DOCKER_FIX", "0");
        assert_eq!(rewrite_upstream_localhost_for_docker("http://localhost:1234"), "http://localhost:1234");
        std::env::remove_var("AGENT_PLATFORM_LOCAL_LLM_DOCKER_FIX");
    }
}
