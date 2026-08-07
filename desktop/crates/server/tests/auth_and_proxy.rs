//! The check that fails if auth stops matching `app/api_tokens/auth.py`, or if
//! the proxy stops forwarding. Runs against a stub upstream — no Python needed.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use agent_platform_server::auth::hash_token;
use agent_platform_server::upstream::Upstream;
use agent_platform_server::{router, AppState};
use axum::extract::Request;
use axum::Router;
use futures::StreamExt;
use serde_json::Value;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Executor, SqlitePool};

const MASTER: &str = "master-key-under-test";

static SEQ: AtomicU32 = AtomicU32::new(0);

fn temp_db_path() -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    std::env::temp_dir().join(format!("agp-server-test-{pid}-{n}.db"))
}

/// The columns auth reads, with the names Alembic gives them.
async fn seed_db(path: &PathBuf) {
    let _ = std::fs::remove_file(path);
    let pool = SqlitePool::connect_with(
        SqliteConnectOptions::new().filename(path).create_if_missing(true),
    )
    .await
    .unwrap();

    pool.execute(
        "CREATE TABLE workspace (id INTEGER PRIMARY KEY, archived_at DATETIME);
         CREATE TABLE api_tokens (
            id INTEGER PRIMARY KEY,
            workspace_id INTEGER NOT NULL,
            prefix TEXT NOT NULL,
            token_hash TEXT NOT NULL,
            scopes_json TEXT NOT NULL DEFAULT '[]',
            status TEXT NOT NULL DEFAULT 'active',
            held_reason TEXT,
            rate_limit_per_minute INTEGER,
            expires_at DATETIME,
            last_used_at DATETIME
         );
         INSERT INTO workspace (id, archived_at) VALUES (1, NULL), (2, '2026-01-01 00:00:00.000000');",
    )
    .await
    .unwrap();

    // (raw token, workspace, status, held_reason, expires_at, rate limit)
    let rows: [(&str, i64, &str, Option<&str>, Option<&str>, Option<i64>); 6] = [
        ("agp_live_good", 1, "active", None, None, None),
        ("agp_live_revoked", 1, "revoked", None, None, None),
        ("agp_live_held", 1, "held", Some("Billing on hold."), None, None),
        ("agp_live_expired", 1, "active", None, Some("2020-01-01 00:00:00.000000"), None),
        ("agp_live_archived", 2, "active", None, None, None),
        ("agp_live_limited", 1, "active", None, None, Some(1)),
    ];
    for (raw, workspace, status, held, expires, limit) in rows {
        sqlx::query(
            "INSERT INTO api_tokens
             (workspace_id, prefix, token_hash, scopes_json, status, held_reason, rate_limit_per_minute, expires_at)
             VALUES (?, ?, ?, '[\"*\"]', ?, ?, ?, ?)",
        )
        .bind(workspace)
        .bind(&raw[..12])
        .bind(hash_token(raw))
        .bind(status)
        .bind(held)
        .bind(limit)
        .bind(expires)
        .execute(&pool)
        .await
        .unwrap();
    }
    pool.close().await;
}

/// Echoes what it was asked, so the proxy assertions can see method and path.
/// `/sse` emits one chunk, stalls, then emits another — the shape that catches a
/// proxy which buffers a whole response before answering.
async fn stub_upstream() -> String {
    let app = Router::new()
        .route("/sse", axum::routing::get(sse_stub))
        .fallback(|req: Request| async move {
            format!("{} {}", req.method(), req.uri().path_and_query().unwrap())
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    origin
}

async fn sse_stub() -> axum::response::Response {
    let stream = futures::stream::once(async { Ok::<_, std::io::Error>("data: first\n\n") })
        .chain(futures::stream::once(async {
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            Ok::<_, std::io::Error>("data: second\n\n")
        }));
    axum::response::Response::builder()
        .header("content-type", "text/event-stream")
        .body(axum::body::Body::from_stream(stream))
        .unwrap()
}

async fn start_server(db: &PathBuf, master_key: Option<&str>) -> String {
    let upstream = Arc::new(Upstream::attached(stub_upstream().await));
    let state = Arc::new(AppState::new(db, master_key.map(str::to_owned), upstream));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });
    origin
}

async fn get(origin: &str, path: &str, bearer: Option<&str>) -> (u16, String) {
    let mut req = reqwest::Client::new().get(format!("{origin}{path}"));
    if let Some(b) = bearer {
        req = req.bearer_auth(b);
    }
    let resp = req.send().await.unwrap();
    (resp.status().as_u16(), resp.text().await.unwrap())
}

fn code_of(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    v["error"]["code"].as_str().unwrap_or("<none>").to_string()
}

