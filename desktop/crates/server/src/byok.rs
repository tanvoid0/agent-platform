//! BYOK (bring-your-own-key): route a request through the client's own provider
//! key. Port of `app/llm_proxy/core/byok.py`.
//!
//! The platform token still gates *access* — every `/v1` route that accepts BYOK
//! authenticates first. BYOK only replaces the *upstream* credential, so the
//! proxy forwards with the caller's key and spends none of the platform's quota.
//!
//! Transport is headers, which keeps the secret out of the body and the logs and
//! works with a stock OpenAI SDK client via `default_headers`:
//!
//! ```text
//! X-BYOK-Provider           required to activate BYOK (e.g. "openai")
//! X-BYOK-Api-Key            the caller's upstream key
//! X-BYOK-Base-Url           optional; host must be allowlisted (SSRF guard)
//! X-BYOK-Anthropic-Version  optional; overrides the anthropic-version pin
//! ```
//!
//! A custom base URL is accepted only when it is https, carries no credentials,
//! names a hostname rather than a raw IP, and that host is allowlisted — the
//! provider's canonical host plus any `BYOK_ALLOWED_HOSTS` the operator adds.
//! That is what stops a caller pointing the proxy at an internal service.

use axum::http::{HeaderMap, StatusCode};
use serde_json::{json, Value};
use url::Url;

use crate::error::ApiError;
use crate::llm_config::{from_env_or_dotenv, Modality};

pub const HEADER_PROVIDER: &str = "x-byok-provider";
pub const HEADER_API_KEY: &str = "x-byok-api-key";
pub const HEADER_BASE_URL: &str = "x-byok-base-url";
pub const HEADER_ANTHROPIC_VERSION: &str = "x-byok-anthropic-version";

const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";

// Every vendor below serves these three at the same paths. Python carries them
// as per-spec fields that nothing overrides; they are constants until one does.
const CHAT_PATH: &str = "/chat/completions";
const EMBEDDINGS_PATH: &str = "/embeddings";
const IMAGES_PATH: &str = "/images/generations";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStyle {
    Bearer,
    /// Bearer *and* the native `x-api-key` + `anthropic-version` pair, which
    /// some Anthropic surfaces expect.
    Anthropic,
}

pub struct ByokSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// Includes the version path (e.g. `.../v1`); no trailing slash.
    pub canonical_base: &'static str,
    pub canonical_host: &'static str,
    pub auth_style: AuthStyle,
    /// Listed in the order `sorted(spec.modalities)` produces in Python, which
    /// is what both the discovery document and the `501` extra report.
    pub modalities: &'static [Modality],
}

use Modality::{Chat, Embeddings, ImageGeneration};

/// OpenAI-compatible, key-based vendors. Local backends (ollama, lm_studio) are
/// deliberately absent: BYOK exists to avoid spending the platform's cloud quota,
/// and a local backend costs none.
pub const BYOK_PROVIDERS: &[ByokSpec] = &[
    ByokSpec {
        id: "openai",
        label: "OpenAI",
        canonical_base: "https://api.openai.com/v1",
        canonical_host: "api.openai.com",
        auth_style: AuthStyle::Bearer,
        modalities: &[Chat, Embeddings, ImageGeneration],
    },
    ByokSpec {
        // Claude's OpenAI-compat surface has no embeddings endpoint.
        id: "anthropic",
        label: "Claude",
        canonical_base: "https://api.anthropic.com/v1",
        canonical_host: "api.anthropic.com",
        auth_style: AuthStyle::Anthropic,
        modalities: &[Chat],
    },
    ByokSpec {
        id: "gemini",
        label: "Gemini",
        canonical_base: "https://generativelanguage.googleapis.com/v1beta/openai",
        canonical_host: "generativelanguage.googleapis.com",
        auth_style: AuthStyle::Bearer,
        modalities: &[Chat, Embeddings],
    },
    ByokSpec {
        id: "aimlapi",
        label: "AIMLAPI",
        canonical_base: "https://api.aimlapi.com/v1",
        canonical_host: "api.aimlapi.com",
        auth_style: AuthStyle::Bearer,
        modalities: &[Chat, Embeddings],
    },
    ByokSpec {
        id: "openrouter",
        label: "OpenRouter",
        canonical_base: "https://openrouter.ai/api/v1",
        canonical_host: "openrouter.ai",
        auth_style: AuthStyle::Bearer,
        modalities: &[Chat, Embeddings],
    },
    ByokSpec {
        id: "groq",
        label: "Groq",
        canonical_base: "https://api.groq.com/openai/v1",
        canonical_host: "api.groq.com",
        auth_style: AuthStyle::Bearer,
        modalities: &[Chat],
    },
    ByokSpec {
        id: "mistral",
        label: "Mistral",
        canonical_base: "https://api.mistral.ai/v1",
        canonical_host: "api.mistral.ai",
        auth_style: AuthStyle::Bearer,
        modalities: &[Chat, Embeddings],
    },
];

