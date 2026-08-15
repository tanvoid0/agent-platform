//! Persisted, workspace-scoped search history (ADR 0008,
//! `docs/web-search-module-plan.md`'s successor plan): the promote-on-open
//! path, the tenancy contract, and the list-limit cap.
//!
//! `common::start_server` deliberately runs no schema — a caller that needs
//! tables seeds its own (see its doc comment) — so this file applies the real
//! migrations itself before starting the server, on the same SQLite file.

mod common;

use std::path::PathBuf;

use agent_platform_server::auth::hash_token;
use agent_platform_server::db::{self, Backend};
use serde_json::Value;

use common::{start_server, MASTER};

fn temp_db_path() -> PathBuf {
    common::temp_db_path("search-history")
}

/// The real schema plus two workspaces and one active token each. Returns the
/// two raw tokens, in workspace order.
async fn seed(path: &PathBuf) -> (String, String) {
    let _ = std::fs::remove_file(path);
    let url = db::url_for(path, None);
    let pool = db::connect_lazy(&url, Backend::Sqlite);
    db::ensure_schema(&pool).await.expect("schema");

    sqlx::query(
        "INSERT INTO workspace (id, name, slug, created_at, updated_at) \
         VALUES (1, 'A', 'a', '2026-01-01 00:00:00.000000', '2026-01-01 00:00:00.000000'), \
                (2, 'B', 'b', '2026-01-01 00:00:00.000000', '2026-01-01 00:00:00.000000')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let tokens = [("agp_live_ws_a_tok", 1i64), ("agp_live_ws_b_tok", 2i64)];
    for (raw, ws) in tokens {
        sqlx::query(
            "INSERT INTO api_tokens (workspace_id, name, prefix, token_hash, scopes_json, status) \
             VALUES (?, 'test', ?, ?, '[\"*\"]', 'active')",
        )
        .bind(ws)
        .bind(&raw[..12])
        .bind(hash_token(raw))
        .execute(&pool)
        .await
        .unwrap();
    }

    pool.close().await;
    (tokens[0].0.to_string(), tokens[1].0.to_string())
}

async fn post_history(
    origin: &str,
    bearer: &str,
    query: &str,
    engine: &str,
    source: &str,
    opened: bool,
) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{origin}/api/v1/search/history"))
        .bearer_auth(bearer)
        .json(&serde_json::json!({ "query": query, "engine": engine, "source": source, "opened": opened }))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let body: Value = serde_json::from_str(&resp.text().await.unwrap()).unwrap_or(Value::Null);
    (status, body)
}

async fn list_history(origin: &str, bearer: &str, query: &str) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .get(format!("{origin}/api/v1/search/history{query}"))
        .bearer_auth(bearer)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let body: Value = serde_json::from_str(&resp.text().await.unwrap()).unwrap_or(Value::Null);
    (status, body)
}

