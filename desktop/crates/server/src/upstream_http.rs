//! Calls out to LLM vendors: retries, rate-limit backoff, error classification.
//! Port of `app/llm_proxy/services/upstream_http.py`.
//!
//! One entry point, [`send_with_retry`], because every caller wants the same
//! thing — a request rebuilt and re-sent on a transport failure, and (for the
//! ones that write) re-sent again when the vendor says "too many". The response
//! body is read before returning, since deciding whether a 4xx *is* a rate limit
//! means looking at it.
//!
//! Two knowing divergences from httpx:
//!
//! - **Coarser error codes.** httpx distinguishes connect/read/write/pool
//!   timeouts; reqwest reports "timed out". `write_timeout` and `pool_timeout`
//!   therefore never appear, and everything unclassified lands on
//!   `transport_error`. The codes are diagnostic — nothing in this repo branches
//!   on them — so the message stays useful and the taxonomy narrows.
//! - **No pool ceiling.** httpx caps total connections at 100 for the whole
//!   process; reqwest pools per host with no global cap, so there is nothing to
//!   configure and no `pool_timeout` to raise.

use std::sync::OnceLock;
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use url::Url;

use crate::env_opt;
use crate::error::ApiError;

pub struct UpstreamResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl UpstreamResponse {
    pub fn is_ok(&self) -> bool {
        self.status == StatusCode::OK
    }

    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&self.body).ok()
    }

    pub fn text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    pub fn content_type(&self, fallback: &'static str) -> String {
        self.headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(fallback)
            .to_string()
    }
}

/// Read once, like the module-level client Python builds at import.
struct Policy {
    max_retries: u32,
    backoff_ms: u64,
    rate_limit_max_retries: u32,
    rate_limit_backoff_ms: u64,
}

fn policy() -> &'static Policy {
    static POLICY: OnceLock<Policy> = OnceLock::new();
    POLICY.get_or_init(|| Policy {
        max_retries: env_num("ORCHESTRATOR_HTTP_MAX_RETRIES", 3).max(1) as u32,
        backoff_ms: env_num("ORCHESTRATOR_HTTP_RETRY_BACKOFF_MS", 120).max(10),
        // A separate, more generous budget for "many agents at once" than for a
        // raw connection failure.
        rate_limit_max_retries: env_num("ORCHESTRATOR_HTTP_RATE_LIMIT_MAX_RETRIES", 6).max(1) as u32,
        rate_limit_backoff_ms: env_num("ORCHESTRATOR_HTTP_RATE_LIMIT_BACKOFF_MS", 400).max(10),
    })
}

fn env_num(name: &str, default: u64) -> u64 {
    env_opt(name).and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Send `build()`, retrying transport failures and — when `retry_rate_limits` —
/// vendor rate-limit responses. `build` is a closure because a retry needs a
/// fresh request, not a replayed one.
pub async fn send_with_retry(
    context: &str,
    retry_rate_limits: bool,
    build: impl Fn() -> reqwest::RequestBuilder,
) -> Result<UpstreamResponse, ApiError> {
    let policy = policy();
    let mut transport_attempt = 0u32;
    let mut rate_limit_attempt = 0u32;

    loop {
        let response = match build().send().await {
            Ok(r) => r,
            Err(e) => {
                if should_retry_transport(&e, transport_attempt, policy.max_retries) {
                    let delay = backoff(transport_attempt, policy.backoff_ms);
                    eprintln!(
                        "[agent-platformd] retry {context} attempt={}/{} delay={:.2}s url={} err={}",
                        transport_attempt + 1,
                        policy.max_retries,
                        delay.as_secs_f64(),
                        sanitize_url(e.url().map(Url::as_str).unwrap_or("")),
                        classify_code(&e),
                    );
                    tokio::time::sleep(delay).await;
                    transport_attempt += 1;
                    continue;
                }
                return Err(transport_error(&e, context));
            }
        };

        let status = StatusCode::from_u16(response.status().as_u16())
            .unwrap_or(StatusCode::BAD_GATEWAY);
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<f64>().ok());
        let headers = copy_headers(response.headers());
        let url = response.url().to_string();
        let body = match response.bytes().await {
            Ok(b) => b.to_vec(),
            // The status arrived but the body did not; httpx raises the same
            // classified transport error here.
            Err(e) => return Err(transport_error(&e, context)),
        };

        if retry_rate_limits
            && rate_limit_attempt < policy.rate_limit_max_retries.saturating_sub(1)
            && is_rate_limited(status, &String::from_utf8_lossy(&body))
        {
            let delay = retry_after
                .filter(|s| *s >= 0.0)
                .map(|s| Duration::from_secs_f64(s.min(30.0)))
                .unwrap_or_else(|| backoff(rate_limit_attempt, policy.rate_limit_backoff_ms));
            eprintln!(
                "[agent-platformd] rate_limited {context} attempt={}/{} delay={:.2}s url={} status={}",
                rate_limit_attempt + 1,
                policy.rate_limit_max_retries,
                delay.as_secs_f64(),
                sanitize_url(&url),
                status.as_u16(),
            );
            tokio::time::sleep(delay).await;
            rate_limit_attempt += 1;
            continue;
        }

        return Ok(UpstreamResponse { status, headers, body });
    }
}

