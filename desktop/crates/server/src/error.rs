//! The error envelope every JSON route answers with, mirroring
//! `app/llm_proxy/core/errors.py`. External callers branch on `code`, so the
//! status→code mapping is part of the contract, not a formatting choice.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

pub const ERROR_TYPE: &str = "llm_proxy_error";

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub extra: Option<Value>,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self { status, code: code_for(status), message: message.into(), extra: None }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    /// The 422 shape FastAPI's `RequestValidationError` handler produces.
    ///
    /// ponytail: `extra.errors` carries `{type, loc, msg}` per failure, not
    /// pydantic's full entry (no `input`, no `ctx`). Status, code and message
    /// match; the per-error detail is best-effort, and no known consumer reads
    /// past `msg`.
    pub fn validation(errors: Vec<Value>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "validation_error",
            message: "Request validation failed".into(),
            extra: Some(json!({ "errors": errors })),
        }
    }

    /// One pydantic-style entry for a string-length violation on a body field.
    pub fn field_error(field: &str, kind: &'static str, msg: &str) -> Value {
        json!({ "type": kind, "loc": ["body", field], "msg": msg })
    }
}

fn code_for(status: StatusCode) -> &'static str {
    match status.as_u16() {
        400 => "bad_request",
        401 => "unauthorized",
        403 => "forbidden",
        404 => "not_found",
        500 => "internal_error",
        502 => "bad_gateway",
        503 => "service_unavailable",
        _ => "http_error",
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut err = json!({
            "message": self.message,
            "type": ERROR_TYPE,
            "code": self.code,
        });
        if let Some(extra) = self.extra {
            err["extra"] = extra;
        }
        // ponytail: no `request_id` — Python's middleware stamps one and this
        // server has no such middleware yet. Add both together.
        (self.status, axum::Json(json!({ "error": err }))).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        eprintln!("[agent-platformd] database error: {e}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "An unexpected error occurred.")
    }
}
