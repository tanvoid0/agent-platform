//! `POST /api/v1/chat`, ported from `app/chat_routes.py` (ADR 0007).
//!
//! Python's route owns no completion logic: it shapes a body and POSTs it to its
//! own `/v1/chat/completions` over loopback with the master key as the bearer.
//! Rust already serves that route, so the completion here *is*
//! [`crate::llm::chat_completions`], called in-process with an unrestricted
//! principal and no request headers — which is exactly what the loopback hop
//! produced (master key ⇒ unrestricted caller; only `Content-Type` and
//! `Authorization` were sent, so BYOK never applied). Streaming, the buffered
//! path, the retry policy and the mid-stream error frame all come from there and
//! are not reimplemented.
//!
//! What is left, and all this file does:
//!
//! - **`chat:write`** — the one route in the assistant+chat half that checks a
//!   scope.
//! - **503 when the master key is unset**, before anything else. A handler that
//!   calls into `llm` needs no key, but the status is user-visible.
//! - **The request shaping** Python does: `fit_chat_messages_for_request`, the
//!   model-alias sanitiser, a lowercased `provider` hint, and `max_tokens`
//!   defaulted from the context budget.
//! - **The concurrency cap** (`AGENT_PLATFORM_CHAT_MAX_CONCURRENT`, default 8),
//!   held across the whole SSE body rather than just the request — a slow reader
//!   still occupies a slot.
//!
//! Two ceilings, both from having no pydantic:
//!
//! ponytail: only `messages`, `model` and `provider` are type-checked. Python
//! 422s a wrongly-typed `temperature`/`tools`/`stream`/…; those are forwarded
//! here, and the upstream (or `llm.rs`'s own field checks) rejects them with a
//! 400 instead. Add the rest of the table if a client is ever seen sending them.
//!
//! ponytail: no lax coercion either — pydantic turns `"stream": "true"` into a
//! bool before the proxy sees it, where this forwards the string.

use std::sync::{Arc, OnceLock};

use axum::extract::State;
use axum::http::{header::CONTENT_TYPE, HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use futures::channel::mpsc;
use futures::StreamExt;
use serde_json::{json, Map, Value};

use crate::auth::{Principal, ProxyPrincipal};
use crate::context_budget::{fit_chat_messages_for_request, max_output_tokens_default};
use crate::dag_schema::sanitize_llm_model_alias;
use crate::error::ApiError;
use crate::wire::parse_body;
use crate::{env_opt, AppState};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/chat", post(chat))
}

/// Single-turn OpenAI-compatible chat completion.
///
/// Creates no Process; `POST /api/v1/processes` is the multi-agent path.
async fn chat(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    // FastAPI validates the body during dependency solving, so a malformed one
    // 422s before `require_scope` is ever reached. Same order here. Read as raw
    // `Bytes`, not `Option<Json<Value>>`: axum's `Json` extractor only yields
    // `None` when `Content-Type` is absent — an empty body *with*
    // `application/json` set (what an argument-less POST from most clients
    // looks like) fails to parse and axum answers its own plain-text 400
    // before this handler runs, never the 422 envelope the comment above
    // promises.
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let body = parse_body(&body)?;
    let payload = payload_from(body)?;

    principal.require_scope("chat:write")?;
    if state.master_key.is_none() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "AGENT_PLATFORM_MASTER_KEY is not set.",
        ));
    }

    let permit = limiter().acquire().await;
    let response = crate::llm::chat_completions(
        ProxyPrincipal(Principal::unrestricted()),
        State(state),
        HeaderMap::new(),
        Value::Object(payload).to_string().into_bytes().into(),
    )
    .await?;

    // An upstream that refused before streaming came back buffered, under its
    // own content type; that response is finished, so the slot is free at the
    // end of this function the way Python's `finally` releases it.
    if !is_event_stream(&response) {
        return Ok(response);
    }

    // A live stream still owes bytes, so the permit rides along inside it and is
    // released when the body is dropped — not when this handler returns.
    let (parts, body) = response.into_parts();
    let body = axum::body::Body::from_stream(body.into_data_stream().map(move |chunk| {
        let _slot = &permit;
        chunk
    }));
    Ok(Response::from_parts(parts, body))
}

fn is_event_stream(response: &Response) -> bool {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"))
}

// ---------------------------------------------------------------------------
// Request shaping
// ---------------------------------------------------------------------------

/// A declared-`str` field. Absent and `null` both read as absent; a non-string
/// adds the 422 entry pydantic answers with. Errors are collected rather than
/// raised, because one request produces one 422 listing all of them.
fn optional_str(
    req: &Map<String, Value>,
    key: &'static str,
    errors: &mut Vec<Value>,
) -> Option<String> {
    match req.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        None | Some(Value::Null) => None,
        Some(_) => {
            errors.push(ApiError::field_error(key, "string_type", "Input should be a valid string"));
            None
        }
    }
}

