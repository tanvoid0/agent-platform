//! `/api/v1/ads/*` — social advertisements: platform-shaped pictures with the
//! words to post beside them (ADR 0017).
//!
//! **What this domain actually is.** A campaign is one press of a button that
//! turns a project's brand brief plus a one-line ask into N *variants*, each a
//! caption, a hashtag set, a call to action, and a picture sized for the
//! platform it is going to. The pictures are ordinary [`crate::media`] jobs —
//! same row, same waiter, same file route — so nothing here polls, watches or
//! writes a file. This module owns exactly two things the media domain cannot:
//! the copy, and the sizes.
//!
//! **The team is a roster rendered into a prompt.** That is what a team *is*
//! in this codebase — [`crate::executor`] does the same thing for the planner,
//! and roles are `modality: "text"` by construction (`teams.rs`). So a
//! campaign either names a `teamtemplate` row or gets [`DEFAULT_ROSTER`], and
//! either way the roles land in the system prompt as the voices the copy is
//! written by. Running the DAG executor instead would need a structured
//! channel out of a tasknode that does not exist, and its tool path is
//! deliberately dead — that is the Phase 2 question, not this one.
//!
//! **One video preset, and its size is a measurement, not a choice.** On a
//! 16 GB card 1088x1920 — the nominal story size — sampled for three and a half
//! minutes and then killed the ComfyUI *process* in VAE decode, which is not an
//! error any client can report. 720x1280 is Instagram's own recommended reel
//! minimum, survives, and takes about seven minutes for two seconds. That is
//! the one offered; the larger one must never be.
//!
//! **Sizes are the server's, not the screen's.** Studio lets a user pick any
//! dimensions; an ad may not. `GET /platforms` is the single list, so the
//! desktop cannot drift from what the seam will actually render — and every
//! entry is chosen to pass [`crate::media::snap`] untouched, which is why they
//! are 1088 and not 1080. Platforms rescale on upload; a silent rewrite to
//! 1072 inside the seam would be the surprise.
//!
//! **Copy is all-or-nothing, pictures are best-effort.** A model that answers
//! prose instead of JSON fails the whole request and starts zero jobs — half a
//! campaign is worse than none. But once the copy exists it is stored even if
//! the backend refuses every picture: the words cost a model round-trip and
//! are useful on their own, so a variant carries `media_job_id: null` and the
//! reason rather than taking the campaign down with it.
//!
//! **Master-key only**, like the media jobs it starts (ADR 0009 "Tenancy"),
//! with `projects::assert_access` on the project so a campaign cannot be filed
//! against a tenant the caller cannot see.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::FromRow;

use crate::auth::Principal;
use crate::db;
use crate::error::{ApiError, PathId};
use crate::teams::{RosterRole, TeamRoster};
use crate::wire::{iso_from_sql, sql_now};
use crate::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/brand",
            get(get_brand).put(put_brand),
        )
        .route("/api/v1/ads/platforms", get(platforms))
        .route("/api/v1/ads/campaigns", get(list_campaigns).post(create_campaign))
        .route("/api/v1/ads/campaigns/{campaign_id}", axum::routing::delete(delete_campaign))
        .route("/api/v1/ads/campaigns/", post(create_campaign))
}

const NOT_MASTER: &str = "Ad campaigns are managed with the master key.";

// ---------------------------------------------------------------------------
// Platform specs
// ---------------------------------------------------------------------------

/// One place a finished ad can go, and the shape it has to be in.
///
/// `width`/`height` are already snapped: every entry is a multiple of 16 inside
/// 256–2048, so [`crate::media::snap`] passes them through unchanged. A test
/// below asserts that, because a 1080 added here later would be silently
/// rewritten to 1072 and nobody would notice until the crop looked wrong.
///
/// `caption_limit` and `hashtag_max` are the platform's own rules, and they go
/// into the copy prompt — a 900-character caption for Threads is a caption the
/// user has to cut by hand, which is the work this feature exists to avoid.
pub struct PlatformSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// What `media` is asked to make: `image` or `video`. Exactly one preset is
    /// video, and only at a size measured not to kill the backend (ADR 0017).
    pub kind: &'static str,
    pub width: i64,
    pub height: i64,
    pub caption_limit: i64,
    pub hashtag_max: i64,
    /// Video only: frames at 24fps, `0` for an image. The seam takes a frame
    /// count, not seconds (`media::job_spec`), and 49 is the `4n+1` the Wan
    /// latent wants — roughly two seconds.
    pub length: i64,
    /// One line for the picker, naming the aspect the numbers add up to.
    pub note: &'static str,
}

impl PlatformSpec {
    pub fn is_video(&self) -> bool {
        self.kind == "video"
    }
}

