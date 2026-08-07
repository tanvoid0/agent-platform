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
//!
//! [`middleware`] also emits one JSON log line per request, in the same shape
//! as `app/observability.py`'s `JsonLogFormatter` — the Logs screen's parser
//! and its trace-id filter treat both servers' output as one stream, so a
//! request served by *this* half must log the same fields Python does or a
//! trace id from a migrated domain filters to nothing.

use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use serde_json::json;
use std::time::Instant;

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

    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let started = Instant::now();

    // Only fails if a caller sent bytes that cannot round-trip a header value,
    // in which case the generated id is used for our half and Python makes its own.
    let response = if let Ok(value) = HeaderValue::from_str(&id) {
        req.headers_mut().insert(HEADER, value.clone());
        let mut response = REQUEST_ID.scope(id.clone(), next.run(req)).await;
        response.headers_mut().insert(HEADER, value);
        response
    } else {
        next.run(req).await
    };

    log_request(&id, &method, &path, response.status().as_u16(), started.elapsed());
    response
}

fn log_request(request_id: &str, method: &str, path: &str, status: u16, elapsed: std::time::Duration) {
    let line = json!({
        "timestamp": iso_now(),
        "level": "INFO",
        "logger": "agent_platform.request",
        "message": "request completed",
        "request_id": request_id,
        "event": "request.completed",
        "method": method,
        "path": path,
        "route": path,
        "status_code": status,
        "duration_ms": elapsed.as_millis(),
    });
    let line = line.to_string();
    println!("{line}");
    // …and into the ring `GET /system/logs` serves, which is where Python's
    // root-logger handler picked the same line up.
    crate::observability::record(&line);
}

/// RFC3339 with the same shape `datetime.isoformat()` produces
/// (`…+00:00`, not `…Z`), so `logs.rs`'s `clock()` splits it identically.
pub fn iso_now() -> String {
    let now = std::time::SystemTime::now();
    let unix = now.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = unix.as_secs();
    let micros = unix.subsec_micros();
    let days = secs / 86_400;
    let time_of_day = secs % 86_400;
    let (h, m, s) = (time_of_day / 3600, (time_of_day % 3600) / 60, time_of_day % 60);
    let (y, mo, d) = civil_from_days(days as i64);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}.{micros:06}+00:00")
}

/// Howard Hinnant's `civil_from_days`: days-since-epoch → (year, month, day),
/// proleptic Gregorian. No `chrono` dependency for one timestamp format.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
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