#[tokio::test]
async fn auth_tiers_and_proxy_passthrough() {
    let db = temp_db_path();
    seed_db(&db).await;
    let origin = start_server(&db, Some(MASTER)).await;
    let guarded = "/api/v1/system/status";

    // Master key reaches the upstream, and the proxy preserves method and path.
    let (status, body) = get(&origin, guarded, Some(MASTER)).await;
    assert_eq!((status, body.as_str()), (200, "GET /api/v1/system/status"));

    // Query strings survive too — SSE cursors ride in them. This probe has to
    // name a route that is still Python's: it read `/api/v1/processes?limit=5`
    // until the read half of that domain migrated, and then asserted Rust's own
    // 400 against the upstream's echo.
    let (_, body) = get(&origin, "/api/v1/system/logs?limit=5", Some(MASTER)).await;
    assert_eq!(body, "GET /api/v1/system/logs?limit=5");

    // Missing and wrong credentials, both TOKEN_INVALID like the Python server.
    let (status, body) = get(&origin, guarded, None).await;
    assert_eq!((status, code_of(&body).as_str()), (401, "TOKEN_INVALID"));
    let (status, body) = get(&origin, guarded, Some("nope")).await;
    assert_eq!((status, code_of(&body).as_str()), (401, "TOKEN_INVALID"));

    // The layer guards `/api/v1/*` only: `/health` is open, and `/v1/*` is the
    // LLM proxy, where each route decides for itself. Anything under `/v1` that
    // has not migrated reaches the upstream without this layer rejecting it.
    assert_eq!(get(&origin, "/health", None).await.0, 200);
    let (status, body) = get(&origin, "/v1/not-migrated", None).await;
    assert_eq!((status, body.as_str()), (200, "GET /v1/not-migrated"));

    // Workspace tokens, one case per exception in api_tokens/exceptions.py.
    let cases = [
        ("agp_live_good", 200, ""),
        ("agp_live_revoked", 401, "TOKEN_REVOKED"),
        ("agp_live_held", 403, "TOKEN_HELD"),
        ("agp_live_expired", 401, "TOKEN_EXPIRED"),
        ("agp_live_archived", 401, "TOKEN_REVOKED"),
        ("agp_live_missing", 401, "TOKEN_INVALID"),
    ];
    for (token, want_status, want_code) in cases {
        let (status, body) = get(&origin, guarded, Some(token)).await;
        assert_eq!(status, want_status, "{token} status; body={body}");
        if !want_code.is_empty() {
            assert_eq!(code_of(&body), want_code, "{token} code");
        }
    }

    // A rejected token reports its public prefix, never the secret.
    let (_, body) = get(&origin, guarded, Some("agp_live_revoked")).await;
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["extra"]["token_prefix"], "agp_live_rev");
    assert!(!body.contains("agp_live_revoked"));

    // Held tokens carry the operator's reason through.
    let (_, body) = get(&origin, guarded, Some("agp_live_held")).await;
    assert!(body.contains("Billing on hold."), "{body}");

    // Fixed window: limit is 1/min, so the second request in the same minute 429s.
    assert_eq!(get(&origin, guarded, Some("agp_live_limited")).await.0, 200);
    let (status, body) = get(&origin, guarded, Some("agp_live_limited")).await;
    assert_eq!((status, code_of(&body).as_str()), (429, "RATE_LIMIT_EXCEEDED"));

    let _ = std::fs::remove_file(&db);
}

#[tokio::test]
async fn every_response_carries_a_correlation_id() {
    // Python stamps one on every response and repeats it in the error envelope.
    // Without it, a failure in the Rust half cannot be lined up with the same
    // request in the Python half's log.
    let db = temp_db_path();
    seed_db(&db).await;
    let origin = start_server(&db, Some(MASTER)).await;

    let resp = reqwest::Client::new()
        .get(format!("{origin}/api/v1/system/status"))
        .send()
        .await
        .unwrap();
    let generated = resp.headers().get("x-request-id").unwrap().to_str().unwrap().to_string();
    assert_eq!(generated.len(), 36, "{generated}");
    let body: Value = serde_json::from_str(&resp.text().await.unwrap()).unwrap();
    assert_eq!(body["error"]["request_id"], generated);
    assert_eq!(body["error"]["code"], "TOKEN_INVALID");

    // A caller's own id wins, and reaches the upstream: the stub echoes the path
    // it was asked for, so a 200 here means the proxied request carried through.
    let resp = reqwest::Client::new()
        .get(format!("{origin}/api/v1/system/status"))
        .bearer_auth(MASTER)
        .header("X-Request-ID", "caller-supplied-id")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.headers().get("x-request-id").unwrap(), "caller-supplied-id");
    assert_eq!(resp.text().await.unwrap(), "GET /api/v1/system/status");

    let _ = std::fs::remove_file(&db);
}