pub const PLATFORMS: &[PlatformSpec] = &[
    PlatformSpec {
        id: "ig_feed",
        label: "Instagram feed (square)",
        kind: "image",
        width: 1088,
        height: 1088,
        caption_limit: 2200,
        hashtag_max: 10,
        length: 0,
        note: "1:1 — the safe default, never cropped in a grid.",
    },
    PlatformSpec {
        id: "ig_portrait",
        label: "Instagram feed (portrait)",
        kind: "image",
        width: 1088,
        height: 1360,
        caption_limit: 2200,
        hashtag_max: 10,
        length: 0,
        note: "4:5 — the tallest a feed post may be, so it fills more screen.",
    },
    PlatformSpec {
        id: "ig_story",
        label: "Instagram story / Reel cover",
        kind: "image",
        width: 1088,
        height: 1920,
        caption_limit: 2200,
        hashtag_max: 5,
        length: 0,
        note: "9:16 full screen — keep text away from the top and bottom 250px.",
    },
    PlatformSpec {
        id: "fb_feed",
        label: "Facebook feed",
        kind: "image",
        width: 1200,
        height: 624,
        caption_limit: 2000,
        hashtag_max: 3,
        length: 0,
        note: "1.91:1 landscape — the link-preview shape.",
    },
    PlatformSpec {
        id: "threads",
        label: "Threads",
        kind: "image",
        width: 1088,
        height: 1088,
        caption_limit: 500,
        hashtag_max: 1,
        length: 0,
        note: "1:1, and a hard 500-character post. Threads shows one topic tag.",
    },
    // The only video preset, and its size is measured rather than chosen.
    // On an RTX 5080 (16 GB): 480x832 renders in 45s and 720x1280 in 7.2
    // minutes, both for 49 frames — while 1088x1920, the nominal story size,
    // sampled for 3.4 minutes and then **killed the ComfyUI process** in VAE
    // decode. 720x1280 is Instagram's own recommended reel minimum and is the
    // largest of those that survives, so it is the one offered.
    PlatformSpec {
        id: "ig_reel",
        label: "Instagram reel / story (video)",
        kind: "video",
        width: 720,
        height: 1280,
        caption_limit: 2200,
        hashtag_max: 5,
        length: 49,
        note: "9:16, about two seconds, and minutes to render. A moving backdrop for a caption, not a film.",
    },
];

fn find_platform(id: &str) -> Option<&'static PlatformSpec> {
    PLATFORMS.iter().find(|p| p.id == id)
}

async fn platforms(principal: Principal) -> Result<Response, ApiError> {
    principal.require_master_key(NOT_MASTER)?;
    let items: Vec<Value> = PLATFORMS
        .iter()
        .map(|p| {
            json!({
                "id": p.id, "label": p.label, "kind": p.kind,
                "width": p.width, "height": p.height,
                "caption_limit": p.caption_limit, "hashtag_max": p.hashtag_max,
                "length": p.length,
                "note": p.note,
            })
        })
        .collect();
    Ok(Json(json!({ "platforms": items })).into_response())
}

// ---------------------------------------------------------------------------
// The marketing team
// ---------------------------------------------------------------------------

/// The roster a campaign uses when it names no team of its own.
///
/// Four voices, because that is what actually shapes an ad: someone who owns
/// the positioning, someone who writes the words, someone who decides what the
/// picture is, and someone who knows the platform's own conventions. They are
/// prompt material — the model is told to write *as* this team — not processes
/// that run.
///
/// The Library screen ships the same four as an editable preset so a user can
/// fork and tune them; the two lists are for different consumers (this one
/// feeds a prompt, that one seeds a `teamtemplate` row) and neither breaks if
/// the other changes.
pub fn default_roster() -> TeamRoster {
    let role = |id: &str, name: &str, description: &str, parent: Option<&str>| RosterRole {
        id: id.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        modality: "text".to_string(),
        parent_id: parent.map(str::to_string),
        accent_color: None,
    };
    TeamRoster {
        roles: vec![
            role(
                "strategist",
                "Campaign strategist",
                "Owns the angle: which single benefit this ad is about, and who it is aimed at. \
                 Refuses to say three things at once.",
                None,
            ),
            role(
                "copywriter",
                "Copywriter",
                "Writes the caption and the call to action in the brand's voice. Leads with the \
                 hook, never with the company name.",
                Some("strategist"),
            ),
            role(
                "art_director",
                "Art director",
                "Decides what the picture shows and describes it as a diffusion prompt: subject, \
                 setting, lighting, composition. Keeps the frame clear where text will sit.",
                Some("strategist"),
            ),
            role(
                "social_lead",
                "Social media lead",
                "Knows each platform's conventions and limits. Picks hashtags that people \
                 actually follow rather than padding the count.",
                Some("strategist"),
            ),
        ],
    }
}

/// The roster as prompt text. Flat and plain: the model is being told who is in
/// the room, not asked to walk a graph. Same idea as
/// `executor::render_team_context_for_planner`, without the planner's wording
/// about mapping subagent names.
fn render_roster(name: &str, description: Option<&str>, roster: &TeamRoster) -> String {
    let mut lines = vec![format!("You are writing as the team \"{name}\".")];
    if let Some(d) = description.map(str::trim).filter(|d| !d.is_empty()) {
        lines.push(format!("Team brief: {d}"));
    }
    lines.push("The people in the room:".to_string());
    for role in &roster.roles {
        let mut line = format!("- {}", role.name);
        if !role.description.trim().is_empty() {
            line.push_str(&format!(": {}", role.description.trim()));
        }
        lines.push(line);
    }
    lines.join("\n")
}