/// Self-describing BYOK contract for `GET /v1/capabilities`.
///
/// Lets a client learn up front which providers it may bring a key for, each
/// one's modalities and canonical host, and the header names to send — instead
/// of discovering support through a failed request.
pub fn discovery() -> Value {
    json!({
        "enabled": true,
        "transport": "headers",
        "headers": {
            "provider": "X-BYOK-Provider",
            "api_key": "X-BYOK-Api-Key",
            "base_url": "X-BYOK-Base-Url",
            "anthropic_version": "X-BYOK-Anthropic-Version",
        },
        "extra_allowed_hosts": extra_allowed_hosts(),
        "providers": BYOK_PROVIDERS.iter().map(|spec| json!({
            "id": spec.id,
            "label": spec.label,
            "modalities": modality_names(spec),
            "canonical_host": spec.canonical_host,
        })).collect::<Vec<_>>(),
    })
}

fn modality_names(spec: &ByokSpec) -> Vec<&'static str> {
    spec.modalities.iter().map(|m| m.as_str()).collect()
}

/// Operator-added hosts (`BYOK_ALLOWED_HOSTS`, comma-separated), sorted.
fn extra_allowed_hosts() -> Vec<String> {
    let raw = from_env_or_dotenv("BYOK_ALLOWED_HOSTS");
    let mut hosts: Vec<String> = raw
        .split(',')
        .map(|h| h.trim().to_ascii_lowercase())
        .filter(|h| !h.is_empty())
        .collect();
    hosts.sort();
    hosts.dedup();
    hosts
}

/// Resolved BYOK target: where to send, and how to authenticate.
///
/// Deliberately not `Debug`: it holds the caller's upstream key, and the whole
/// point of the header transport is that the key never reaches a log line.
#[derive(Clone)]
pub struct Route {
    pub spec: &'static ByokSpec,
    pub api_key: String,
    /// Canonical or validated custom base; no trailing slash.
    pub base: String,
    pub anthropic_version: String,
}

impl Route {
    pub fn supports(&self, capability: Modality) -> bool {
        self.spec.modalities.contains(&capability)
    }

    /// The structured `501` for a capability this BYOK provider cannot serve.
    pub fn require(&self, capability: Modality) -> Result<(), ApiError> {
        if self.supports(capability) {
            return Ok(());
        }
        Err(ApiError::coded(
            StatusCode::NOT_IMPLEMENTED,
            "capability_unavailable",
            format!(
                "BYOK provider {} does not support {}.",
                self.spec.id,
                capability.as_str()
            ),
        )
        .with_extra(json!({
            "capability": capability.as_str(),
            "byok_provider": self.spec.id,
            "byok_provider_modalities": modality_names(self.spec),
        })))
    }

    pub fn url(&self, capability: Modality) -> String {
        let path = match capability {
            Modality::Chat => CHAT_PATH,
            Modality::Embeddings => EMBEDDINGS_PATH,
            _ => IMAGES_PATH,
        };
        format!("{}{path}", self.base)
    }