/// `ChatCompletionRequest` → the body posted to `/v1/chat/completions`.
///
/// Key order is Python's insertion order, and `extra="ignore"` means anything
/// not named here is dropped rather than forwarded.
fn payload_from(body: Value) -> Result<Map<String, Value>, ApiError> {
    let Value::Object(req) = body else {
        return Err(ApiError::validation(vec![json!({
            "type": "model_attributes_type",
            "loc": ["body"],
            "msg": "Input should be a valid dictionary or object to extract fields from",
        })]));
    };

    let mut errors: Vec<Value> = Vec::new();
    let field = |kind, msg| ApiError::field_error("messages", kind, msg);
    let messages = match req.get("messages") {
        // Required and not `Optional`, so a null is a type error, not a missing
        // field.
        None => {
            errors.push(field("missing", "Field required"));
            Vec::new()
        }
        Some(Value::Array(m)) if m.is_empty() => {
            errors.push(field("value_error", "Value error, messages must be a non-empty list"));
            Vec::new()
        }
        Some(Value::Array(m)) => m.clone(),
        Some(_) => {
            errors.push(field("list_type", "Input should be a valid list"));
            Vec::new()
        }
    };
    let model = optional_str(&req, "model", &mut errors);
    let provider = optional_str(&req, "provider", &mut errors);
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    let (fitted, _budget) = fit_chat_messages_for_request(messages);
    let mut payload = Map::new();
    payload.insert("messages".into(), Value::Array(fitted));

    // A role slug the planner mistook for an alias sanitises to nothing, and
    // then the proxy picks the default — same as an omitted model.
    if let Some(model) = model.and_then(|m| sanitize_llm_model_alias(m.trim())) {
        payload.insert("model".into(), json!(model));
    }
    // The proxy validates the hint and routes to it; an unknown one is a 400
    // from there, not from here.
    if let Some(provider) = provider.filter(|p| !p.trim().is_empty()) {
        payload.insert("provider".into(), json!(provider.trim().to_lowercase()));
    }

    let copy = |payload: &mut Map<String, Value>, key: &str| {
        if let Some(value) = req.get(key).filter(|v| !v.is_null()) {
            payload.insert(key.into(), value.clone());
        }
    };
    for key in ["tools", "tool_choice", "temperature"] {
        copy(&mut payload, key);
    }
    // The one field with a default: an unbounded completion would blow the
    // context window it was just fitted to.
    let max_tokens = req
        .get("max_tokens")
        .filter(|v| !v.is_null())
        .cloned()
        .unwrap_or_else(|| json!(max_output_tokens_default()));
    payload.insert("max_tokens".into(), max_tokens);
    for key in ["top_p", "response_format", "stream"] {
        copy(&mut payload, key);
    }

    Ok(payload)
}

// ---------------------------------------------------------------------------
// Concurrency cap
// ---------------------------------------------------------------------------

/// Requests in flight to the upstream at once. Many simulated agents fire chat
/// calls in the same tick; this queues them rather than letting them all bounce
/// off the upstream's own rate limiting.
fn parse_max_concurrent(raw: Option<&str>) -> usize {
    let Some(raw) = raw.map(str::trim).filter(|r| !r.is_empty()) else {
        return 8;
    };
    // `int(raw)` then `max(1, …)`: a non-integer falls back to the default, a
    // zero or negative one clamps to a single slot.
    raw.parse::<i64>().map_or(8, |n| n.max(1) as usize)
}

/// Module-level, like Python's `_llm_semaphore` — read once, on first request.
///
/// A permit pool over a bounded channel rather than a semaphore, because
/// **`tokio`'s `sync` feature is not compiled into this crate** (see
/// `Cargo.toml`, and `executor.rs`'s `futures::lock::Mutex` for the same
/// choice). Tokens are the channel's contents; the receiver behind a fair mutex
/// is the queue.
struct Limiter {
    tx: mpsc::Sender<()>,
    rx: futures::lock::Mutex<mpsc::Receiver<()>>,
}

/// Returns its token on drop, so every exit path — early return, `?`, a dropped
/// response body, a cancelled request — releases exactly once.
struct Permit(mpsc::Sender<()>);

impl Drop for Permit {
    fn drop(&mut self) {
        // Cannot fail: the channel buffers as many tokens as were ever minted,
        // and this one came out of it.
        let _ = self.0.try_send(());
    }
}

impl Limiter {
    fn new(permits: usize) -> Self {
        let (mut tx, rx) = mpsc::channel(permits);
        for _ in 0..permits {
            let _ = tx.try_send(());
        }
        Self { tx, rx: futures::lock::Mutex::new(rx) }
    }

    async fn acquire(&self) -> Permit {
        // One waiter holds the receiver at a time, so waiters are served in
        // arrival order instead of racing for the next token. `None` is
        // unreachable: the sender is owned by this `'static` Limiter.
        let _token = self.rx.lock().await.next().await;
        Permit(self.tx.clone())
    }
}