/// The team a campaign is written by: a stored template if one was named, else
/// [`default_roster`]. A named template the caller cannot see 404s, which is
/// `teams.rs`'s own rule and not something to relax here.
async fn resolve_team(
    state: &AppState,
    principal: &Principal,
    team_template_id: Option<i64>,
) -> Result<(String, Option<String>, TeamRoster), ApiError> {
    let Some(id) = team_template_id else {
        return Ok(("Social media marketing".to_string(), None, default_roster()));
    };
    #[derive(FromRow)]
    struct Row {
        name: String,
        description: Option<String>,
        roster_json: String,
        workspace_id: Option<i64>,
    }
    let row: Row = sqlx::query_as(&db::sql(
        "SELECT name, description, roster_json, CAST(workspace_id AS BIGINT) AS workspace_id \
         FROM teamtemplate WHERE id = ?",
        state.backend,
    ))
    .bind(id)
    .fetch_optional(&state.any)
    .await?
    .ok_or_else(|| ApiError::not_found("Team template not found"))?;

    // A workspace token may use a global template or its own, never another
    // tenant's — the same visibility `teams::assert_visible` enforces.
    if let Some(ws) = principal.workspace_id {
        if row.workspace_id.is_some_and(|owner| owner != ws) {
            return Err(ApiError::not_found("Team template not found"));
        }
    }
    let roster = crate::teams::parse_roster(&row.roster_json)?;
    Ok((row.name, row.description, roster))
}

// ---------------------------------------------------------------------------
// The brand brief (per project)
// ---------------------------------------------------------------------------

/// What the copy pass knows about the thing being advertised.
///
/// Free text on purpose — every field lands in a prompt, so validating its
/// *shape* would buy nothing and cost the user the one place they can say
/// something the schema did not anticipate. The only rule is a size cap, so a
/// pasted document cannot push the real instructions out of the context window.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Brand {
    #[serde(default)]
    pub company: String,
    /// What the product or service actually is, in the user's own words.
    #[serde(default)]
    pub product: String,
    #[serde(default)]
    pub audience: String,
    /// How it should sound — "dry and technical", "warm, no exclamation marks".
    #[serde(default)]
    pub voice: String,
    /// Where a reader is meant to end up. Goes into the call to action.
    #[serde(default)]
    pub link: String,
    /// Anything the model must not do: claims that are not true yet,
    /// competitors not to name, words the founder hates.
    #[serde(default)]
    pub avoid: String,
}

/// The whole brief, capped. 8 KB is a page of prose per field and still leaves
/// a local model's context room for the roster and the platform rules.
const BRAND_MAX: usize = 8192;

impl Brand {
    fn is_empty(&self) -> bool {
        [&self.company, &self.product, &self.audience, &self.voice, &self.link, &self.avoid]
            .iter()
            .all(|f| f.trim().is_empty())
    }

    /// The brief as prompt text, skipping fields the user left blank — an empty
    /// `Audience:` line teaches the model that blank answers are acceptable.
    fn render(&self) -> String {
        let mut lines = Vec::new();
        for (label, value) in [
            ("Company", &self.company),
            ("What it is", &self.product),
            ("Who it is for", &self.audience),
            ("Voice", &self.voice),
            ("Link", &self.link),
            ("Never do this", &self.avoid),
        ] {
            let value = value.trim();
            if !value.is_empty() {
                lines.push(format!("{label}: {value}"));
            }
        }
        lines.join("\n")
    }
}

async fn load_brand(state: &AppState, project_id: i64) -> Result<Brand, ApiError> {
    let raw: Option<String> =
        sqlx::query_scalar(&db::sql("SELECT brand_json FROM project WHERE id = ?", state.backend))
            .bind(project_id)
            .fetch_optional(&state.any)
            .await?
            .ok_or_else(|| ApiError::not_found("Project not found"))?;
    // Unreadable stored JSON reads as no brief at all, the way
    // `projects::get_workspace_state` treats its own blob.
    Ok(raw.and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default())
}

async fn get_brand(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(project_id): PathId<i64>,
) -> Result<Response, ApiError> {
    crate::projects::assert_access(&state, &principal, project_id).await?;
    // The bare object both ways, so the thing a client PUTs is the thing it
    // GETs back — an envelope on one side only is a shape to remember.
    Ok(Json(load_brand(&state, project_id).await?).into_response())
}

async fn put_brand(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(project_id): PathId<i64>,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    crate::projects::assert_access(&state, &principal, project_id).await?;
    if body.len() > BRAND_MAX {
        return Err(ApiError::bad_request(format!(
            "The brand brief is {} bytes; the limit is {BRAND_MAX}. It all goes into one prompt, \
             so a document pasted here would crowd out the instructions.",
            body.len()
        )));
    }
    // The bare object is the documented shape; a `{"brand": {...}}` envelope is
    // still unwrapped, because refusing one would be a 400 for a request that
    // said exactly what it meant.
    let parsed: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    let inner = parsed.get("brand").cloned().unwrap_or(parsed);
    let brand: Brand = serde_json::from_value(inner).unwrap_or_default();

    sqlx::query(&db::sql(
        "UPDATE project SET brand_json = ?, updated_at = ? WHERE id = ?",
        state.backend,
    ))
    .bind(serde_json::to_string(&brand).unwrap_or_else(|_| "{}".into()))
    .bind(sql_now())
    .bind(project_id)
    .execute(&state.any)
    .await?;

    Ok(Json(brand).into_response())
}

// ---------------------------------------------------------------------------
// Campaigns
// ---------------------------------------------------------------------------