    pub fn outbound_headers(&self) -> Vec<(&'static str, String)> {
        let mut headers = vec![
            ("Content-Type", "application/json".to_string()),
            ("Authorization", format!("Bearer {}", self.api_key)),
        ];
        if self.spec.auth_style == AuthStyle::Anthropic {
            headers.push(("x-api-key", self.api_key.clone()));
            headers.push(("anthropic-version", self.anthropic_version.clone()));
        }
        headers
    }
}

/// Build a `Route` from request headers, or `None` when BYOK is not requested.
///
/// BYOK activates only on `X-BYOK-Provider`. A provider without a key, an
/// unknown provider, or a disallowed base URL is an error rather than a silent
/// fall back to the platform's own credentials.
pub fn parse(headers: &HeaderMap) -> Result<Option<Route>, ApiError> {
    let provider = header(headers, HEADER_PROVIDER).to_ascii_lowercase();
    if provider.is_empty() {
        return Ok(None);
    }

    let Some(spec) = BYOK_PROVIDERS.iter().find(|s| s.id == provider) else {
        return Err(ApiError::coded(
            StatusCode::BAD_REQUEST,
            "byok_unknown_provider",
            format!("Unknown BYOK provider '{provider}'."),
        )
        .with_extra(json!({
            "byok_providers": BYOK_PROVIDERS.iter().map(|s| s.id).collect::<Vec<_>>(),
        })));
    };

    let api_key = header(headers, HEADER_API_KEY);
    if api_key.is_empty() {
        return Err(ApiError::coded(
            StatusCode::BAD_REQUEST,
            "byok_missing_key",
            "X-BYOK-Api-Key is required when X-BYOK-Provider is set.",
        ));
    }

    let raw_base = header(headers, HEADER_BASE_URL);
    let base = if raw_base.is_empty() {
        spec.canonical_base.to_string()
    } else {
        validate_custom_base(spec, &raw_base)?
    };

    let anthropic_version = {
        let v = header(headers, HEADER_ANTHROPIC_VERSION);
        if v.is_empty() { DEFAULT_ANTHROPIC_VERSION.to_string() } else { v }
    };

    Ok(Some(Route { spec, api_key, base, anthropic_version }))
}

fn header(headers: &HeaderMap, name: &str) -> String {
    headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default().trim().to_string()
}

fn invalid_base(message: &str) -> ApiError {
    ApiError::coded(StatusCode::BAD_REQUEST, "byok_invalid_base_url", message)
}

fn validate_custom_base(spec: &ByokSpec, raw: &str) -> Result<String, ApiError> {
    let parsed = Url::parse(raw).map_err(|_| {
        // Unparseable: report whichever of the two checks it would have failed
        // first, so the message still tells the caller what to fix.
        if raw.to_ascii_lowercase().starts_with("https://") {
            invalid_base("X-BYOK-Base-Url is missing a host.")
        } else {
            invalid_base("X-BYOK-Base-Url must be an https URL.")
        }
    })?;

    if parsed.scheme() != "https" {
        return Err(invalid_base("X-BYOK-Base-Url must be an https URL."));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(invalid_base("X-BYOK-Base-Url must not contain credentials."));
    }
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    if host.is_empty() {
        return Err(invalid_base("X-BYOK-Base-Url is missing a host."));
    }
    // `Url` strips the brackets from an IPv6 literal, so both families parse here.
    if host.parse::<std::net::IpAddr>().is_ok() {
        return Err(ApiError::coded(
            StatusCode::FORBIDDEN,
            "byok_host_not_allowed",
            "X-BYOK-Base-Url must be a hostname, not an IP address.",
        ));
    }

    let allowed = allowed_hosts(spec);
    if !allowed.contains(&host) {
        return Err(ApiError::coded(
            StatusCode::FORBIDDEN,
            "byok_host_not_allowed",
            format!(
                "Host {host} is not allowlisted for BYOK. \
                 Set BYOK_ALLOWED_HOSTS to permit additional upstreams."
            ),
        )
        .with_extra(json!({ "host": host, "allowed_hosts": allowed })));
    }

    // The caller's path prefix is preserved — regional and proxied endpoints
    // carry one — so the raw string is returned rather than the normalized URL.
    Ok(raw.trim_end_matches('/').to_string())
}