#[tokio::test]
async fn llm_proxy_routes_authenticate_per_route() {
    // `require_token` guards `/api/v1/*` only, so every migrated `/v1` route
    // carries its own answer to "who is calling" — including "nobody, on
    // purpose". Getting that wrong in either direction is silent: a route that
    // stops checking serves the config to anyone, and one that starts checking
    // breaks the desktop's pre-key probe.
    let db = temp_db_path();
    seed_db(&db).await;
    let origin = start_server(&db, Some(MASTER)).await;

    let (status, body) = get(&origin, "/v1/health/readiness", None).await;
    assert_eq!(status, 200, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["status"], "ok");
    assert_eq!(v["checks"][0]["name"], "provider_config");

    let (status, body) = get(&origin, "/v1/capabilities", None).await;
    assert_eq!((status, code_of(&body).as_str()), (401, "TOKEN_INVALID"));

    // `/v1/models` is Rust's now, so it is authenticated here rather than by the
    // upstream — the stub would have answered 200 to anyone.
    let (status, body) = get(&origin, "/v1/models", None).await;
    assert_eq!((status, code_of(&body).as_str()), (401, "TOKEN_INVALID"));
    let (status, body) = get(&origin, "/v1/models", Some(MASTER)).await;
    assert_eq!(status, 200, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["object"], "list");
    assert!(v["data"].is_array(), "{body}");

    // `?live=false` keeps the catalog off every upstream, so this stays fast and
    // deterministic with no backend running.
    let (status, body) = get(&origin, "/v1/catalog?live=false", None).await;
    assert_eq!((status, code_of(&body).as_str()), (401, "TOKEN_INVALID"));
    let (status, body) = get(&origin, "/v1/catalog?live=false", Some(MASTER)).await;
    assert_eq!(status, 200, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["object"], "catalog");
    assert_eq!(v["providers"][0]["id"], "ollama");
    assert_eq!(v["providers"][0]["configured"], true);
    assert!(v["resolved_defaults"]["provider"].is_string(), "{body}");

    // `/v1/health` takes no token either, and answers about the default provider.
    let (status, body) = get(&origin, "/v1/health?provider=banana", None).await;
    assert_eq!(status, 400, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["status"], "error");
    assert!(v["detail"].as_str().unwrap().starts_with("provider must be"), "{body}");

    let (status, body) = get(&origin, "/v1/capabilities", Some(MASTER)).await;
    assert_eq!(status, 200, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["object"], "capabilities");
    assert_eq!(v["modalities"][0], "chat");
    // Both local backends carry a loopback default, so this holds with no
    // config on disk — which is what this test has.
    assert_eq!(v["providers"]["ollama"]["chat"], true);
    assert_eq!(v["providers"]["ollama"]["configured"], true);
    assert_eq!(v["providers"]["image_local"]["configured"], false);
    assert_eq!(v["resolved"]["chat"], "ollama");
    assert_eq!(v["resolved"]["image_generation"], Value::Null);
    assert_eq!(v["byok"]["providers"][0]["id"], "openai");

    let _ = std::fs::remove_file(&db);
}

#[tokio::test]
async fn sse_reaches_the_client_before_the_stream_ends() {
    // Run events, logs and chat are all SSE. A proxy that collects the response
    // before forwarding it would still pass every other test in this file, and
    // would make every live view in the app arrive late or not at all.
    let db = temp_db_path();
    seed_db(&db).await;
    let origin = start_server(&db, None).await;

    let started = std::time::Instant::now();
    let resp = reqwest::get(format!("{origin}/sse")).await.unwrap();
    let mut stream = resp.bytes_stream();
    let first = stream.next().await.unwrap().unwrap();

    assert_eq!(&first[..], b"data: first\n\n");
    assert!(started.elapsed() < std::time::Duration::from_millis(350), "first chunk was buffered");

    let _ = std::fs::remove_file(&db);
}

#[tokio::test]
async fn no_master_key_leaves_auth_open() {
    // The Python server's dev convenience: unset master key means no auth at all.
    // Diverging here would break every local run that never set the variable.
    let db = temp_db_path();
    seed_db(&db).await;
    let origin = start_server(&db, None).await;

    let (status, body) = get(&origin, "/api/v1/system/status", None).await;
    assert_eq!((status, body.as_str()), (200, "GET /api/v1/system/status"));

    let _ = std::fs::remove_file(&db);
}