/// One finished ad: the words, the picture prompt, and the media job drawing
/// it. `media_job_id` is `None` when the backend refused the picture — see the
/// module doc on why that does not fail the campaign.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variant {
    pub caption: String,
    #[serde(default)]
    pub hashtags: Vec<String>,
    #[serde(default)]
    pub cta: String,
    pub image_prompt: String,
    #[serde(default)]
    pub negative: String,
    #[serde(default)]
    pub media_job_id: Option<i64>,
    /// Why this variant has no picture, when it has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CampaignCreate {
    project_id: Option<i64>,
    platform: Option<String>,
    /// The one-line ask: what this particular ad is about. The standing facts
    /// live in the project's brand brief and are not repeated here.
    brief: Option<String>,
    #[serde(default)]
    team_template_id: Option<i64>,
    #[serde(default)]
    variants: Option<i64>,
}

/// Three is enough to choose from and cheap enough to wait for; the cap stops a
/// model-written call from starting sixteen renders on a 16 GB card.
const DEFAULT_VARIANTS: i64 = 3;
const MAX_VARIANTS: i64 = 6;
const BRIEF_MAX: usize = 2000;

#[derive(Debug, FromRow)]
struct CampaignRow {
    id: i64,
    project_id: i64,
    team_template_id: Option<i64>,
    platform: String,
    brief: String,
    copy_json: Option<String>,
    created_at: String,
    updated_at: String,
    user_id: Option<i64>,
}

const CAMPAIGN_COLUMNS: &str = "CAST(id AS BIGINT) AS id, \
     CAST(project_id AS BIGINT) AS project_id, \
     CAST(team_template_id AS BIGINT) AS team_template_id, platform, brief, copy_json, \
     CAST(created_at AS TEXT) AS created_at, CAST(updated_at AS TEXT) AS updated_at, \
     CAST(user_id AS BIGINT) AS user_id";

/// The wire shape. `copy_json` is unpacked into real variants rather than
/// handed over as a string — a client that has to parse a field of a field is a
/// client that will parse it differently from this one.
fn row_to_json(row: &CampaignRow) -> Value {
    let variants: Vec<Variant> = row
        .copy_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|v| v.get("variants").cloned())
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let spec = find_platform(&row.platform);
    json!({
        "id": row.id,
        "project_id": row.project_id,
        "team_template_id": row.team_template_id,
        "platform": row.platform,
        "platform_label": spec.map(|s| s.label),
        "width": spec.map(|s| s.width),
        "height": spec.map(|s| s.height),
        "brief": row.brief,
        "variants": variants,
        "created_at": iso_from_sql(&row.created_at),
        "updated_at": iso_from_sql(&row.updated_at),
        "user_id": row.user_id,
    })
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    project_id: Option<i64>,
}

const CAMPAIGNS_LIMIT: i64 = 100;

async fn list_campaigns(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    principal.require_master_key(NOT_MASTER)?;

    // Filtering by project is the normal case and checks access on the way in;
    // the unfiltered list is the gallery, scoped by user like `media::list_jobs`.
    let rows: Vec<CampaignRow> = match q.project_id {
        Some(project_id) => {
            crate::projects::assert_access(&state, &principal, project_id).await?;
            sqlx::query_as(&db::sql(
                &format!(
                    "SELECT {CAMPAIGN_COLUMNS} FROM ad_campaigns WHERE project_id = ? \
                     ORDER BY id DESC LIMIT ?"
                ),
                state.backend,
            ))
            .bind(project_id)
            .bind(CAMPAIGNS_LIMIT)
            .fetch_all(&state.any)
            .await?
        }
        None => match principal.scoped_user_id() {
            Some(uid) => {
                sqlx::query_as(&db::sql(
                    &format!(
                        "SELECT {CAMPAIGN_COLUMNS} FROM ad_campaigns WHERE user_id = ? \
                         ORDER BY id DESC LIMIT ?"
                    ),
                    state.backend,
                ))
                .bind(uid)
                .bind(CAMPAIGNS_LIMIT)
                .fetch_all(&state.any)
                .await?
            }
            None => {
                sqlx::query_as(&db::sql(
                    &format!("SELECT {CAMPAIGN_COLUMNS} FROM ad_campaigns ORDER BY id DESC LIMIT ?"),
                    state.backend,
                ))
                .bind(CAMPAIGNS_LIMIT)
                .fetch_all(&state.any)
                .await?
            }
        },
    };

    let items: Vec<Value> = rows.iter().map(row_to_json).collect();
    Ok(Json(json!({ "campaigns": items })).into_response())
}

async fn delete_campaign(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(campaign_id): PathId<i64>,
) -> Result<Response, ApiError> {
    principal.require_master_key(NOT_MASTER)?;
    let row: CampaignRow = sqlx::query_as(&db::sql(
        &format!("SELECT {CAMPAIGN_COLUMNS} FROM ad_campaigns WHERE id = ?"),
        state.backend,
    ))
    .bind(campaign_id)
    .fetch_optional(&state.any)
    .await?
    .ok_or_else(|| ApiError::not_found("Campaign not found"))?;
    crate::identity::assert_user_row(&principal, row.user_id)?;

    // The media jobs are left alone on purpose: they are Studio's gallery too,
    // and deleting a campaign is deleting the words, not the pictures.
    sqlx::query(&db::sql("DELETE FROM ad_campaigns WHERE id = ?", state.backend))
        .bind(campaign_id)
        .execute(&state.any)
        .await?;
    // A body rather than a 204: every other delete in this API answers with
    // JSON, and the client's one delete helper parses one.
    Ok(Json(json!({ "deleted": campaign_id })).into_response())
}