fn allowed_hosts(spec: &ByokSpec) -> Vec<String> {
    let mut hosts = extra_allowed_hosts();
    hosts.push(spec.canonical_host.to_string());
    hosts.sort();
    hosts.dedup();
    hosts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Result::unwrap_err` wants `T: Debug`, and `Route` deliberately is not.
    fn expect_err(result: Result<Option<Route>, ApiError>) -> ApiError {
        match result {
            Err(e) => e,
            Ok(_) => panic!("expected a rejection"),
        }
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        map
    }

    #[test]
    fn absent_provider_header_means_no_byok() {
        assert!(parse(&headers(&[])).unwrap().is_none());
        assert!(parse(&headers(&[(HEADER_API_KEY, "sk-x")])).unwrap().is_none());
    }

    #[test]
    fn provider_without_a_key_is_a_named_400() {
        let err = expect_err(parse(&headers(&[(HEADER_PROVIDER, "openai")])));
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert_eq!(err.code, "byok_missing_key");

        let err = expect_err(parse(&headers(&[(HEADER_PROVIDER, "nope"), (HEADER_API_KEY, "k")])));
        assert_eq!(err.code, "byok_unknown_provider");
        assert_eq!(err.extra.unwrap()["byok_providers"][0], json!("openai"));
    }

    #[test]
    fn canonical_route_and_capability_refusal() {
        let route = parse(&headers(&[(HEADER_PROVIDER, "Anthropic"), (HEADER_API_KEY, "k")]))
            .unwrap()
            .unwrap();
        assert_eq!(route.url(Modality::Chat), "https://api.anthropic.com/v1/chat/completions");
        assert!(route.require(Modality::Chat).is_ok());

        // Claude's OpenAI-compat surface has no embeddings endpoint.
        let err = route.require(Modality::Embeddings).unwrap_err();
        assert_eq!(err.status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(err.code, "capability_unavailable");
        assert_eq!(err.extra.unwrap()["byok_provider_modalities"], json!(["chat"]));

        let sent = route.outbound_headers();
        assert!(sent.iter().any(|(k, v)| *k == "x-api-key" && v == "k"));
        assert!(sent.iter().any(|(k, v)| *k == "anthropic-version" && v == "2023-06-01"));

        let bearer_only = parse(&headers(&[(HEADER_PROVIDER, "groq"), (HEADER_API_KEY, "k")]))
            .unwrap()
            .unwrap();
        assert!(!bearer_only.outbound_headers().iter().any(|(k, _)| *k == "x-api-key"));
    }

    #[test]
    fn custom_base_is_an_ssrf_guard() {
        let with_base = |base: &str| {
            parse(&headers(&[
                (HEADER_PROVIDER, "openai"),
                (HEADER_API_KEY, "k"),
                (HEADER_BASE_URL, base),
            ]))
        };

        for (base, status, code) in [
            ("http://api.openai.com/v1", StatusCode::BAD_REQUEST, "byok_invalid_base_url"),
            ("https://u:p@api.openai.com/v1", StatusCode::BAD_REQUEST, "byok_invalid_base_url"),
            ("not-a-url", StatusCode::BAD_REQUEST, "byok_invalid_base_url"),
            // The whole point: no raw IPs, and no unlisted host.
            ("https://10.0.0.1/v1", StatusCode::FORBIDDEN, "byok_host_not_allowed"),
            ("https://[::1]/v1", StatusCode::FORBIDDEN, "byok_host_not_allowed"),
            ("https://evil.example/v1", StatusCode::FORBIDDEN, "byok_host_not_allowed"),
        ] {
            let err = expect_err(with_base(base));
            assert_eq!(err.status, status, "{base}");
            assert_eq!(err.code, code, "{base}");
        }

        // The canonical host is allowed, and the caller's path prefix survives.
        let route = with_base("https://api.openai.com/regional/v1/").unwrap().unwrap();
        assert_eq!(route.base, "https://api.openai.com/regional/v1");
        assert_eq!(route.url(Modality::Embeddings), "https://api.openai.com/regional/v1/embeddings");
    }
}