/// Open a streaming response, leaving the body for the caller to forward.
///
/// Retries transport failures, and rate limits by **status only** — the body is
/// still unread here, so a 429/503 is dropped and retried without sniffing the
/// message the way the buffered path can.
pub async fn open_stream(
    context: &str,
    build: impl Fn() -> reqwest::RequestBuilder,
) -> Result<reqwest::Response, ApiError> {
    let policy = policy();
    let mut transport_attempt = 0u32;
    let mut rate_limit_attempt = 0u32;

    loop {
        let response = match build().send().await {
            Ok(r) => r,
            Err(e) => {
                if should_retry_transport(&e, transport_attempt, policy.max_retries) {
                    tokio::time::sleep(backoff(transport_attempt, policy.backoff_ms)).await;
                    transport_attempt += 1;
                    continue;
                }
                return Err(transport_error(&e, context));
            }
        };

        let status = response.status().as_u16();
        if rate_limit_attempt < policy.rate_limit_max_retries.saturating_sub(1)
            && (status == 429 || status == 503)
        {
            let delay = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|s| *s >= 0.0)
                .map(|s| Duration::from_secs_f64(s.min(30.0)))
                .unwrap_or_else(|| backoff(rate_limit_attempt, policy.rate_limit_backoff_ms));
            eprintln!(
                "[agent-platformd] rate_limited {context} attempt={}/{} delay={:.2}s status={status}",
                rate_limit_attempt + 1,
                policy.rate_limit_max_retries,
                delay.as_secs_f64(),
            );
            drop(response);
            tokio::time::sleep(delay).await;
            rate_limit_attempt += 1;
            continue;
        }

        return Ok(response);
    }
}

fn copy_headers(from: &reqwest::header::HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::with_capacity(from.len());
    for (name, value) in from {
        out.append(name.clone(), value.clone());
    }
    out
}

// ---------------------------------------------------------------------------
// Retry policy
// ---------------------------------------------------------------------------

/// A refused connect to loopback means nothing is listening on that port.
///
/// Ollama and LM Studio are addressed as loopback, so when one of them is not
/// running, every probe would otherwise burn its whole retry budget waiting for
/// a local app to start within a few hundred milliseconds. It will not. Remote
/// hosts keep retrying: there, a refusal really can be a restarting load balancer.
fn is_loopback_refusal(e: &reqwest::Error) -> bool {
    if !e.is_connect() || e.is_timeout() {
        return false;
    }
    e.url()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|h| matches!(h.as_str(), "127.0.0.1" | "::1" | "localhost"))
}

fn should_retry_transport(e: &reqwest::Error, attempt: u32, max_attempts: u32) -> bool {
    if attempt + 1 >= max_attempts || is_loopback_refusal(e) {
        return false;
    }
    e.is_connect() || e.is_timeout() || e.is_body()
}

/// Vendors reject bursts from many concurrent agents with these statuses, or with
/// a 4xx whose body names the limit (AIML API answers "too many concurrent
/// requests" under a plain 400).
const RATE_LIMIT_PHRASES: [&str; 4] =
    ["too many concurrent", "rate limit", "rate_limit_exceeded", "too many requests"];

pub fn is_rate_limited(status: StatusCode, body: &str) -> bool {
    if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE {
        return true;
    }
    if status.is_client_error() || status.is_server_error() {
        let lowered = body.to_ascii_lowercase();
        return RATE_LIMIT_PHRASES.iter().any(|p| lowered.contains(p));
    }
    false
}

/// Exponential with up to 25% jitter, capped at 30s — the same curve as Python's.
fn backoff(attempt: u32, base_ms: u64) -> Duration {
    let base = (base_ms as f64 / 1000.0) * 2f64.powi(attempt.min(16) as i32);
    Duration::from_secs_f64((base + jitter_fraction() * base * 0.25).min(30.0))
}

/// `random.uniform(0, 1)` without pulling in an RNG crate; a failed draw just
/// costs the jitter, and the backoff underneath it is what actually spaces retries.
fn jitter_fraction() -> f64 {
    let mut bytes = [0u8; 4];
    let _ = getrandom::getrandom(&mut bytes);
    u32::from_le_bytes(bytes) as f64 / u32::MAX as f64
}

// ---------------------------------------------------------------------------
// Error classification
// ---------------------------------------------------------------------------

