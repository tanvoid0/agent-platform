//! Reverse proxy to the Python server for every route this server has not
//! migrated (ADR 0007). Also the permanent home of the domains that stay Python:
//! the MCP client, the model-ops training pipeline, PDF extraction.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::AppState;

/// Dropped in both directions: framing and connection headers describe *this*
/// hop, and re-sending them across the next one produces a body length that
/// disagrees with the body.
const HOP_BY_HOP: [HeaderName; 7] = [
    header::CONNECTION,
    header::TRANSFER_ENCODING,
    header::CONTENT_LENGTH,
    header::UPGRADE,
    header::TE,
    header::TRAILER,
    HeaderName::from_static("keep-alive"),
];

pub async fn forward(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| "/".into());
    let url = format!("{}{}", state.upstream.origin, path_and_query);

    let (parts, body) = req.into_parts();

    // The caller's `host` is forwarded deliberately. FastAPI builds redirect
    // targets from it — a trailing-slash 307 with the child's host in it sends
    // the client straight at the ephemeral port, around this proxy and at a
    // number that changes every restart.

    // Streamed, not buffered: SSE is the whole reason. `bytes_stream` on the way
    // back does the same for the response.
    let upstream_req = state
        .http
        .request(parts.method, &url)
        .headers(sanitize(&parts.headers))
        .body(reqwest::Body::wrap_stream(body.into_data_stream()));

    let resp = match upstream_req.send().await {
        Ok(r) => r,
        Err(e) => return upstream_unavailable(&e).into_response(),
    };

    let mut out = Response::builder().status(resp.status());
    if let Some(headers) = out.headers_mut() {
        *headers = sanitize(resp.headers());
    }
    out.body(Body::from_stream(resp.bytes_stream()))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

fn sanitize(headers: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::with_capacity(headers.len());
    for (name, value) in headers {
        if HOP_BY_HOP.contains(name) {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}

fn upstream_unavailable(e: &reqwest::Error) -> impl IntoResponse {
    eprintln!("[agent-platformd] upstream request failed: {e}");
    (
        StatusCode::BAD_GATEWAY,
        axum::Json(json!({
            "error": {
                "message": "The platform server is not reachable.",
                "type": "llm_proxy_error",
                "code": "UPSTREAM_UNAVAILABLE",
            }
        })),
    )
}