async fn create_campaign(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    principal.require_master_key(NOT_MASTER)?;
    let req: CampaignCreate = crate::wire::parse_body_typed(&body)?;

    let project_id = req.project_id.ok_or_else(|| {
        ApiError::bad_request("`project_id` is required — an ad is written from a project's brand brief.")
    })?;
    crate::projects::assert_access(&state, &principal, project_id).await?;

    let platform_id = req.platform.as_deref().map(str::trim).unwrap_or("");
    let platform = find_platform(platform_id).ok_or_else(|| {
        ApiError::bad_request(format!(
            "`platform` must be one of {}, not {platform_id:?}.",
            PLATFORMS.iter().map(|p| p.id).collect::<Vec<_>>().join(", ")
        ))
    })?;

    let brief = req
        .brief
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("`brief` is required — say what this ad is about."))?;
    if brief.len() > BRIEF_MAX {
        return Err(ApiError::bad_request(format!(
            "`brief` is {} characters; the limit is {BRIEF_MAX}.",
            brief.len()
        )));
    }
    let want = req.variants.unwrap_or(DEFAULT_VARIANTS).clamp(1, MAX_VARIANTS);

    let brand = load_brand(&state, project_id).await?;
    if brand.is_empty() {
        return Err(ApiError::coded(
            StatusCode::BAD_REQUEST,
            "ads_brand_missing",
            "This project has no brand brief yet, so there is nothing for the ad to be about. \
             Fill it in first — company, what it is, and who it is for is enough to start.",
        ));
    }
    let (team_name, team_description, roster) =
        resolve_team(&state, &principal, req.team_template_id).await?;

    // Copy first, and all of it or none: a campaign with two captions where
    // three were asked for is a bug the user has to notice by counting.
    let variants = write_copy(
        &state,
        &CopyBrief { brand: &brand, brief, platform, want },
        &render_roster(&team_name, team_description.as_deref(), &roster),
    )
    .await?;

    // Then the pictures, best-effort. `start_job` reaches the backend, so this
    // is where a stopped ComfyUI shows up — and where it must not take the
    // words down with it.
    let user_id = crate::identity::stamp_user_id(&state, &principal);
    let mut stored = Vec::with_capacity(variants.len());
    for mut variant in variants {
        let spec = crate::media::JobSpec {
            kind: platform.kind,
            prompt: variant.image_prompt.clone(),
            negative: variant.negative.clone(),
            width: crate::media::snap(platform.width, platform.width),
            height: crate::media::snap(platform.height, platform.height),
            length: platform.length,
            seed: crate::media::random_seed(),
        };
        match crate::media::start_job(&state, &spec, None, &variant.image_prompt, user_id).await {
            Ok(id) => variant.media_job_id = Some(id),
            Err(e) => {
                logd!("[ads] campaign picture refused: {}", e.message);
                variant.media_error = Some(e.message.to_string());
            }
        }
        stored.push(variant);
    }

    let now = sql_now();
    let copy_json = json!({ "variants": stored }).to_string();
    let id: i64 = sqlx::query_scalar(&db::sql(
        "INSERT INTO ad_campaigns (project_id, team_template_id, platform, brief, copy_json, \
         user_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
         RETURNING CAST(id AS BIGINT)",
        state.backend,
    ))
    .bind(project_id)
    .bind(req.team_template_id)
    .bind(platform.id)
    .bind(brief)
    .bind(&copy_json)
    .bind(user_id)
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.any)
    .await?;

    let row = CampaignRow {
        id,
        project_id,
        team_template_id: req.team_template_id,
        platform: platform.id.to_string(),
        brief: brief.to_string(),
        copy_json: Some(copy_json),
        created_at: now.clone(),
        updated_at: now,
        user_id,
    };
    Ok((StatusCode::CREATED, Json(row_to_json(&row))).into_response())
}

// ---------------------------------------------------------------------------
// The copy pass
// ---------------------------------------------------------------------------

struct CopyBrief<'a> {
    brand: &'a Brand,
    brief: &'a str,
    platform: &'static PlatformSpec,
    want: i64,
}

