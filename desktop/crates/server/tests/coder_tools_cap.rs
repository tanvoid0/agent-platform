//! The caps on a caller-supplied `tools` list, on every route that reads one.
//!
//! `/send` and `/stream` validate through `SendRequest::validate`; `/retry` and
//! `/approve` build their errors field by field, because neither has a
//! `message` to require. That split is exactly how a limit ends up existing on
//! two routes out of four, and it did — `validate_tools` had to be called
//! twice more by hand. This is the check that notices if either call goes away.
//!
//! Every case here is rejected *before* the handler reaches the database or the
//! model, which is why no seeding and no upstream are needed.

mod common;

use serde_json::{json, Value};

use common::MASTER;

fn temp_db_path() -> std::path::PathBuf {
    common::temp_db_path("tools-cap")
}

async fn start_server(db: &std::path::Path) -> String {
    common::start_server(db, Some(MASTER)).await
}

async fn post(origin: &str, path: &str, body: Value) -> (u16, String) {
    let resp = reqwest::Client::new()
        .post(format!("{origin}{path}"))
        .bearer_auth(MASTER)
        // The header that turns delegation on, because a caller-supplied tool
        // list is only meaningful to a delegating client.
        .header("X-Agent-Platform-Client", "portal-desktop")
        .json(&body)
        .send()
        .await
        .unwrap();
    (resp.status().as_u16(), resp.text().await.unwrap())
}

/// The `loc` of the first field error, dotted — `body.tools` for these.
///
/// The envelope is this crate's, not pydantic's bare `detail`: field errors
/// live at `error.extra.errors`. Reading the wrong path here returns `<none>`
/// for *every* body, which makes a "did it reject?" assertion pass by accident
/// — it did, on the first draft of this file.
fn first_error_loc(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    match &v["error"]["extra"]["errors"][0]["loc"] {
        Value::Array(parts) => {
            parts.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(".")
        }
        _ => "<none>".to_string(),
    }
}

/// Bodies that satisfy each route's *other* required fields, so the only thing
/// left to reject is `tools`.
fn routes() -> [(&'static str, Value); 5] {
    [
        ("/api/v1/coder/chat/send", json!({"message": "hi", "thread_id": 1})),
        ("/api/v1/coder/chat/stream", json!({"message": "hi", "thread_id": 1})),
        ("/api/v1/coder/chat/retry", json!({"thread_id": 1})),
        (
            "/api/v1/coder/chat/approve",
            json!({"thread_id": 1, "call_id": "c1", "approve": true}),
        ),
        // The JSON twin shares `approval_request` with the streaming one, which
        // is exactly why it is listed: the caps live on a call both make, and a
        // route that stopped making it would look fine until a caller sent 65
        // tool specs to it.
        (
            "/api/v1/coder/chat/approve/send",
            json!({"thread_id": 1, "call_id": "c1", "approve": true}),
        ),
    ]
}

fn with_tools(base: &Value, tools: Value) -> Value {
    let mut body = base.clone();
    body["tools"] = tools;
    body
}

#[tokio::test]
async fn every_coder_route_caps_a_caller_supplied_tool_list() {
    let db = temp_db_path();
    let origin = start_server(&db).await;

    let spec = json!({"type": "function", "function": {"name": "t"}});
    let too_many = Value::Array(vec![spec.clone(); 65]);
    let not_objects = json!(["read_file"]);
    // Eight entries — under the count cap — of 20 KB each, so only the byte cap
    // can reject this one.
    let too_fat = Value::Array(vec![json!({"pad": "x".repeat(20_000)}); 8]);

    for (path, base) in routes() {
        for (label, tools) in [
            ("65 entries", too_many.clone()),
            ("a non-object entry", not_objects.clone()),
            ("160 KB", too_fat.clone()),
        ] {
            let (status, body) = post(&origin, path, with_tools(&base, tools)).await;
            assert_eq!(status, 422, "{path} accepted {label}: {body}");
            assert_eq!(first_error_loc(&body), "body.tools", "{path} / {label}: {body}");
        }
    }

    let _ = std::fs::remove_file(&db);
}

/// The other half: a list within the caps is not rejected *by this check*. It
/// goes on to fail for a reason of its own (there is no thread 1, and no model
/// behind the proxy) — the assertion is only that it is not a 422 naming
/// `tools`.
#[tokio::test]
async fn a_legal_tool_list_passes_validation() {
    let db = temp_db_path();
    let origin = start_server(&db).await;
    let tools = json!([{"type": "function", "function": {"name": "read_file"}}]);

    for (path, base) in routes() {
        let (_, body) = post(&origin, path, with_tools(&base, tools.clone())).await;
        assert_ne!(first_error_loc(&body), "body.tools", "{path} rejected a legal list: {body}");
    }

    // And an empty list is legal too — it means a tool-free turn, not the
    // default set.
    for (path, base) in routes() {
        let (_, body) = post(&origin, path, with_tools(&base, json!([]))).await;
        assert_ne!(first_error_loc(&body), "body.tools", "{path} rejected []: {body}");
    }

    let _ = std::fs::remove_file(&db);
}
