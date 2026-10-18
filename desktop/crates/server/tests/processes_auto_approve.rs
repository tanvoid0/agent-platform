//! `PATCH /api/v1/processes/{id}` — the one field it is allowed to change.
//!
//! Auto-approve used to live only in the `spawn_plan` argument, readable once at
//! the plan gate and unchangeable afterwards. It is a column now, and this is
//! the round trip that proves it: the flag persists, the route flips it, and
//! `GET` reports it back as a boolean rather than the `INTEGER` it is stored as.
//!
//! The row is seeded with SQL rather than `POST /processes`, which would start a
//! planner and want a language model. What the executor *does* with the flag is
//! unit-tested next to the gates themselves.

mod common;

use std::sync::Arc;

use serde_json::{json, Value};

use common::{start_server, temp_db_path, MASTER};

fn authed(rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    rb.header("Authorization", format!("Bearer {MASTER}"))
}

#[tokio::test]
async fn auto_approve_persists_and_flips_on_a_process_already_created() {
    let db = temp_db_path("processes-auto-approve");
    let state = Arc::new(agent_platform_server::AppState::new(&db, Some(MASTER.to_string())));
    agent_platform_server::db::ensure_schema(&state.any).await.unwrap();
    let process_id: i64 = sqlx::query_scalar(&agent_platform_server::db::sql(
        "INSERT INTO process (goal, status, total_tokens, total_cost, tool_invocations_used, \
         auto_approve, created_at, updated_at) \
         VALUES ('g', 'running', 0, 0.0, 0, 1, '2026-01-01T00:00:00', '2026-01-01T00:00:00') \
         RETURNING CAST(id AS BIGINT)",
        state.backend,
    ))
    .fetch_one(&state.any)
    .await
    .unwrap();
    drop(state);

    let origin = start_server(&db, Some(MASTER)).await;
    let http = reqwest::Client::new();

    // Seeded on: the column survives the write and reads back as a boolean.
    let detail: Value = authed(http.get(format!("{origin}/api/v1/processes/{process_id}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["process"]["auto_approve"], json!(true));

    // Off, mid-run — the process stays `running`, only the flag moves.
    let patched: Value = authed(http.patch(format!("{origin}/api/v1/processes/{process_id}")))
        .json(&json!({ "auto_approve": false }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(patched["auto_approve"], json!(false));
    assert_eq!(patched["status"], json!("running"));

    // And back on, so the toggle is not one-way.
    let patched: Value = authed(http.patch(format!("{origin}/api/v1/processes/{process_id}")))
        .json(&json!({ "auto_approve": true }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(patched["auto_approve"], json!(true));

    // An empty body is a no-op, not a reset: only what was sent is written.
    let untouched: Value = authed(http.patch(format!("{origin}/api/v1/processes/{process_id}")))
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(untouched["auto_approve"], json!(true));

    let missing = authed(http.patch(format!("{origin}/api/v1/processes/999999")))
        .json(&json!({ "auto_approve": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status().as_u16(), 404);
}
