//! `/api/v1/ads/*` end to end: the brand brief round trip, the platform list,
//! and the two refusals that happen before any model is asked (ADR 0017).
//!
//! **No stub language model here on purpose.** The copy pass is one
//! `llm::complete_internal` call, and everything branchy about it — the fence
//! stripping, the preamble, prose instead of JSON, a variant with no picture
//! prompt, the variant-count ceiling — is unit-tested against `parse_variants`
//! in `ads.rs` where it costs nothing. What that cannot reach is the schema:
//! whether `0009_ad_campaigns` actually applies, whether a brief survives a
//! write and a read, and whether a campaign is refused *before* it spends a
//! model round-trip. That is what this file is for.
//!
//! The two early refusals matter more than they look. A campaign against a
//! blank brand brief would be an ad about nothing, and a platform this server
//! does not know would be a size the media seam silently rewrites — both have
//! to fail at the door, with zero media jobs started.

mod common;

use std::sync::Arc;

use serde_json::{json, Value};

use common::{start_server, temp_db_path, MASTER};

fn authed(rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    rb.header("Authorization", format!("Bearer {MASTER}"))
}

/// A workspace and a project inside it, because a brand brief hangs off a
/// project and `projects::assert_access` hides one with no workspace from
/// everyone — including the master key.
async fn seed_project(http: &reqwest::Client, origin: &str) -> i64 {
    let workspace: Value = authed(http.post(format!("{origin}/api/v1/workspaces/")))
        .json(&json!({ "name": "Ads test" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let workspace_id = workspace["id"].as_i64().expect("workspace id");

    let project: Value = authed(http.post(format!("{origin}/api/v1/projects/")))
        .json(&json!({ "name": "Devstrail", "workspace_id": workspace_id }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    project["id"].as_i64().expect("project id")
}

#[tokio::test]
async fn the_brief_round_trips_and_a_campaign_is_refused_before_any_model_is_asked() {
    let db = temp_db_path("ads-routes");
    let state = Arc::new(agent_platform_server::AppState::new(&db, Some(MASTER.to_string())));
    agent_platform_server::db::ensure_schema(&state.any).await.unwrap();
    drop(state);

    let origin = start_server(&db, Some(MASTER)).await;
    let http = reqwest::Client::new();
    let project_id = seed_project(&http, &origin).await;

    // -- the platform list --------------------------------------------------
    //
    // The server owns these so a client cannot ask for a size the media seam
    // would rewrite; the unit test proves they are snap-clean, this proves the
    // route hands them over.
    let platforms: Value = authed(http.get(format!("{origin}/api/v1/ads/platforms")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let list = platforms["platforms"].as_array().expect("platforms array");
    assert!(!list.is_empty(), "a client with no platforms cannot make an ad");
    assert!(
        list.iter().any(|p| p["id"] == "ig_feed" && p["width"] == 1088),
        "the square Instagram preset is the safe default and must be offered: {platforms}"
    );

    // -- an untouched project has a blank brief, not a 404 ------------------
    let brand: Value = authed(http.get(format!("{origin}/api/v1/projects/{project_id}/brand")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(brand["company"], "", "no brief yet is the ordinary starting state");

    // -- a campaign against a blank brief is refused, and starts nothing ----
    let response = authed(http.post(format!("{origin}/api/v1/ads/campaigns")))
        .json(&json!({
            "project_id": project_id, "platform": "ig_feed", "brief": "launch week"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.unwrap();
    assert!(
        body.to_string().contains("ads_brand_missing"),
        "the refusal must name the missing brief rather than failing on the model: {body}"
    );

    // -- the brief round trips through the new column -----------------------
    let saved: Value = authed(http.put(format!("{origin}/api/v1/projects/{project_id}/brand")))
        .json(&json!({
            "company": "Devstrail",
            "product": "internal tools for small teams",
            "audience": "founders and ops leads",
            "voice": "plain and specific",
            "link": "https://devstrail.com",
            "avoid": "never claim we are the cheapest"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(saved["company"], "Devstrail");

    let read_back: Value = authed(http.get(format!("{origin}/api/v1/projects/{project_id}/brand")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(read_back, saved, "what a client PUTs is what it GETs back");

    // -- an unknown platform is refused, and names the ones that work -------
    //
    // Now that the brief exists, this is the *next* gate — proving the order:
    // a bad platform never reaches the copy pass either.
    let response = authed(http.post(format!("{origin}/api/v1/ads/campaigns")))
        .json(&json!({
            "project_id": project_id, "platform": "tiktok", "brief": "launch week"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let body = response.text().await.unwrap();
    assert!(
        body.contains("ig_feed") && body.contains("threads"),
        "the refusal must list what is accepted, not just say no: {body}"
    );

    // -- no campaign was created by any of that ----------------------------
    let campaigns: Value =
        authed(http.get(format!("{origin}/api/v1/ads/campaigns?project_id={project_id}")))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    assert_eq!(
        campaigns["campaigns"].as_array().map(Vec::len),
        Some(0),
        "a refused request must leave no row behind: {campaigns}"
    );

    // -- and no media job either -------------------------------------------
    let jobs: Value = authed(http.get(format!("{origin}/api/v1/media/jobs")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        jobs["jobs"].as_array().map(Vec::len),
        Some(0),
        "the refusals happen before the media seam is touched: {jobs}"
    );

    let _ = std::fs::remove_file(&db);
}

/// A brief big enough to crowd the instructions out of the prompt is refused by
/// size, not truncated silently — the user would otherwise generate ads from a
/// brief the server had quietly cut in half.
#[tokio::test]
async fn an_oversized_brief_is_refused_rather_than_trimmed() {
    let db = temp_db_path("ads-brief-cap");
    let state = Arc::new(agent_platform_server::AppState::new(&db, Some(MASTER.to_string())));
    agent_platform_server::db::ensure_schema(&state.any).await.unwrap();
    drop(state);

    let origin = start_server(&db, Some(MASTER)).await;
    let http = reqwest::Client::new();
    let project_id = seed_project(&http, &origin).await;

    let response = authed(http.put(format!("{origin}/api/v1/projects/{project_id}/brand")))
        .json(&json!({ "product": "x".repeat(9000) }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);

    // The oversized write did not land: the stored brief is still blank.
    let brand: Value = authed(http.get(format!("{origin}/api/v1/projects/{project_id}/brand")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(brand["product"], "", "a refused write must not half-apply");

    let _ = std::fs::remove_file(&db);
}
