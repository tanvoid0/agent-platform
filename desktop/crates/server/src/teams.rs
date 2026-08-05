//! Team templates, ported from `app/teams_routes.py` + `app/team_schema.py`.
//!
//! Rust owns the `teamtemplate` table. Python keeps its copy of `team_schema`
//! because the planner still renders rosters into prompts and snapshots them
//! onto processes — the shared surface is the JSON in `roster_json`, not the
//! code. `DELETE` nullifies `process.team_template_id` for the same reason
//! projects nullifies its FK: it has to happen with the delete.
//!
//! Visibility differs from projects. A `NULL` workspace is a *global* template
//! every tenant can read, so reads are "mine or global" while writes are "mine
//! only" — a workspace token may not edit a global template, and asking gets a
//! 404 rather than a 403, same as everywhere else here.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use sqlx::FromRow;

use crate::auth::Principal;
use crate::error::{ApiError, PathId};
use crate::wire::{check_len, sql_now, sql_time};
use crate::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/teams", get(list_teams).post(create_team))
        .route("/api/v1/teams/", get(list_teams).post(create_team))
        .route(
            "/api/v1/teams/{team_id}",
            get(get_team).patch(update_team).delete(delete_team),
        )
}

const DEFAULT_TEAM_COLOR: &str = "#6366f1";

/// Matches the web roster palette; the order is load-bearing because the stable
/// fallback indexes into it.
const ROSTER_ACCENT_PALETTE: [&str; 6] =
    ["#2563eb", "#16a34a", "#9333ea", "#ca8a04", "#dc2626", "#0ea5e9"];

// ---------------------------------------------------------------------------
// Roster
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RosterRole {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "text_modality")]
    pub modality: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default, deserialize_with = "blank_is_none")]
    pub accent_color: Option<String>,
}

fn text_modality() -> String {
    "text".into()
}

fn blank_is_none<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let raw: Option<String> = Option::deserialize(d)?;
    Ok(raw.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamRoster {
    pub roles: Vec<RosterRole>,
}

impl TeamRoster {
    /// Every rule from `TeamRoster`'s pydantic validators, in the same order, so
    /// a roster Python rejects is rejected here too.
    fn validate(&self, errors: &mut Vec<Value>) {
        if self.roles.is_empty() {
            errors.push(ApiError::field_error_at(
                &["roster", "roles"],
                "too_short",
                "List should have at least 1 item after validation, not 0",
            ));
            return;
        }
        if self.roles.len() > 64 {
            errors.push(ApiError::field_error_at(
                &["roster", "roles"],
                "too_long",
                "List should have at most 64 items after validation",
            ));
            return;
        }

        for (i, role) in self.roles.iter().enumerate() {
            let idx = i.to_string();
            check_len(errors, &["roster", "roles", &idx, "id"], Some(&role.id), 1, 128);
            check_len(errors, &["roster", "roles", &idx, "name"], Some(&role.name), 1, 256);
            check_len(
                errors,
                &["roster", "roles", &idx, "description"],
                Some(&role.description),
                0,
                4096,
            );
            check_len(
                errors,
                &["roster", "roles", &idx, "parent_id"],
                role.parent_id.as_deref(),
                0,
                128,
            );
            check_len(
                errors,
                &["roster", "roles", &idx, "accent_color"],
                role.accent_color.as_deref(),
                0,
                32,
            );
            if role.modality != "text" {
                errors.push(ApiError::field_error_at(
                    &["roster", "roles", &idx, "modality"],
                    "value_error",
                    "Value error, Only modality 'text' is supported until the server resolves \
                     audio, video, and image routing.",
                ));
            }
        }
        if !errors.is_empty() {
            return;
        }

        let ids: std::collections::HashSet<&str> = self.roles.iter().map(|r| r.id.as_str()).collect();
        if ids.len() != self.roles.len() {
            errors.push(graph_error("Value error, Duplicate role id"));
            return;
        }
        for role in &self.roles {
            let Some(parent) = role.parent_id.as_deref() else { continue };
            if !ids.contains(parent) {
                errors.push(graph_error(&format!(
                    "Value error, Unknown parent_id '{parent}' for role '{}'",
                    role.id
                )));
                return;
            }
            if parent == role.id {
                errors.push(graph_error("Value error, Role cannot be its own parent"));
                return;
            }
        }
        // Walk every role to a root. A cycle is otherwise only visible from
        // inside it, and a planner that follows parents would spin.
        for start in self.roles.iter().map(|r| r.id.as_str()) {
            let mut seen = std::collections::HashSet::new();
            let mut cur = Some(start);
            while let Some(id) = cur {
                if !seen.insert(id) {
                    errors.push(graph_error("Value error, Cycle in role parent graph"));
                    return;
                }
                cur = self
                    .roles
                    .iter()
                    .find(|r| r.id == id)
                    .and_then(|r| r.parent_id.as_deref());
            }
        }
    }

    /// First root in roster order; an unknown parent counts as a root, and a
    /// roster that is all cycles falls back to its first role.
    fn lead_role_id(&self) -> Option<&str> {
        let ids: std::collections::HashSet<&str> =
            self.roles.iter().map(|r| r.id.as_str()).collect();
        self.roles
            .iter()
            .find(|r| r.parent_id.as_deref().is_none_or(|p| !ids.contains(p)))
            .or_else(|| self.roles.first())
            .map(|r| r.id.as_str())
    }
}

fn graph_error(msg: &str) -> Value {
    ApiError::field_error_at(&["roster"], "value_error", msg)
}

// ---------------------------------------------------------------------------
// Colors
// ---------------------------------------------------------------------------

pub(crate) fn stable_palette_color(seed: &str) -> &'static str {
    let digest = Sha256::digest(seed.as_bytes());
    let idx = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    ROSTER_ACCENT_PALETTE[idx as usize % ROSTER_ACCENT_PALETTE.len()]
}