fn transport_error(e: &reqwest::Error, context: &str) -> ApiError {
    let (code, message) = classify_with_context(e, context);
    ApiError::coded(StatusCode::BAD_GATEWAY, code, message)
}

fn classify_code(e: &reqwest::Error) -> &'static str {
    if e.is_builder() {
        return "bad_url";
    }
    if e.is_timeout() {
        return if e.is_connect() { "connect_timeout" } else { "read_timeout" };
    }
    if e.is_connect() {
        return "connect_failed";
    }
    if e.is_body() || e.is_decode() {
        return "protocol_error";
    }
    "transport_error"
}

/// `(machine_code, human_message)` — the message carries the sanitized URL,
/// because the first question about a proxy failure is always "which upstream".
pub fn classify_with_context(e: &reqwest::Error, context: &str) -> (&'static str, String) {
    let raw = e.url().map(Url::as_str).unwrap_or("");
    let safe = if raw.is_empty() { "(unknown url)".to_string() } else { sanitize_url(raw) };
    let code = classify_code(e);
    let message = match code {
        "connect_failed" => {
            let hint = if safe.to_ascii_lowercase().contains("ollama") || safe.contains(":11434") {
                "Is Ollama running?"
            } else {
                "Check the URL and network."
            };
            format!("Cannot reach upstream at {safe} (connection failed). {hint}")
        }
        "connect_timeout" => format!("Connection to upstream timed out: {safe}"),
        "read_timeout" => format!("Upstream read timed out ({context}): {safe}"),
        "protocol_error" => format!("Upstream closed the connection unexpectedly ({safe})."),
        "bad_url" => format!("Invalid or unsupported URL: {safe}"),
        _ => format!("Upstream request failed ({context}): {safe}"),
    };
    (code, message)
}

/// Redact credential-bearing query parameters. Gemini takes its key as `?key=`,
/// so an unsanitized log line is a leaked API key.
pub fn sanitize_url(raw: &str) -> String {
    let Ok(mut url) = Url::parse(raw) else {
        return raw.to_string();
    };
    if url.query().is_none() {
        return raw.to_string();
    }
    let redacted: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| {
            let secret = matches!(
                k.to_ascii_lowercase().as_str(),
                "key" | "api_key" | "token" | "access_token"
            );
            (k.into_owned(), if secret { "***".to_string() } else { v.into_owned() })
        })
        .collect();
    url.query_pairs_mut().clear().extend_pairs(redacted);
    url.to_string()
}

/// One `data:` frame carrying the OpenAI error shape, for a stream that fails
/// after its headers have already gone out.
pub fn sse_error_chunk(code: &str, message: &str) -> Vec<u8> {
    let payload = serde_json::json!({
        "error": { "message": message, "type": crate::error::ERROR_TYPE, "param": null, "code": code }
    });
    format!("data: {payload}\n\n").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_stripped_from_logged_urls() {
        // Python writes `%2A%2A%2A` here: its `quote_plus` escapes `*`, the URL
        // spec's form serializer does not. Log text either way, never a body.
        assert_eq!(
            sanitize_url("https://generativelanguage.googleapis.com/v1beta/models/x?key=sk-secret"),
            "https://generativelanguage.googleapis.com/v1beta/models/x?key=***"
        );
        assert!(!sanitize_url("https://x.test/m?api_key=abc&q=1").contains("abc"));
        // Untouched when there is nothing to hide.
        assert_eq!(sanitize_url("http://127.0.0.1:11434/api/tags"), "http://127.0.0.1:11434/api/tags");
        assert_eq!(sanitize_url("not a url"), "not a url");
    }

    #[test]
    fn rate_limits_are_recognised_by_status_or_message() {
        assert!(is_rate_limited(StatusCode::TOO_MANY_REQUESTS, ""));
        assert!(is_rate_limited(StatusCode::SERVICE_UNAVAILABLE, ""));
        // AIML API answers a plain 400 whose body names the limit.
        assert!(is_rate_limited(StatusCode::BAD_REQUEST, "Too Many Concurrent Requests"));
        assert!(!is_rate_limited(StatusCode::BAD_REQUEST, "model not found"));
        assert!(!is_rate_limited(StatusCode::OK, "rate limit"));
    }

    #[test]
    fn backoff_grows_and_stays_bounded() {
        // 120ms base: ~0.12s, ~0.24s, ~0.48s, each up to 25% over.
        for attempt in 0..4u32 {
            let d = backoff(attempt, 120).as_secs_f64();
            let base = 0.12 * 2f64.powi(attempt as i32);
            assert!(d >= base && d <= base * 1.25 + 1e-9, "attempt {attempt}: {d}");
        }
        assert!(backoff(30, 400).as_secs_f64() <= 30.0);
    }
}