/// One model round-trip for the whole campaign, retried once.
///
/// Not `Option` like `media::enhance_prompt`: there is no graceful degradation
/// available here. Enhancement has the user's own words to fall back on; an ad
/// campaign with no copy is nothing at all, so a failure is a named 502 the
/// screen can show rather than an empty campaign the user has to interpret.
///
/// The retry exists because local models answer contracts loosely — the second
/// attempt says so bluntly rather than hoping the first was a fluke. Two is the
/// whole ladder: a model that ignored an explicit "JSON only, no prose" is not
/// going to comply on the third ask, and the user is waiting.
async fn write_copy(
    state: &AppState,
    brief: &CopyBrief<'_>,
    roster_text: &str,
) -> Result<Vec<Variant>, ApiError> {
    let system = system_prompt(brief, roster_text);
    let user = user_prompt(brief);

    let mut last: Option<String> = None;
    for attempt in 0..2 {
        let mut payload = Map::new();
        let mut messages = vec![
            json!({ "role": "system", "content": system }),
            json!({ "role": "user", "content": user }),
        ];
        if attempt == 1 {
            messages.push(json!({
                "role": "user",
                "content": "That was not parseable JSON. Answer again with the JSON object and \
                            nothing else — no prose before it, no markdown fence around it.",
            }));
        }
        payload.insert("messages".into(), Value::Array(messages));
        // 700 a variant, not 400. Measured: a local 8B writing a full caption,
        // hashtags, a CTA and a picture prompt ran past 400 and the array came
        // back truncated — twice, so the retry did not help either. The budget
        // was the bug, not the model.
        payload.insert("max_tokens".into(), json!(700 * brief.want.max(1)));
        payload.insert("temperature".into(), json!(0.8));

        let data = match crate::llm::complete_internal(
            state,
            payload,
            crate::resources::Priority::Interactive,
        )
        .await
        {
            Ok(data) => data,
            Err(e) => {
                last = Some(format!("{e:?}"));
                continue;
            }
        };
        let message = data
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
            .and_then(|c| c.get("message"));
        // `content`, then `reasoning`. A thinking model routes its answer to a
        // separate channel and leaves `content` empty — Ollama surfaces that as
        // `message.reasoning`, and reading only `content` sees an empty string
        // and reports "no JSON object in the answer" for a model that in fact
        // answered. Cheap to check, and the alternative is a 502 nobody can
        // diagnose without the raw body.
        let content = message
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .or_else(|| message.and_then(|m| m.get("reasoning")).and_then(Value::as_str))
            .unwrap_or("");

        match parse_variants(content, brief.want) {
            Ok(variants) => return Ok(variants),
            Err(why) => {
                // The answer itself, not just the parser's complaint. A model
                // that returns the wrong *shape* and one that gets cut off
                // mid-array produce near-identical serde errors, and without
                // the text there is no way to tell which happened.
                logd!(
                    "[ads] copy attempt {} unusable: {why} -- model said: {}",
                    attempt + 1,
                    preview(content)
                );
                last = Some(why);
            }
        }
    }

    Err(ApiError::coded(
        StatusCode::BAD_GATEWAY,
        "ads_copy_failed",
        format!(
            "The model did not return usable ad copy, so nothing was generated. {}",
            last.unwrap_or_else(|| "No language model answered.".into())
        ),
    ))
}

/// The first and last of a model answer, for a log line. Both ends, because a
/// truncation is only visible at the tail and a wrong shape only at the head.
fn preview(content: &str) -> String {
    let text = content.trim();
    let head: String = text.chars().take(220).collect();
    if text.chars().count() <= 340 {
        return head;
    }
    let tail: String = text.chars().rev().take(120).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{head} ... [cut] ... {tail}")
}

/// Strip the fence a model wraps JSON in when it was told not to, then read the
/// object. Local models do this often enough that failing on it would be
/// failing on formatting rather than on content.
fn parse_variants(content: &str, want: i64) -> Result<Vec<Variant>, String> {
    let text = content.trim();
    let text = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .map(|rest| rest.trim_start().trim_end_matches('`').trim_end())
        .unwrap_or(text);
    // A model that adds a sentence before the object still gave us the object.
    let start = text.find('{').ok_or_else(|| "no JSON object in the answer".to_string())?;
    let end = text.rfind('}').ok_or_else(|| "the JSON object is unterminated".to_string())?;
    // A truncated answer is the common local-model failure: the object never
    // closes, so the whole parse fails even though the first N variants are
    // complete and usable. Try the strict parse, then salvage.
    let value: Value = match serde_json::from_str(&text[start..=end]) {
        Ok(value) => value,
        Err(strict) => match salvage_variants(&text[start..]) {
            Some(items) => json!({ "variants": items }),
            None => return Err(format!("the answer is not valid JSON: {strict}")),
        },
    };

    let raw = value
        .get("variants")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| salvage_variants(&text[start..]))
        .ok_or_else(|| "the answer has no `variants` array".to_string())?;

    let mut out = Vec::new();
    for item in raw.iter().take(want as usize) {
        let Ok(variant) = serde_json::from_value::<Variant>(item.clone()) else { continue };
        // A variant with no picture prompt cannot start a job, and one with no
        // caption is not an ad — either way it is not worth a card.
        if variant.image_prompt.trim().is_empty() || variant.caption.trim().is_empty() {
            continue;
        }
        out.push(variant);
    }
    if out.is_empty() {
        return Err("every variant was missing its caption or its picture prompt".to_string());
    }
    Ok(out)
}

/// Every complete `{...}` object inside a `variants` array, ignoring a trailing
/// partial one.
///
/// This exists because the answer that gets cut off at `max_tokens` is not
/// garbage — it is two good ads and half of a third, and throwing all three
/// away to honour "all or nothing" would be honouring the letter of that rule
/// against its point. The count the user asked for is visible on screen, so
/// fewer cards is legible; zero cards and a 502 is not.
///
/// Brace-depth scan rather than a regex: a caption may contain braces, and a
/// regex that stops at the first `}` would truncate every variant.
fn salvage_variants(text: &str) -> Option<Vec<Value>> {
    let array_start = text.find("\"variants\"").and_then(|i| text[i..].find('[').map(|j| i + j + 1))?;
    let mut out = Vec::new();
    let (mut depth, mut start, mut in_string, mut escaped) = (0usize, None, false, false);

    for (i, c) in text[array_start..].char_indices() {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if let Some(from) = start.take() {
                        let slice = &text[array_start + from..=array_start + i];
                        if let Ok(v) = serde_json::from_str::<Value>(slice) {
                            out.push(v);
                        }
                    }
                }
            }
            // The array closed cleanly; anything after it is not ours.
            ']' if depth == 0 => break,
            _ => {}
        }
    }
    (!out.is_empty()).then_some(out)
}