fn limiter() -> &'static Limiter {
    static LIMITER: OnceLock<Limiter> = OnceLock::new();
    LIMITER.get_or_init(|| {
        Limiter::new(parse_max_concurrent(
            env_opt("AGENT_PLATFORM_CHAT_MAX_CONCURRENT").as_deref(),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrency_cap_falls_back_the_way_python_does() {
        assert_eq!(parse_max_concurrent(None), 8);
        assert_eq!(parse_max_concurrent(Some("  ")), 8, "blank reads as unset");
        assert_eq!(parse_max_concurrent(Some("2")), 2);
        assert_eq!(parse_max_concurrent(Some(" 3 ")), 3, "int() strips");
        assert_eq!(parse_max_concurrent(Some("8.5")), 8, "ValueError → the default");
        assert_eq!(parse_max_concurrent(Some("nope")), 8);
        assert_eq!(parse_max_concurrent(Some("0")), 1, "max(1, …)");
        assert_eq!(parse_max_concurrent(Some("-3")), 1);
    }

    fn errors(e: &ApiError) -> Vec<Value> {
        assert_eq!(e.status, StatusCode::UNPROCESSABLE_ENTITY);
        e.extra.as_ref().unwrap()["errors"].as_array().unwrap().clone()
    }

    #[test]
    fn messages_must_be_a_non_empty_list() {
        let err = payload_from(json!({})).unwrap_err();
        assert_eq!(errors(&err)[0]["type"], "missing");

        let err = payload_from(json!({"messages": null})).unwrap_err();
        assert_eq!(errors(&err)[0]["type"], "list_type");

        let err = payload_from(json!({"messages": "hi"})).unwrap_err();
        assert_eq!(errors(&err)[0]["loc"], json!(["body", "messages"]));

        let err = payload_from(json!({"messages": []})).unwrap_err();
        assert_eq!(errors(&err)[0]["msg"], "Value error, messages must be a non-empty list");

        // The body itself has to be an object before any field is read.
        let err = payload_from(json!([1])).unwrap_err();
        assert_eq!(errors(&err)[0]["loc"], json!(["body"]));
    }

    #[test]
    fn a_wrongly_typed_model_is_a_422_not_a_silent_default() {
        let err = payload_from(json!({"messages": [{"role": "user"}], "model": 7})).unwrap_err();
        assert_eq!(errors(&err)[0]["type"], "string_type");
    }

    #[test]
    fn the_payload_is_pythons_fields_in_pythons_order() {
        let payload = payload_from(json!({
            "messages": [{"role": "user", "content": "hi"}],
            "model": "  gpt-4o  ",
            "provider": " Ollama ",
            "tools": [],
            "tool_choice": "auto",
            "temperature": 0.2,
            "top_p": 1,
            "response_format": {"type": "json_object"},
            "stream": true,
            "seen_but_ignored": 1,
        }))
        .unwrap();

        assert_eq!(
            payload.keys().collect::<Vec<_>>(),
            [
                "messages",
                "model",
                "provider",
                "tools",
                "tool_choice",
                "temperature",
                "max_tokens",
                "top_p",
                "response_format",
                "stream",
            ]
        );
        assert_eq!(payload["model"], "gpt-4o", "trimmed by the alias sanitiser");
        assert_eq!(payload["provider"], "ollama");
        assert_eq!(payload["messages"].as_array().unwrap().len(), 1);
        // Not a caller's field: the budget fills it in.
        assert!(payload["max_tokens"].as_i64().unwrap() > 0);
    }

    #[test]
    fn omitted_fields_stay_omitted_and_a_role_slug_is_dropped() {
        let payload = payload_from(json!({
            "messages": [{"role": "user", "content": "hi"}],
            // Not an alias the proxy can resolve.
            "model": "typescript-expert",
            "provider": "   ",
            "temperature": null,
            "max_tokens": 32,
        }))
        .unwrap();

        assert_eq!(payload.keys().collect::<Vec<_>>(), ["messages", "max_tokens"]);
        assert_eq!(payload["max_tokens"], 32, "the caller's value wins over the default");
    }

    /// The permit has to come back on drop, or the route wedges after N streams
    /// — the leak `test_chat_stream.py` guards on the Python side.
    #[test]
    fn permits_are_returned_when_they_drop() {
        futures::executor::block_on(async {
            let limiter = Limiter::new(2);
            let first = limiter.acquire().await;
            let second = limiter.acquire().await;
            // Both slots are out: a third acquire cannot complete yet.
            assert!(futures::poll!(Box::pin(limiter.acquire())).is_pending());

            drop(first);
            drop(second);
            let _third = limiter.acquire().await;
            let _fourth = limiter.acquire().await;
        });
    }
}