pub(crate) fn random_palette_color(avoid: &[String]) -> String {
    let blocked: Vec<String> = avoid.iter().map(|c| c.to_lowercase()).collect();
    let pool: Vec<&str> = ROSTER_ACCENT_PALETTE
        .iter()
        .copied()
        .filter(|c| !blocked.contains(&c.to_lowercase()))
        .collect();
    let pool = if pool.is_empty() { ROSTER_ACCENT_PALETTE.to_vec() } else { pool };
    let mut byte = [0u8; 1];
    let _ = getrandom::getrandom(&mut byte);
    pool[byte[0] as usize % pool.len()].to_string()
}

fn resolved_team_color(team_color: Option<&str>, stable_key: Option<&str>) -> String {
    let explicit = team_color.unwrap_or("").trim();
    if !explicit.is_empty() {
        return explicit.to_string();
    }
    match stable_key {
        Some(key) => stable_palette_color(&format!("team:{key}")).to_string(),
        None => DEFAULT_TEAM_COLOR.to_string(),
    }
}

/// Write path: pick a team color if none was given, then give every role
/// without an accent a distinct one. The lead deliberately inherits the team
/// color so the roster map reads as one family.
fn assign_missing_accents(roster: &TeamRoster, team_color: Option<&str>) -> (TeamRoster, String) {
    let resolved_team = match team_color.unwrap_or("").trim() {
        "" => random_palette_color(&[]),
        explicit => explicit.to_string(),
    };
    let lead = roster.lead_role_id().map(str::to_owned);
    let mut used = vec![resolved_team.to_lowercase()];
    let mut roles = Vec::with_capacity(roster.roles.len());
    for role in &roster.roles {
        if let Some(accent) = &role.accent_color {
            used.push(accent.to_lowercase());
            roles.push(role.clone());
            continue;
        }
        let accent = if Some(role.id.as_str()) == lead.as_deref() {
            resolved_team.clone()
        } else {
            random_palette_color(&used)
        };
        used.push(accent.to_lowercase());
        roles.push(RosterRole { accent_color: Some(accent), ..role.clone() });
    }
    (TeamRoster { roles }, resolved_team)
}

/// Read path: fill accents deterministically so two GETs of an old row that
/// never had colors do not disagree with each other.
fn with_default_accents(roster: &TeamRoster, team_color: Option<&str>, key: &str) -> TeamRoster {
    let mut resolved_team = team_color.unwrap_or("").trim().to_string();
    if resolved_team.is_empty() {
        resolved_team = stable_palette_color(&format!("team:{key}")).to_string();
    }
    let lead = roster.lead_role_id().map(str::to_owned);
    let roles = roster
        .roles
        .iter()
        .map(|role| {
            if role.accent_color.is_some() {
                return role.clone();
            }
            let accent = if Some(role.id.as_str()) == lead.as_deref() {
                resolved_team.clone()
            } else {
                stable_palette_color(&format!("{key}:{}", role.id)).to_string()
            };
            RosterRole { accent_color: Some(accent), ..role.clone() }
        })
        .collect();
    TeamRoster { roles }
}