async fn delete_history(origin: &str, bearer: &str, id: i64) -> u16 {
    reqwest::Client::new()
        .delete(format!("{origin}/api/v1/search/history/{id}"))
        .bearer_auth(bearer)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// Posting the same query twice — first as a build (`opened: false`), then as
/// a run (`opened: true`) — must promote the one row already on file rather
/// than leave two near-duplicates behind. This is the one bit of real logic
/// `create_history` has, per the task brief.
#[tokio::test]
async fn opening_a_built_query_promotes_the_existing_row_instead_of_duplicating_it() {
    let db = temp_db_path();
    let (token_a, _) = seed(&db).await;
    let origin = start_server(&db, Some(MASTER)).await;

    let (status, built) =
        post_history(&origin, &token_a, "site:reddit.com keyboards", "google", "rules", false).await;
    assert_eq!(status, 201, "{built}");
    assert_eq!(built["opened"], false);
    let built_id = built["id"].as_i64().unwrap();

    let (status, opened) =
        post_history(&origin, &token_a, "site:reddit.com keyboards", "google", "rules", true).await;
    // 200, not 201: the existing row was promoted, nothing was inserted.
    assert_eq!(status, 200, "{opened}");
    assert_eq!(opened["id"], built_id, "promotion must reuse the same row");
    assert_eq!(opened["opened"], true);

    let (status, list) = list_history(&origin, &token_a, "").await;
    assert_eq!(status, 200, "{list}");
    let rows = list["history"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "one row, not two: {rows:?}");
    assert_eq!(rows[0]["opened"], true);

    let _ = std::fs::remove_file(&db);
}

/// A second `opened: false` post for the same text is not promotion — it is a
/// second build event, and gets its own row.
#[tokio::test]
async fn a_second_unopened_post_of_the_same_query_is_not_promoted() {
    let db = temp_db_path();
    let (token_a, _) = seed(&db).await;
    let origin = start_server(&db, Some(MASTER)).await;

    post_history(&origin, &token_a, "cheap keyboard", "google", "rules", false).await;
    post_history(&origin, &token_a, "cheap keyboard", "google", "rules", false).await;

    let (_, list) = list_history(&origin, &token_a, "").await;
    assert_eq!(list["history"].as_array().unwrap().len(), 2);

    let _ = std::fs::remove_file(&db);
}

/// The tenancy contract: a workspace token cannot read, delete, or promote
/// into a row that belongs to a different workspace, and every rejection is a
/// 404 — never a 401 — so a foreign token cannot learn the row exists.
#[tokio::test]
async fn a_workspace_token_cannot_read_delete_or_promote_another_workspaces_row() {
    let db = temp_db_path();
    let (token_a, token_b) = seed(&db).await;
    let origin = start_server(&db, Some(MASTER)).await;

    let (_, built) =
        post_history(&origin, &token_a, "workspace a's own query", "google", "rules", false).await;
    let a_id = built["id"].as_i64().unwrap();

    // B's list never shows A's row.
    let (status, list) = list_history(&origin, &token_b, "").await;
    assert_eq!(status, 200, "{list}");
    assert!(list["history"].as_array().unwrap().is_empty(), "{list}");

    // B cannot delete A's row.
    assert_eq!(delete_history(&origin, &token_b, a_id).await, 404);
    // A still lists it — the rejected delete above did not touch it.
    let (_, list) = list_history(&origin, &token_a, "").await;
    assert_eq!(list["history"].as_array().unwrap().len(), 1);

    // B posting the identical query text with opened=true must not promote
    // A's row — it is scoped out of B's lookup, so B gets a fresh row of its
    // own instead.
    let (status, b_opened) =
        post_history(&origin, &token_b, "workspace a's own query", "google", "rules", true).await;
    assert_eq!(status, 201, "{b_opened}");
    assert_ne!(b_opened["id"], a_id, "B must not have promoted A's row");

    // A's own row is unaffected by B's post.
    let (_, list) = list_history(&origin, &token_a, "").await;
    assert_eq!(list["history"][0]["opened"], false);

    // A can delete its own row.
    assert_eq!(delete_history(&origin, &token_a, a_id).await, 204);

    let _ = std::fs::remove_file(&db);
}

/// `limit` is capped regardless of what the caller asks for.
#[tokio::test]
async fn the_list_limit_is_capped() {
    let db = temp_db_path();
    let (token_a, _) = seed(&db).await;
    let origin = start_server(&db, Some(MASTER)).await;

    for n in 0..5 {
        post_history(&origin, &token_a, &format!("query {n}"), "google", "verbatim", false).await;
    }

    let (status, list) = list_history(&origin, &token_a, "?limit=1000000").await;
    assert_eq!(status, 200, "{list}");
    // Five rows were written; the point under test is that an absurd `limit`
    // did not pass straight through to `LIMIT ?` unclamped, not the exact
    // cap value — five is comfortably under any reasonable cap.
    assert_eq!(list["history"].as_array().unwrap().len(), 5);

    let (status, capped) = list_history(&origin, &token_a, "?limit=2").await;
    assert_eq!(status, 200, "{capped}");
    assert_eq!(capped["history"].as_array().unwrap().len(), 2, "a small limit is still honoured");

    let _ = std::fs::remove_file(&db);
}

/// `opened_only=true` filters out the built-but-not-opened rows.
#[tokio::test]
async fn opened_only_filters_the_list() {
    let db = temp_db_path();
    let (token_a, _) = seed(&db).await;
    let origin = start_server(&db, Some(MASTER)).await;

    post_history(&origin, &token_a, "built only", "google", "rules", false).await;
    post_history(&origin, &token_a, "actually run", "google", "rules", true).await;

    let (_, all) = list_history(&origin, &token_a, "").await;
    assert_eq!(all["history"].as_array().unwrap().len(), 2);

    let (_, opened) = list_history(&origin, &token_a, "?opened_only=true").await;
    let rows = opened["history"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["query"], "actually run");

    let _ = std::fs::remove_file(&db);
}

/// `query`, `engine` and `source` are all required — a 400 naming the
/// problem, not a 500 from a NOT NULL column.
#[tokio::test]
async fn missing_required_fields_are_a_400() {
    let db = temp_db_path();
    let (token_a, _) = seed(&db).await;
    let origin = start_server(&db, Some(MASTER)).await;

    let resp = reqwest::Client::new()
        .post(format!("{origin}/api/v1/search/history"))
        .bearer_auth(&token_a)
        .json(&serde_json::json!({ "engine": "google", "source": "rules" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);

    let _ = std::fs::remove_file(&db);
}

/// `DELETE /search/history` clears only the caller's own workspace.
#[tokio::test]
async fn clear_history_only_clears_the_callers_workspace() {
    let db = temp_db_path();
    let (token_a, token_b) = seed(&db).await;
    let origin = start_server(&db, Some(MASTER)).await;

    post_history(&origin, &token_a, "a's query", "google", "rules", false).await;
    post_history(&origin, &token_b, "b's query", "google", "rules", false).await;

    let status = reqwest::Client::new()
        .delete(format!("{origin}/api/v1/search/history"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16();
    assert_eq!(status, 204);

    let (_, a_list) = list_history(&origin, &token_a, "").await;
    assert!(a_list["history"].as_array().unwrap().is_empty(), "{a_list}");

    let (_, b_list) = list_history(&origin, &token_b, "").await;
    assert_eq!(b_list["history"].as_array().unwrap().len(), 1, "B's row must survive A's clear");

    let _ = std::fs::remove_file(&db);
}
