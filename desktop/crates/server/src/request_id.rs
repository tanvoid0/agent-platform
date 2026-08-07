//! Request correlation id, ported from `app/llm_proxy/core/middleware.py`.
//!
//! One id per request: taken from an incoming `X-Request-ID` or generated, put
//! back on the response, and written into every error envelope — the field
//! Python's handlers add through `get_request_id`.
//!
//! It is also written onto the *request* before the fallback runs, so the proxied
//! half of the app adopts the same id: Python's own middleware prefers an
//! incoming header over a fresh uuid, so one request reads the same in both
//! servers' logs instead of two unrelated ids for the same call.
//!
//! Handlers never see it. `ApiError` and `AuthError` build their bodies inside
//! the handler's task, which is why a task-local reaches them without threading
//! an extractor through every signature.

use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

pub const HEADER: HeaderName = HeaderName::from_static("x-request-id");

tokio::task_local! {
    static REQUEST_ID: String;
}

/// The id of the request being served, if this task is serving one.
pub fn current() -> Option<String> {
    REQUEST_ID.try_with(String::clone).ok()
}

pub async fn middleware(mut req: Request, next: Next) -> Response {
    let id = req
        .headers()
        .get(HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(uuid_v4);

    // Only fails if a caller sent bytes that cannot round-trip a header value,
    // in which case the generated id is used for our half and Python makes its own.
    if let Ok(value) = HeaderValue::from_str(&id) {
        req.headers_mut().insert(HEADER, value.clone());
        let mut response = REQUEST_ID.scope(id, next.run(req)).await;
        response.headers_mut().insert(HEADER, value);
        return response;
    }
    next.run(req).await
}

/// A v4 uuid in the shape `str(uuid.uuid4())` produces.
fn uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    // A failed draw degrades correlation, not safety — this id authorizes
    // nothing — so it is not worth failing a request over.
    let _ = getrandom::getrandom(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_look_like_uuid4() {
        let id = uuid_v4();
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), vec![8, 4, 4, 4, 12]);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'), "{id}");
        assert_eq!(parts[2].as_bytes()[0], b'4', "version nibble");
        assert!(matches!(parts[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'), "variant nibble");
        assert_ne!(uuid_v4(), id);
    }
}
