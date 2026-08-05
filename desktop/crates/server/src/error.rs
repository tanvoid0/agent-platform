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

    /// One pydantic-style entry for a violation on a body field.
    pub fn field_error(field: &str, kind: &'static str, msg: &str) -> Value {
        Self::field_error_at(&[field], kind, msg)
    }

    /// Same, for a field nested inside the body (`["roster", "roles", "0", "id"]`).
    pub fn field_error_at(path: &[&str], kind: &str, msg: &str) -> Value {
        let mut loc = vec![Value::from("body")];
        loc.extend(path.iter().map(|p| Value::from(*p)));
        json!({ "type": kind, "loc": loc, "msg": msg })
    }
}

/// `Path`, but rejecting like FastAPI does.
///
/// axum answers `/todos/items/abc` with a plain-text 400 (`Cannot parse "abc"
/// to a i64`); FastAPI answers 422 with the validation envelope. Every migrated
/// route with an id in its path would otherwise differ on the same request.
pub struct PathId<T>(pub T);

impl<T, S> axum::extract::FromRequestParts<S> for PathId<T>
where
    T: serde::de::DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        use axum::extract::rejection::PathRejection;
        use axum::extract::{path::ErrorKind, Path};

        match Path::<T>::from_request_parts(parts, state).await {
            Ok(Path(value)) => Ok(PathId(value)),
            Err(PathRejection::FailedToDeserializePathParams(err)) => {
                let key = match err.kind() {
                    ErrorKind::ParseErrorAtKey { key, .. }
                    | ErrorKind::InvalidUtf8InPathParam { key }
                    | ErrorKind::DeserializeError { key, .. } => key.clone(),
                    // A single path param deserializes without a key, so the
                    // name comes from the route pattern instead — otherwise the
                    // caller is told "path" where FastAPI names the parameter.
                    _ => last_path_param(parts).unwrap_or_else(|| "path".to_string()),
                };
                Err(ApiError::validation(vec![json!({
                    "type": "int_parsing",
                    "loc": ["path", key],
                    "msg": "Input should be a valid integer, unable to parse string as an integer",
                })]))
            }
            Err(other) => Err(ApiError::new(other.status(), other.body_text())),
        }
    }
}

/// `"/api/v1/todos/items/{item_id}"` → `"item_id"`.
fn last_path_param(parts: &axum::http::request::Parts) -> Option<String> {
    let matched = parts.extensions.get::<axum::extract::MatchedPath>()?;
    matched
        .as_str()
        .rsplit('/')
        .find_map(|seg| seg.strip_prefix('{')?.strip_suffix('}').map(str::to_string))
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