fn system_prompt(brief: &CopyBrief<'_>, roster_text: &str) -> String {
    let p = brief.platform;
    format!(
        "{roster_text}\n\n\
         You write social media advertisements. The team above is who you are writing as: take \
         the strategist's discipline about one idea per ad, the copywriter's voice, the art \
         director's eye for what the picture shows, and the social lead's knowledge of the \
         platform.\n\n\
         The platform is {label}. Its rules are not suggestions:\n\
         - The caption must be under {caption_limit} characters.\n\
         - At most {hashtag_max} hashtags, each one people actually follow. Fewer is better than \
           padding.\n\
         - The picture is {width}x{height}. {note}\n\n\
         For each variant write:\n\
         - `caption`: the post text. Lead with the hook, not the company name. No emoji unless \
           the brand's voice asks for them.\n\
         - `hashtags`: an array of strings, each starting with `#`.\n\
         - `cta`: one short call to action, using the brand's link if it has one.\n\
         - `image_prompt`: a diffusion prompt for the picture — subject, setting, lighting, \
           style, composition, as comma-separated phrases. Describe a photograph or an \
           illustration, never a screenshot of a post. Do not ask for words, logos or lettering \
           in the image: diffusion models render text as nonsense, and the caption carries the \
           words.\n\
         - `negative`: comma-separated things to keep out of the picture.\n\n\
         {motion}\
         Make each variant a genuinely different angle, not a reworded copy of the first.\n\n\
         Answer with ONLY a JSON object of the form \
         {{\"variants\": [{{\"caption\": \"...\", \"hashtags\": [\"#...\"], \"cta\": \"...\", \
         \"image_prompt\": \"...\", \"negative\": \"...\"}}]}} — no prose, no markdown fence.",
        label = p.label,
        caption_limit = p.caption_limit,
        hashtag_max = p.hashtag_max,
        width = p.width,
        height = p.height,
        note = p.note,
        // The clip is two seconds. A model told only "this is a video" writes a
        // scene with a beginning and an end; told the length, it writes one
        // movement — which is the only thing that fits.
        motion = if p.is_video() {
            "This is a VIDEO of about two seconds. Put one simple, slow movement in \
             `image_prompt` — a drift, a push in, steam rising. Anything needing a beginning \
             and an end will not fit.\n\n"
        } else {
            ""
        },
    )
}

