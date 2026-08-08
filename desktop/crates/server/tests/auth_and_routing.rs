//! The check that fails if auth stops matching what `app/api_tokens/auth.py`
//! did, or if an unknown path stops answering 404.
//!
//! It was `auth_and_proxy` when there was a proxy: it ran against a stub
//! upstream and asserted that requests reached it. The upstream is gone, so the
//! passthrough assertions became 404 assertions on the same invented path, and
//! the SSE test went with it — it existed to catch a proxy that buffered a
//! whole response before forwarding it, and there is nothing left in between to
//! buffer. Streaming is still covered where it is produced (`model_ops`'s job
//! stream, `processes`' run stream).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use agent_platform_server::auth::hash_token;
use agent_platform_server::{router, AppState};
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

async fn start_server(db: &PathBuf, master_key: Option<&str>) -> String {
    let state = Arc::new(AppState::new(db, master_key.map(str::to_owned)));
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
async fn auth_tiers_and_unknown_paths() {
    let db = temp_db_path();
    seed_db(&db).await;
    let origin = start_server(&db, Some(MASTER)).await;
    // A path with no route, invented rather than borrowed from a real domain:
    // this probe used `/api/v1/processes` and then `/api/v1/system/status`, and
    // each time that domain migrated the test started asserting something else.
    //
    // **Auth runs before routing**, which is the property under test here — an
    // unknown path must still 401 for a bad token rather than telling an
    // unauthenticated caller which paths exist.
    let guarded = "/api/v1/proxy-probe";

    // Authenticated, so routing is reached and the 404 is this server's envelope.
    let (status, body) = get(&origin, guarded, Some(MASTER)).await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(code_of(&body), "not_found");

    // The 404 message names the method and path, and not the query string.
    let (_, body) = get(&origin, "/api/v1/proxy-probe?limit=5", Some(MASTER)).await;
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["message"], "No route for GET /api/v1/proxy-probe");

    // Missing and wrong credentials, both TOKEN_INVALID like the Python server.
    let (status, body) = get(&origin, guarded, None).await;
    assert_eq!((status, code_of(&body).as_str()), (401, "TOKEN_INVALID"));
    let (status, body) = get(&origin, guarded, Some("nope")).await;
    assert_eq!((status, code_of(&body).as_str()), (401, "TOKEN_INVALID"));

    // The layer guards `/api/v1/*` only: `/health` is open, and `/v1/*` is the
    // LLM proxy, where each route decides for itself. An unknown path under
    // `/v1` therefore 404s rather than 401ing — the layer never saw it.
    assert_eq!(get(&origin, "/health", None).await.0, 200);
    let (status, body) = get(&origin, "/v1/no-such-route", None).await;
    assert_eq!((status, code_of(&body).as_str()), (404, "not_found"));

    // Workspace tokens, one case per exception in api_tokens/exceptions.py.
    //
    // A token that *passes* now shows as 404, not 200: the probe path has no
    // handler, so getting past the auth layer is exactly what "the request
    // reached routing" looks like. `PASSES` names that so the table below reads
    // as accept/reject rather than as a list of arbitrary status codes.
    const PASSES: u16 = 404;
    let cases = [
        ("agp_live_good", PASSES, ""),
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
    assert_eq!(get(&origin, guarded, Some("agp_live_limited")).await.0, PASSES);
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
        .get(format!("{origin}/api/v1/proxy-probe"))
        .send()
        .await
        .unwrap();
    let generated = resp.headers().get("x-request-id").unwrap().to_str().unwrap().to_string();
    assert_eq!(generated.len(), 36, "{generated}");
    let body: Value = serde_json::from_str(&resp.text().await.unwrap()).unwrap();
    assert_eq!(body["error"]["request_id"], generated);
    assert_eq!(body["error"]["code"], "TOKEN_INVALID");

    // A caller's own id wins, and is stamped on a 404 as much as on anything
    // else — the middleware is outside the router, not inside a handler.
    let resp = reqwest::Client::new()
        .get(format!("{origin}/api/v1/proxy-probe"))
        .bearer_auth(MASTER)
        .header("X-Request-ID", "caller-supplied-id")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.headers().get("x-request-id").unwrap(), "caller-supplied-id");
    let body: Value = serde_json::from_str(&resp.text().await.unwrap()).unwrap();
    assert_eq!(body["error"]["request_id"], "caller-supplied-id");

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

/// `/health` is the desktop's liveness probe and a container's readiness check,
/// and it used to answer `ok` from the fact that the handler ran at all. A
/// server whose database it cannot open answers every *other* route with a 500,
/// so reporting `ok` there is the one failure this endpoint exists to catch.
#[tokio::test]
async fn health_fails_when_the_database_cannot_be_opened() {
    let db = temp_db_path();
    seed_db(&db).await;
    let origin = start_server(&db, None).await;
    let (status, body) = get(&origin, "/health", None).await;
    assert_eq!(status, 200, "{body}");

    // A path under a directory that does not exist: `mode=rwc` creates a file,
    // it does not create the directory above it.
    let unopenable = temp_db_path().join("no-such-dir").join("agent_platform.db");
    let origin = start_server(&unopenable, None).await;
    let (status, body) = get(&origin, "/health", None).await;
    assert_eq!(status, 503, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["status"], "error");
    assert_eq!(v["detail"], "database unavailable");

    let _ = std::fs::remove_file(&db);
}

/// The general body cap holds, and an upload route's own cap overrides it.
///
/// The override is the part worth a test: both layers write the same request
/// extension and the *inner* one wins, so moving the route-level layer outward
/// by one line silently reverts every upload to 16 MB. axum's default was 2 MB
/// and applied to the upload routes too, which is the regression this pins.
/// Driven through the router as a `Service` rather than over a socket. A body
/// limit is rejected *while the client is still writing*, so over a real
/// connection reqwest reports the reset rather than the 413 the server sent,
/// and which of the two it sees depends on socket buffer sizes.
#[tokio::test]
async fn oversized_json_is_refused_and_uploads_are_not() {
    use tower::ServiceExt;

    let db = temp_db_path();
    seed_db(&db).await;
    let app = agent_platform_server::router(Arc::new(AppState::new(&db, None)));

    async fn post(
        app: &axum::Router,
        path: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> u16 {
        let req = axum::http::Request::post(path)
            .header("content-type", content_type)
            .body(axum::body::Body::from(body))
            .unwrap();
        app.clone().oneshot(req).await.unwrap().status().as_u16()
    }

    // Past the 16 MB general cap, under the 512 MB upload one.
    let big = vec![b'x'; 17 * 1024 * 1024];

    let status = post(&app, "/api/v1/projects", "application/json", big.clone()).await;
    assert_eq!(status, 413, "a 17 MB JSON body must not be buffered");

    // The upload route reads the same body. It is not valid multipart, so the
    // assertion is only that it got *past* the limit and into the handler —
    // a 413 here would mean the route-level layer is not winning.
    let status = post(
        &app,
        "/api/v1/projects/1/workspace/upload",
        "multipart/form-data; boundary=nope",
        big,
    )
    .await;
    assert_ne!(status, 413, "the upload route sets its own, larger cap");

    let _ = std::fs::remove_file(&db);
}

#[tokio::test]
async fn no_master_key_leaves_auth_open() {
    // The Python server's dev convenience: unset master key means no auth at all.
    // Diverging here would break every local run that never set the variable.
    let db = temp_db_path();
    seed_db(&db).await;
    let origin = start_server(&db, None).await;

    // 404 rather than 401: with no key configured every caller is authorized,
    // so the request reaches routing and finds nothing there.
    let (status, body) = get(&origin, "/api/v1/proxy-probe", None).await;
    assert_eq!((status, code_of(&body).as_str()), (404, "not_found"));

    let _ = std::fs::remove_file(&db);
}