// ---------------------------------------------------------------------------
// Rows and wire shapes
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct TeamRow {
    id: i64,
    workspace_id: Option<i64>,
    name: String,
    description: Option<String>,
    color: Option<String>,
    category: Option<String>,
    roster_json: String,
    created_at: String,
    updated_at: String,
}

const TEAM_COLUMNS: &str =
    "id, workspace_id, name, description, color, category, roster_json, created_at, updated_at";

#[derive(Serialize)]
struct TeamOut {
    id: i64,
    workspace_id: Option<i64>,
    name: String,
    description: Option<String>,
    color: String,
    category: Option<String>,
    roster: TeamRoster,
    role_count: usize,
    #[serde(serialize_with = "sql_time")]
    created_at: String,
    #[serde(serialize_with = "sql_time")]
    updated_at: String,
}

#[derive(Serialize)]
struct TeamSummary {
    id: i64,
    workspace_id: Option<i64>,
    name: String,
    description: Option<String>,
    /// The stored value, not the resolved one — the list has always shown the
    /// raw column while the detail view resolves it.
    color: Option<String>,
    category: Option<String>,
    role_count: usize,
    #[serde(serialize_with = "sql_time")]
    created_at: String,
    #[serde(serialize_with = "sql_time")]
    updated_at: String,
}

fn parse_roster(roster_json: &str) -> Result<TeamRoster, ApiError> {
    serde_json::from_str(roster_json).map_err(|e| {
        eprintln!("[agent-platformd] unreadable roster_json: {e}");
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "An unexpected error occurred.")
    })
}