fn user_prompt(brief: &CopyBrief<'_>) -> String {
    format!(
        "The brand:\n{}\n\nThis ad is about: {}\n\nWrite {} variant(s).",
        brief.brand.render(),
        brief.brief,
        brief.want
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seam snaps dimensions to multiples of 16 inside 256–2048. Every
    /// preset here is chosen to survive that untouched — a 1080 added later
    /// would be silently rendered at 1072 and only noticed when the crop looked
    /// wrong, so this fails at the moment it is written rather than in a
    /// gallery weeks later.
    #[test]
    fn every_platform_size_survives_the_seam_unchanged() {
        for p in PLATFORMS {
            assert_eq!(
                crate::media::snap(p.width, p.width),
                p.width,
                "{} width {} is rewritten by the media seam",
                p.id,
                p.width
            );
            assert_eq!(
                crate::media::snap(p.height, p.height),
                p.height,
                "{} height {} is rewritten by the media seam",
                p.id,
                p.height
            );
            assert!(p.caption_limit > 0 && p.hashtag_max >= 0, "{} has no platform rules", p.id);
            assert!(matches!(p.kind, "image" | "video"), "{} has an unknown kind", p.id);
            // `media::job_spec` clamps a video to 9..=241 frames and ignores
            // `length` entirely for an image. A preset outside that is a
            // duration the seam quietly rewrites.
            if p.is_video() {
                assert!(
                    (9..=241).contains(&p.length),
                    "{} asks for {} frames, outside the seam's 9..=241",
                    p.id,
                    p.length
                );
                assert_eq!(p.length % 4, 1, "{} is not the 4n+1 the Wan latent wants", p.id);
            } else {
                assert_eq!(p.length, 0, "{} is an image and must not ask for frames", p.id);
            }
        }
    }

    /// The size that killed ComfyUI must not come back as a video preset.
    ///
    /// Measured on an RTX 5080 (16 GB): 1088x1920 x 49 frames sampled for 3m24s
    /// and then took the whole ComfyUI process down in VAE decode — no error
    /// frame, no failed job, just a dead backend. 720x1280 survives at 7.2
    /// minutes. This asserts the ceiling rather than trusting a comment.
    #[test]
    fn no_video_preset_asks_for_a_size_that_killed_the_backend() {
        const MEASURED_VIDEO_CEILING: i64 = 720 * 1280;
        for p in PLATFORMS.iter().filter(|p| p.is_video()) {
            assert!(
                p.width * p.height <= MEASURED_VIDEO_CEILING,
                "{} is {}x{}; anything past 720x1280 was measured to kill ComfyUI outright",
                p.id,
                p.width,
                p.height
            );
        }
    }

    /// The default team is what most campaigns are written by, so it has to
    /// pass the same roster validation a user-built one does.
    #[test]
    fn the_default_roster_is_a_valid_team() {
        let roster = default_roster();
        assert!(roster.roles.len() >= 2);
        let ids: Vec<&str> = roster.roles.iter().map(|r| r.id.as_str()).collect();
        for role in &roster.roles {
            assert_eq!(role.modality, "text", "only text roles are supported");
            assert!(!role.description.trim().is_empty(), "{} says nothing", role.id);
            if let Some(parent) = &role.parent_id {
                assert!(ids.contains(&parent.as_str()), "{} has an unknown parent", role.id);
                assert_ne!(parent, &role.id, "{} is its own parent", role.id);
            }
        }
        // The rendered form is what the model actually reads.
        let text = render_roster("Social media marketing", None, &roster);
        assert!(text.contains("Copywriter") && text.contains("Art director"));
    }

    /// The three shapes a local model actually answers in: clean JSON, a fenced
    /// block, and a sentence before the object.
    #[test]
    fn copy_parses_through_a_fence_and_a_preamble() {
        // `r##` rather than `r#`: the hashtag inside would close a single-hash
        // raw string at `"#`.
        let clean = r##"{"variants":[{"caption":"c","hashtags":["#a"],"cta":"go","image_prompt":"p","negative":"n"}]}"##;
        let parsed = parse_variants(clean, 3).expect("clean JSON");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].hashtags, vec!["#a"]);
        assert_eq!(parsed[0].media_job_id, None, "the job is started after parsing, never by it");

        let fenced = format!("```json\n{clean}\n```");
        assert_eq!(parse_variants(&fenced, 3).expect("fenced JSON").len(), 1);

        let chatty = format!("Sure! Here are your ads:\n{clean}\nHope that helps.");
        assert_eq!(parse_variants(&chatty, 3).expect("preamble").len(), 1);
    }

    /// Prose instead of JSON, and JSON that is the wrong shape, both fail —
    /// they must not become an empty campaign that started zero jobs and said
    /// nothing about why.
    #[test]
    fn prose_and_the_wrong_shape_are_both_refused() {
        assert!(parse_variants("I'd be happy to help with your campaign!", 3).is_err());
        assert!(parse_variants(r#"{"ads":[]}"#, 3).is_err(), "no `variants` key");
        assert!(parse_variants(r#"{"variants":[]}"#, 3).is_err(), "an empty array is no copy");
        // A variant that could not start a job is dropped; all of them dropped
        // is a failure, not an empty success.
        assert!(
            parse_variants(r#"{"variants":[{"caption":"c","image_prompt":"  "}]}"#, 3).is_err(),
            "a variant with no picture prompt is not usable"
        );
    }

    /// The failure that actually happened on a local 8B: the array is cut off
    /// mid-object at `max_tokens`. Two complete ads and half a third must come
    /// back as two ads, not as a 502 — the count is visible on screen, so
    /// fewer cards reads correctly and zero cards does not.
    #[test]
    fn a_truncated_array_yields_the_variants_that_completed() {
        // r## again: the "#ops" hashtag would close a single-hash raw string.
        let cut = r##"{"variants":[
            {"caption":"Your worst sheet, mapped.","hashtags":["#ops"],"cta":"devstrail.com",
             "image_prompt":"a desk at dawn","negative":"text"},
            {"caption":"Second one.","hashtags":[],"cta":"go","image_prompt":"a window","negative":""},
            {"caption":"Third one, cut off here","image_pro"##;
        let parsed = parse_variants(cut, 3).expect("two of three survived");
        assert_eq!(parsed.len(), 2, "the complete objects come back");
        assert_eq!(parsed[0].caption, "Your worst sheet, mapped.");
        assert_eq!(parsed[1].caption, "Second one.");
    }

    /// A brace inside a caption must not end that variant early.
    #[test]
    fn a_brace_in_a_caption_does_not_split_a_variant() {
        let tricky = r#"{"variants":[{"caption":"use {curly} braces","image_prompt":"p"}]}"#;
        let parsed = parse_variants(tricky, 3).expect("one variant");
        assert_eq!(parsed[0].caption, "use {curly} braces");
    }

    /// More variants than asked for is a model being generous with the user's
    /// GPU. The extra ones are dropped before any job is started.
    #[test]
    fn the_variant_count_is_a_ceiling_the_model_cannot_raise() {
        let many = r#"{"variants":[
            {"caption":"a","image_prompt":"p"},
            {"caption":"b","image_prompt":"p"},
            {"caption":"c","image_prompt":"p"},
            {"caption":"d","image_prompt":"p"}
        ]}"#;
        assert_eq!(parse_variants(many, 2).expect("truncates").len(), 2);
    }

    /// A blank brief is caught before the model call, not after — the check is
    /// what stops a campaign being generated about nothing at all.
    #[test]
    fn an_empty_brand_is_empty_and_a_filled_one_renders_only_what_it_has() {
        assert!(Brand::default().is_empty());
        let brand = Brand { company: "Devstrail".into(), product: "  ".into(), ..Brand::default() };
        assert!(!brand.is_empty());
        let rendered = brand.render();
        assert!(rendered.contains("Company: Devstrail"));
        assert!(!rendered.contains("What it is"), "a blank field teaches the model to leave blanks");
    }
}