fn row_to_out(row: TeamRow) -> Result<TeamOut, ApiError> {
    let key = row.id.to_string();
    let roster = with_default_accents(&parse_roster(&row.roster_json)?, row.color.as_deref(), &key);
    Ok(TeamOut {
        id: row.id,
        workspace_id: row.workspace_id,
        name: row.name,
        description: row.description,
        color: resolved_team_color(row.color.as_deref(), Some(&key)),
        category: row.category,
        role_count: roster.roles.len(),
        roster,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

// ---------------------------------------------------------------------------
// Access
// ---------------------------------------------------------------------------

/// Reads: a workspace token sees global templates and its own.
fn assert_visible(principal: &Principal, row: &TeamRow) -> Result<(), ApiError> {
    match principal.workspace_id {
        None => Ok(()),
        Some(ws) => match row.workspace_id {
            None => Ok(()),
            Some(owner) if owner == ws => Ok(()),
            Some(_) => Err(ApiError::not_found("Team template not found")),
        },
    }
}

/// Writes: a workspace token may only touch its own — never a global template,
/// which every other tenant is reading.
fn assert_owned(principal: &Principal, row: &TeamRow) -> Result<(), ApiError> {
    match principal.workspace_id {
        None => Ok(()),
        Some(ws) if row.workspace_id == Some(ws) => Ok(()),
        Some(_) => Err(ApiError::not_found("Team template not found")),
    }
}

async fn load_row(state: &AppState, team_id: i64) -> Result<TeamRow, ApiError> {
    sqlx::query_as(&format!("SELECT {TEAM_COLUMNS} FROM teamtemplate WHERE id = ?"))
        .bind(team_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("Team template not found"))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct TeamCreate {
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    category: Option<String>,
    roster: Option<TeamRoster>,
    /// Master-key only: `None` creates a global template.
    #[serde(default)]
    workspace_id: Option<i64>,
}

async fn list_teams(
    State(state): State<Arc<AppState>>,
    principal: Principal,
) -> Result<Response, ApiError> {
    let rows: Vec<TeamRow> = match principal.workspace_id {
        Some(ws) => sqlx::query_as(&format!(
            "SELECT {TEAM_COLUMNS} FROM teamtemplate \
             WHERE workspace_id IS NULL OR workspace_id = ? ORDER BY id ASC"
        ))
        .bind(ws)
        .fetch_all(&state.pool)
        .await?,
        None => sqlx::query_as(&format!(
            "SELECT {TEAM_COLUMNS} FROM teamtemplate ORDER BY id ASC"
        ))
        .fetch_all(&state.pool)
        .await?,
    };

    let teams: Vec<TeamSummary> = rows
        .into_iter()
        .map(|row| TeamSummary {
            // A row whose roster will not parse still lists, with zero roles.
            // Python swallows the same error here and only raises on the detail
            // view, and a library page that 500s because one row is corrupt is
            // worse than one that shows it as empty.
            role_count: parse_roster(&row.roster_json).map(|r| r.roles.len()).unwrap_or(0),
            id: row.id,
            workspace_id: row.workspace_id,
            name: row.name,
            description: row.description,
            color: row.color,
            category: row.category,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
        .collect();

    Ok(Json(json!({ "teams": teams })).into_response())
}

async fn create_team(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    body: Option<Json<TeamCreate>>,
) -> Result<Response, ApiError> {
    let Json(req) = body.ok_or_else(|| missing_fields())?;

    let mut errors = Vec::new();
    match req.name.as_deref() {
        None => errors.push(ApiError::field_error("name", "missing", "Field required")),
        Some(name) => check_len(&mut errors, &["name"], Some(name), 1, 256),
    }
    check_len(&mut errors, &["description"], req.description.as_deref(), 0, 4096);
    check_len(&mut errors, &["color"], req.color.as_deref(), 0, 32);
    check_len(&mut errors, &["category"], req.category.as_deref(), 0, 128);
    match &req.roster {
        None => errors.push(ApiError::field_error("roster", "missing", "Field required")),
        Some(roster) => roster.validate(&mut errors),
    }
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }
    let roster = req.roster.expect("checked above");

    let workspace_id = match principal.workspace_id {
        Some(ws) => Some(ws),
        None => {
            if let Some(ws) = req.workspace_id {
                let archived: Option<Option<NaiveDateTime>> =
                    sqlx::query_scalar("SELECT archived_at FROM workspace WHERE id = ?")
                        .bind(ws)
                        .fetch_optional(&state.pool)
                        .await?;
                if !matches!(archived, Some(None)) {
                    return Err(ApiError::not_found("Workspace not found"));
                }
            }
            req.workspace_id
        }
    };

    let (roster, team_color) = assign_missing_accents(&roster, req.color.as_deref());
    let now = sql_now();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO teamtemplate \
         (workspace_id, name, description, color, category, roster_json, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(workspace_id)
    .bind(req.name.unwrap_or_default().trim())
    .bind(trimmed(req.description))
    .bind(&team_color)
    .bind(trimmed(req.category))
    .bind(serde_json::to_string(&roster).unwrap_or_else(|_| "{\"roles\":[]}".into()))
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.pool)
    .await?;

    Ok((StatusCode::CREATED, Json(row_to_out(load_row(&state, id).await?)?)).into_response())
}

fn missing_fields() -> ApiError {
    ApiError::validation(vec![
        ApiError::field_error("name", "missing", "Field required"),
        ApiError::field_error("roster", "missing", "Field required"),
    ])
}

fn trimmed(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

async fn get_team(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(team_id): PathId<i64>,
) -> Result<Response, ApiError> {
    let row = load_row(&state, team_id).await?;
    assert_visible(&principal, &row)?;
    Ok(Json(row_to_out(row)?).into_response())
}

async fn update_team(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(team_id): PathId<i64>,
    body: Option<Json<Value>>,
) -> Result<Response, ApiError> {
    let row = load_row(&state, team_id).await?;
    assert_owned(&principal, &row)?;

    let patch: Map<String, Value> = match body {
        Some(Json(Value::Object(map))) => map,
        _ => Map::new(),
    };
    let field = |key: &str| -> Option<String> {
        patch.get(key).and_then(Value::as_str).map(str::to_owned)
    };

    let roster: Option<TeamRoster> = match patch.get("roster") {
        Some(raw) if !raw.is_null() => Some(serde_json::from_value(raw.clone()).map_err(|e| {
            ApiError::validation(vec![ApiError::field_error_at(
                &["roster"],
                "model_attributes_type",
                &format!("Input should be a valid roster: {e}"),
            )])
        })?),
        _ => None,
    };

    let mut errors = Vec::new();
    check_len(&mut errors, &["name"], field("name").as_deref(), 1, 256);
    check_len(&mut errors, &["description"], field("description").as_deref(), 0, 4096);
    check_len(&mut errors, &["color"], field("color").as_deref(), 0, 32);
    check_len(&mut errors, &["category"], field("category").as_deref(), 0, 128);
    if let Some(roster) = &roster {
        roster.validate(&mut errors);
    }
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    let mut color = row.color.clone();
    if let Some(name) = field("name") {
        sqlx::query("UPDATE teamtemplate SET name = ? WHERE id = ?")
            .bind(name.trim())
            .bind(team_id)
            .execute(&state.pool)
            .await?;
    }
    if patch.get("description").is_some_and(|v| !v.is_null()) {
        sqlx::query("UPDATE teamtemplate SET description = ? WHERE id = ?")
            .bind(trimmed(field("description")))
            .bind(team_id)
            .execute(&state.pool)
            .await?;
    }
    if patch.get("color").is_some_and(|v| !v.is_null()) {
        // An explicit empty color is a request for a new random one, not for a
        // colorless team.
        color = Some(match field("color").unwrap_or_default().trim() {
            "" => random_palette_color(&[]),
            explicit => explicit.to_string(),
        });
        sqlx::query("UPDATE teamtemplate SET color = ? WHERE id = ?")
            .bind(color.as_deref())
            .bind(team_id)
            .execute(&state.pool)
            .await?;
    }
    // Category is the one field where an explicit `null` clears rather than
    // being ignored — presence in the body is what counts.
    if patch.contains_key("category") {
        sqlx::query("UPDATE teamtemplate SET category = ? WHERE id = ?")
            .bind(trimmed(field("category")))
            .bind(team_id)
            .execute(&state.pool)
            .await?;
    }
    if let Some(roster) = roster {
        let (roster, team_color) = assign_missing_accents(&roster, color.as_deref());
        sqlx::query("UPDATE teamtemplate SET roster_json = ?, color = ? WHERE id = ?")
            .bind(serde_json::to_string(&roster).unwrap_or_else(|_| "{\"roles\":[]}".into()))
            .bind(&team_color)
            .bind(team_id)
            .execute(&state.pool)
            .await?;
    }
    sqlx::query("UPDATE teamtemplate SET updated_at = ? WHERE id = ?")
        .bind(sql_now())
        .bind(team_id)
        .execute(&state.pool)
        .await?;

    Ok(Json(row_to_out(load_row(&state, team_id).await?)?).into_response())
}

async fn delete_team(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(team_id): PathId<i64>,
) -> Result<Response, ApiError> {
    let row = load_row(&state, team_id).await?;
    assert_owned(&principal, &row)?;

    let mut tx = state.pool.begin().await?;
    // The snapshot on each process keeps the roster readable after the template
    // is gone, so only the pointer is cleared.
    sqlx::query("UPDATE process SET team_template_id = NULL WHERE team_template_id = ?")
        .bind(team_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM teamtemplate WHERE id = ?")
        .bind(team_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(Json(json!({ "ok": true })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stable fallback has to agree with Python's, or a roster's colors
    /// change the day this server starts answering.
    #[test]
    fn stable_palette_matches_python() {
        // sha256(seed)[:4] big-endian % 6, values taken from hashlib.
        assert_eq!(stable_palette_color("team:1"), "#16a34a");
        assert_eq!(stable_palette_color("team:2"), "#9333ea");
        assert_eq!(stable_palette_color("1:lead"), "#ca8a04");
        assert_eq!(stable_palette_color("1:b"), "#9333ea");
    }

    #[test]
    fn lead_is_the_first_root() {
        let roster: TeamRoster = serde_json::from_str(
            r#"{"roles":[{"id":"b","name":"B","parent_id":"a"},{"id":"a","name":"A"}]}"#,
        )
        .unwrap();
        assert_eq!(roster.lead_role_id(), Some("a"));
    }

    #[test]
    fn cycles_and_unknown_parents_are_rejected() {
        let cyclic: TeamRoster = serde_json::from_str(
            r#"{"roles":[{"id":"a","name":"A","parent_id":"b"},{"id":"b","name":"B","parent_id":"a"}]}"#,
        )
        .unwrap();
        let mut errors = Vec::new();
        cyclic.validate(&mut errors);
        assert!(!errors.is_empty(), "a parent cycle must not validate");

        let orphan: TeamRoster =
            serde_json::from_str(r#"{"roles":[{"id":"x","name":"X","parent_id":"nope"}]}"#).unwrap();
        let mut errors = Vec::new();
        orphan.validate(&mut errors);
        assert!(!errors.is_empty(), "an unknown parent must not validate");
    }
}
