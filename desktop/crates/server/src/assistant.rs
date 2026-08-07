//! Assistant reads, ported from `app/assistant/routes.py` + services.
//!
//! **Scope: the whole assistant domain bar one route** — ADR 0007 step 4's
//! ordered list in `plan.md`, sub-steps 4 through 9: the reads,
//! `PATCH /profile/{domain}`, `POST /chat/send`, `chat/apply` +
//! `reviews/run`/`apply`/`dismiss` + `items/{id}/complete`, `chat/retry` +
//! `chat/submit-form`, and `POST /reset`.
//!
//! `POST /chat/threads` is the one path still handed to Python, and by choice
//! rather than by blocker: it shares its path with the `GET` this module owns,
//! so it is declared to `proxy::forward` explicitly (leaving it to the
//! router's fallback would answer 405 instead of falling through).
//!
//! `chat/apply` and `reviews/{id}/apply` both close through
//! [`apply_board_actions`], the board-scoped twin of `todos.rs`'s per-item
//! `agent_apply` — the two look alike but are not the same function ported
//! twice: `board_action_apply.py`'s `_apply_item_action` skips
//! `export_markdown_checklist`/`export_ics_event` (unsupported at board
//! scope) and its `break_down_task` has no `grocery_groups` branch, both
//! matched here rather than "fixed".
//!
//! `dashboard` and `goals` both call [`ensure_assistant_board`], which
//! find-or-creates the Personal Assistant's `TodoBoard` and shares
//! `todos.rs`'s `ItemRow`/`ItemOut`/`CategoryRow`/`CategoryOut` (now
//! `pub(crate)`) rather than re-querying `todo_items`/`todo_categories` with a
//! second copy of the palette-fallback and JSON-column decoding they already
//! carry.
//!
//! `PATCH /profile/{domain}` closes `assistant_domain_profiles`' two-writer:
//! [`crate::todos::merge_domain_profile`] already wrote this table from
//! `agent/apply`'s `store_user_profile` action, so this route calls that
//! function rather than re-porting `user_profile_service.merge_profile` —
//! `plan.md` names them the same function on purpose.
//!
//! **No assistant route calls `require_scope`.** Access is `project_id` (a
//! required query param, `ge=1`) plus [`crate::projects::assert_access`], the
//! same check `assert_token_project_access` is elsewhere in this crate
//! (`plan.md`, `processes.rs` notes).

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Duration, NaiveDateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::FromRow;

use crate::action_orchestrator::{build_action_tools, decide_actions, list_actions, py_truthy, PlannedAction};
use crate::auth::Principal;
use crate::chat_thread_title::{
    await_smart_title, fallback_title_from_message, is_placeholder_title, start_smart_title_task,
    DEFAULT_PLACEHOLDERS,
};
use crate::chat_usage::{estimate_context_usage, merge_llm_usages, ContextInputs, ContextUsageOut, LlmStepUsageOut};
use crate::clarifying_form::is_clarifying_form;
use crate::dag_schema::sanitize_llm_model_alias;
use crate::db;
use crate::error::ApiError;
use crate::todos::{
    py_repr, CategoryOut, CategoryRow, ItemOut, ItemRow, ProfileRow as PlannerProfileRow,
    CATEGORY_COLUMNS, ITEM_COLUMNS, PROFILE_COLUMNS,
};
use crate::wire::{iso_from_sql, iso_string, parse_naive, sql_now};
use crate::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/assistant/dashboard", get(dashboard))
        .route("/api/v1/assistant/goals", get(goals))
        // `POST /chat/threads` and `PATCH /profile/{domain}` share a path with
        // a route Rust now owns and would otherwise 405 instead of falling
        // through to Python — same trap `processes.rs` hit porting `list`
        // ahead of `start`; declared here explicitly rather than left to the
        // router's fallback, which only fires on a path with no match at all.
        .route(
            "/api/v1/assistant/chat/threads",
            get(chat_threads_list).post(crate::proxy::forward),
        )
        .route("/api/v1/assistant/chat/context-usage", get(chat_context_usage))
        .route("/api/v1/assistant/chat/thread", get(chat_thread))
        .route("/api/v1/assistant/chat/send", post(chat_send))
        .route("/api/v1/assistant/profile", get(list_profiles))
        .route("/api/v1/assistant/profile/forms", get(list_profile_forms))
        .route(
            "/api/v1/assistant/profile/{domain}",
            get(get_domain_profile).patch(patch_domain_profile),
        )
        .route("/api/v1/assistant/reset", post(assistant_reset))
        .route("/api/v1/assistant/chat/apply", post(chat_apply))
        .route("/api/v1/assistant/chat/retry", post(chat_retry))
        .route("/api/v1/assistant/chat/submit-form", post(chat_submit_form))
        .route("/api/v1/assistant/items/{item_id}/complete", post(complete_item))
        .route("/api/v1/assistant/reviews/run", post(reviews_run))
        .route("/api/v1/assistant/reviews/pending", get(reviews_pending))
        .route("/api/v1/assistant/reviews/{review_id}/apply", post(reviews_apply))
        .route("/api/v1/assistant/reviews/{review_id}/dismiss", post(reviews_dismiss))
}

// ---------------------------------------------------------------------------
// The Personal Assistant board — `assistant_service.py::ensure_assistant_board`
// ---------------------------------------------------------------------------

const ASSISTANT_BOARD_NAME: &str = "Personal Assistant";
const ASSISTANT_BOARD_DESCRIPTION: &str =
    "Your personal planning board — agents organize, you execute.";
const ASSISTANT_TEMPLATE_SLUG: &str = "personal-assistant";

/// Find-or-create the project's Personal Assistant board. `project.
/// assistant_board_id` is the fast path; a same-named board or a fresh one
/// (from the `personal-assistant` template) are the fallbacks, in that order —
/// each of which writes the pointer back so the next call takes the fast path.
async fn ensure_assistant_board(state: &AppState, project_id: i64) -> Result<i64, ApiError> {
    let pointer: Option<Option<i64>> = sqlx::query_scalar(&db::sql(
        "SELECT assistant_board_id FROM project WHERE id = ?",
        state.backend,
    ))
    .bind(project_id)
    .fetch_optional(&state.any)
    .await?;
    let Some(pointer) = pointer else {
        return Err(ApiError::not_found("Project not found"));
    };
    if let Some(board_id) = pointer {
        let owner: Option<i64> = sqlx::query_scalar("SELECT project_id FROM todo_boards WHERE id = ?")
            .bind(board_id)
            .fetch_optional(&state.pool)
            .await?;
        if owner == Some(project_id) {
            return Ok(board_id);
        }
    }

    let named: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM todo_boards WHERE project_id = ? AND name = ?",
    )
    .bind(project_id)
    .bind(ASSISTANT_BOARD_NAME)
    .fetch_optional(&state.pool)
    .await?;

    let board_id = match named {
        Some(id) => id,
        None => {
            let now = sql_now();
            let new_id: i64 = sqlx::query_scalar(
                "INSERT INTO todo_boards \
                 (project_id, name, description, default_model, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
            )
            .bind(project_id)
            .bind(ASSISTANT_BOARD_NAME)
            .bind(ASSISTANT_BOARD_DESCRIPTION)
            .bind(crate::todos::default_board_model())
            .bind(&now)
            .bind(&now)
            .fetch_one(&state.pool)
            .await?;
            crate::todos::apply_board_template(state, new_id, ASSISTANT_TEMPLATE_SLUG).await?;
            new_id
        }
    };

    let now = sql_now();
    sqlx::query(&db::sql(
        "UPDATE project SET assistant_board_id = ?, updated_at = ? WHERE id = ?",
        state.backend,
    ))
    .bind(board_id)
    .bind(&now)
    .bind(project_id)
    .execute(&state.any)
    .await?;

    Ok(board_id)
}

fn horizon_range(horizon: &str, now: NaiveDateTime) -> (NaiveDateTime, NaiveDateTime) {
    let start = now.date().and_hms_opt(0, 0, 0).expect("midnight is always valid");
    let end = match horizon {
        "day" => start + Duration::days(1),
        "week" => start + Duration::days(7),
        "month" => start + Duration::days(30),
        _ => start + Duration::days(1),
    };
    (start, end)
}

fn item_in_horizon(item: &ItemRow, horizon: &str, now: NaiveDateTime) -> bool {
    if item.time_horizon.as_deref() == Some(horizon) {
        return true;
    }
    if item.time_horizon.as_deref() == Some("goal") && horizon == "month" {
        return item.item_kind.as_deref() == Some("goal");
    }
    let (start, end) = horizon_range(horizon, now);
    for raw in [&item.scheduled_at, &item.due_at] {
        if let Some(dt) = raw.as_deref().and_then(parse_naive) {
            if dt >= start && dt < end {
                return true;
            }
        }
    }
    if horizon == "day"
        && matches!(item.status.as_str(), "in_progress" | "plan")
        && item.parent_item_id.is_none()
        && matches!(item.time_horizon.as_deref(), None | Some("day") | Some("week"))
    {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// GET /assistant/dashboard, GET /assistant/goals
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct DashboardQuery {
    project_id: Option<String>,
    #[serde(default = "default_horizon")]
    horizon: String,
}

fn default_horizon() -> String {
    "day".to_string()
}

async fn dashboard(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<DashboardQuery>,
) -> Result<Response, ApiError> {
    let project_id = require_project(&state, &principal, q.project_id).await?;
    let board_id = ensure_assistant_board(&state, project_id).await?;
    let now = Utc::now().naive_utc();
    let (start, end) = horizon_range(&q.horizon, now);

    let items: Vec<ItemRow> = sqlx::query_as(&format!(
        "SELECT {ITEM_COLUMNS} FROM todo_items WHERE board_id = ?"
    ))
    .bind(board_id)
    .fetch_all(&state.pool)
    .await?;

    let mut overdue = Vec::new();
    let mut habits_due = Vec::new();
    let mut goal_items = Vec::new();
    let mut top_level = Vec::new();
    let mut subtasks_by_parent: Map<String, Value> = Map::new();

    let done_count = items.iter().filter(|i| i.status == "done").count();
    let active_count = items.len() - done_count;

    for item in items {
        let is_goal = item.item_kind.as_deref() == Some("goal") || item.time_horizon.as_deref() == Some("goal");
        let is_habit_due = item.item_kind.as_deref() == Some("habit") && item.status != "done";
        let is_overdue = item
            .due_at
            .as_deref()
            .and_then(parse_naive)
            .is_some_and(|dt| dt < now)
            && item.status != "done";
        let in_horizon = item_in_horizon(&item, &q.horizon, now);
        let parent_item_id = item.parent_item_id;

        if is_goal {
            goal_items.push(json_item(&item));
        }
        if is_habit_due {
            habits_due.push(json_item(&item));
        }
        if is_overdue {
            overdue.push(json_item(&item));
        }
        if in_horizon {
            let out: Value = json_item(&item);
            match parent_item_id {
                Some(pid) => {
                    let key = pid.to_string();
                    match subtasks_by_parent.get_mut(&key) {
                        Some(Value::Array(arr)) => arr.push(out),
                        _ => {
                            subtasks_by_parent.insert(key, Value::Array(vec![out]));
                        }
                    }
                }
                None => top_level.push(out),
            }
        }
    }

    let categories: Vec<CategoryRow> = sqlx::query_as(&format!(
        "SELECT {CATEGORY_COLUMNS} FROM todo_categories WHERE board_id = ? ORDER BY sort_order ASC"
    ))
    .bind(board_id)
    .fetch_all(&state.pool)
    .await?;
    let categories: Vec<CategoryOut> = categories.into_iter().map(CategoryOut::from).collect();

    Ok(Json(json!({
        "project_id": project_id,
        "board_id": board_id,
        "horizon": q.horizon,
        "range_start": iso_string(start),
        "range_end": iso_string(end),
        "categories": categories,
        "items": top_level,
        "subtasks_by_parent": subtasks_by_parent,
        "overdue": overdue,
        "habits_due": habits_due,
        "goals": goal_items,
        "stats": {
            "total_items": done_count + active_count,
            "done_count": done_count,
            "active_count": active_count,
            "overdue_count": overdue.len(),
            "habits_due_count": habits_due.len(),
        },
    }))
    .into_response())
}

/// `_item_out(item)` rendered straight to JSON — the dashboard buckets the same
/// row into up to four lists, so this clones (`ItemRow` is cheap and `Clone`
/// for exactly this) rather than constructing `ItemOut`, a move, four times.
fn json_item(item: &ItemRow) -> Value {
    serde_json::to_value(ItemOut::from(item.clone())).expect("ItemOut always serializes")
}

async fn goals(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<ProjectIdQuery>,
) -> Result<Response, ApiError> {
    let project_id = require_project(&state, &principal, q.project_id).await?;
    let board_id = ensure_assistant_board(&state, project_id).await?;

    let items: Vec<ItemRow> = sqlx::query_as(&format!(
        "SELECT {ITEM_COLUMNS} FROM todo_items WHERE board_id = ? AND item_kind = 'goal'"
    ))
    .bind(board_id)
    .fetch_all(&state.pool)
    .await?;
    let items: Vec<ItemOut> = items.into_iter().map(ItemOut::from).collect();

    Ok(Json(json!({ "goals": items })).into_response())
}

// ---------------------------------------------------------------------------
// Access — `project_id: int = Query(..., ge=1)` plus project access
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ProjectIdQuery {
    project_id: Option<String>,
}

/// FastAPI's `Query(..., ge=1)` dependency every assistant route shares.
/// Reproduced by hand because axum's own `Query<i64>` rejection does not carry
/// this shape. The `input`/`ctx` fields of the pydantic entry are a known gap
/// (see `error.rs::field_error_at`).
fn parse_project_id(raw: Option<String>) -> Result<i64, ApiError> {
    let missing = || {
        ApiError::validation(vec![json!({
            "type": "missing", "loc": ["query", "project_id"], "msg": "Field required",
        })])
    };
    let raw = match raw {
        Some(r) if !r.is_empty() => r,
        _ => return Err(missing()),
    };
    match raw.parse::<i64>() {
        Ok(id) if id >= 1 => Ok(id),
        Ok(_) => Err(ApiError::validation(vec![json!({
            "type": "greater_than_equal", "loc": ["query", "project_id"],
            "msg": "Input should be greater than or equal to 1",
        })])),
        Err(_) => Err(ApiError::validation(vec![json!({
            "type": "int_parsing", "loc": ["query", "project_id"],
            "msg": "Input should be a valid integer, unable to parse string as an integer",
        })])),
    }
}

async fn require_project(
    state: &AppState,
    principal: &Principal,
    raw: Option<String>,
) -> Result<i64, ApiError> {
    let project_id = parse_project_id(raw)?;
    crate::projects::assert_access(state, principal, project_id).await?;
    Ok(project_id)
}

// ---------------------------------------------------------------------------
// JSON columns — best-effort decode, same discipline as `todos.rs`
// ---------------------------------------------------------------------------

fn json_object(raw: Option<String>) -> Map<String, Value> {
    raw.and_then(|r| serde_json::from_str::<Value>(&r).ok())
        .and_then(|v| match v {
            Value::Object(o) => Some(o),
            _ => None,
        })
        .unwrap_or_default()
}

fn json_array(raw: Option<String>) -> Vec<Value> {
    raw.and_then(|r| serde_json::from_str::<Value>(&r).ok())
        .and_then(|v| match v {
            Value::Array(a) => Some(a),
            _ => None,
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// GET /assistant/chat/threads
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct ThreadRow {
    id: i64,
    title: Option<String>,
    messages_json: Option<String>,
    created_at: String,
    updated_at: String,
}

async fn chat_threads_list(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<ProjectIdQuery>,
) -> Result<Response, ApiError> {
    let project_id = require_project(&state, &principal, q.project_id).await?;

    let rows: Vec<ThreadRow> = sqlx::query_as(&db::sql(
        "SELECT id, title, messages_json, \
         CAST(created_at AS TEXT) AS created_at, CAST(updated_at AS TEXT) AS updated_at \
         FROM assistant_chat_threads WHERE project_id = ? ORDER BY updated_at DESC",
        state.backend,
    ))
    .bind(project_id)
    .fetch_all(&state.any)
    .await?;

    let threads: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            let messages = json_array(row.messages_json);
            // `next((m for m in reversed(messages) if m["role"] == "user" and
            // m["content"]), None)` — a non-string content (rare) is skipped
            // rather than `str()`-ed, a narrower gap than the rest of this port.
            let preview = messages
                .iter()
                .rev()
                .find_map(|m| {
                    let obj = m.as_object()?;
                    if obj.get("role").and_then(Value::as_str) != Some("user") {
                        return None;
                    }
                    let content = obj.get("content")?.as_str()?;
                    if content.is_empty() {
                        return None;
                    }
                    Some(content.chars().take(120).collect::<String>())
                })
                .unwrap_or_default();
            let title = row
                .title
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "New chat".to_string());
            json!({
                "id": row.id,
                "project_id": project_id,
                "title": title,
                "message_count": messages.len(),
                "preview": preview,
                "created_at": iso_from_sql(&row.created_at),
                "updated_at": iso_from_sql(&row.updated_at),
            })
        })
        .collect();

    Ok(Json(json!({ "project_id": project_id, "threads": threads })).into_response())
}

// ---------------------------------------------------------------------------
// GET /assistant/profile, /assistant/profile/{domain}
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct ProfileRow {
    domain: String,
    profile_json: Option<String>,
}

/// `get_all_profiles`: every `assistant_domain_profiles` row for a project, as
/// `{domain: profile}`. Shared by the `GET /profile` route and
/// `build_profile_context`'s turn-generation path — one query either way.
async fn all_profiles(state: &AppState, project_id: i64) -> Result<Map<String, Value>, ApiError> {
    let rows: Vec<ProfileRow> = sqlx::query_as(&db::sql(
        "SELECT domain, profile_json FROM assistant_domain_profiles WHERE project_id = ?",
        state.backend,
    ))
    .bind(project_id)
    .fetch_all(&state.any)
    .await?;

    let mut profiles = Map::new();
    for row in rows {
        profiles.insert(row.domain, Value::Object(json_object(row.profile_json)));
    }
    Ok(profiles)
}

async fn list_profiles(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<ProjectIdQuery>,
) -> Result<Response, ApiError> {
    let project_id = require_project(&state, &principal, q.project_id).await?;
    let profiles = all_profiles(&state, project_id).await?;
    Ok(Json(json!({ "project_id": project_id, "profiles": profiles })).into_response())
}

#[derive(FromRow)]
struct ProfileJsonOnly {
    profile_json: Option<String>,
}

async fn get_domain_profile(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Path(domain): Path<String>,
    Query(q): Query<ProjectIdQuery>,
) -> Result<Response, ApiError> {
    let project_id = require_project(&state, &principal, q.project_id).await?;

    let row: Option<ProfileJsonOnly> = sqlx::query_as(&db::sql(
        "SELECT profile_json FROM assistant_domain_profiles WHERE project_id = ? AND domain = ?",
        state.backend,
    ))
    .bind(project_id)
    .bind(&domain)
    .fetch_optional(&state.any)
    .await?;

    let profile = row.map(|r| json_object(r.profile_json)).unwrap_or_default();
    Ok(Json(json!({ "project_id": project_id, "domain": domain, "profile": profile })).into_response())
}

#[derive(Deserialize)]
struct ProfilePatchBody {
    #[serde(default)]
    profile: Map<String, Value>,
}

async fn patch_domain_profile(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Path(domain): Path<String>,
    Query(q): Query<ProjectIdQuery>,
    body: Option<Json<ProfilePatchBody>>,
) -> Result<Response, ApiError> {
    let project_id = require_project(&state, &principal, q.project_id).await?;
    // `DomainProfilePatch` has no required fields, but the body itself is —
    // FastAPI 422s a request with no body at all rather than defaulting it.
    let Some(Json(body)) = body else {
        return Err(ApiError::validation(vec![json!({
            "type": "missing", "loc": ["body"], "msg": "Field required",
        })]));
    };

    let profile = crate::todos::merge_domain_profile(&state, project_id, &domain, &body.profile).await?;
    Ok(Json(json!({ "project_id": project_id, "domain": domain, "profile": profile })).into_response())
}

// ---------------------------------------------------------------------------
// GET /assistant/profile/forms — a constant table, like `todos.rs`'s board
// templates. `list_profile_domains()`'s UI order, not `_DOMAIN_FORMS`'s
// definition order, is what the JSON below is keyed in.
// ---------------------------------------------------------------------------

async fn list_profile_forms(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<ProjectIdQuery>,
) -> Result<Response, ApiError> {
    let project_id = require_project(&state, &principal, q.project_id).await?;
    let forms: Value = serde_json::from_str(DOMAIN_FORMS_JSON).expect("DOMAIN_FORMS_JSON is valid");
    Ok(Json(json!({ "project_id": project_id, "forms": forms })).into_response())
}

const DOMAIN_FORMS_JSON: &str = r#"{
  "general": {
    "title": "About you",
    "description": "Basics shared across assistants — name, location, and free-form notes.",
    "domain": "general",
    "fields": [
      {"id": "display_name", "label": "Preferred name", "kind": "text", "required": false, "placeholder": "e.g. Alex"},
      {"id": "pronouns", "label": "Pronouns", "kind": "single_select", "options": ["she/her", "he/him", "they/them", "Prefer not to say"], "required": false},
      {"id": "timezone", "label": "Timezone", "kind": "text", "required": false, "placeholder": "e.g. America/New_York"},
      {"id": "home_location", "label": "Home base", "kind": "text", "required": false, "placeholder": "City or region"},
      {"id": "personal_notes", "label": "Anything else assistants should know", "kind": "textarea", "required": false, "placeholder": "Schedule constraints, dependents, accessibility needs, etc."}
    ]
  },
  "fitness": {
    "title": "Fitness profile",
    "description": "A few details so your workout plan fits you — saved for future sessions.",
    "domain": "fitness",
    "fields": [
      {"id": "sex", "label": "Sex", "kind": "single_select", "options": ["Female", "Male", "Non-binary", "Prefer not to say"], "required": true, "helpText": "Used for training volume and recovery guidance."},
      {"id": "age", "label": "Age", "kind": "text", "required": true, "placeholder": "e.g. 32"},
      {"id": "height_cm", "label": "Height (cm)", "kind": "text", "required": true, "placeholder": "e.g. 175"},
      {"id": "weight_kg", "label": "Weight (kg)", "kind": "text", "required": true, "placeholder": "e.g. 70"},
      {"id": "fitness_goal", "label": "Primary goal", "kind": "single_select", "options": ["Lose weight", "Build muscle", "General fitness", "Train for event", "Mobility & recovery"], "required": true},
      {"id": "experience_level", "label": "Experience", "kind": "single_select", "options": ["Beginner", "Intermediate", "Advanced"], "required": true},
      {"id": "equipment", "label": "Equipment available", "kind": "multi_select", "options": ["Gym full access", "Home dumbbells", "Resistance bands", "Bodyweight only", "Outdoor running"], "required": true},
      {"id": "injuries", "label": "Injuries or limits (optional)", "kind": "text", "required": false, "placeholder": "e.g. bad knee, lower back"}
    ]
  },
  "nutrition": {
    "title": "Nutrition preferences",
    "description": "Saved to personalize meal plans and shopping lists.",
    "domain": "nutrition",
    "fields": [
      {"id": "diet_style", "label": "Diet style", "kind": "single_select", "options": ["Omnivore", "Vegetarian", "Vegan", "Pescatarian", "Keto", "Mediterranean", "Other"], "required": true},
      {"id": "dietary_requirements", "label": "Religious / cultural dietary rules", "kind": "multi_select", "options": ["Halal", "Kosher", "No pork", "No beef"], "required": false, "helpText": "Applied on top of your diet style when planning meals and shopping lists."},
      {"id": "allergies", "label": "Allergies / avoid", "kind": "text", "required": false, "placeholder": "e.g. nuts, dairy, shellfish"},
      {"id": "meals_per_day", "label": "Meals per day", "kind": "single_select", "options": ["2", "3", "4+"], "required": true},
      {"id": "cooking_time_minutes", "label": "Typical cooking time", "kind": "single_select", "options": ["15 min or less", "30 min", "45+ min", "Meal prep batches"], "required": true}
    ]
  },
  "travel": {
    "title": "Trip details",
    "description": "Tell me about the trip — I'll remember this for packing and bookings.",
    "domain": "travel",
    "fields": [
      {"id": "destination", "label": "Destination", "kind": "text", "required": true, "placeholder": "City, country, or region"},
      {"id": "departure_date", "label": "Departure date", "kind": "text", "required": true, "placeholder": "YYYY-MM-DD"},
      {"id": "return_date", "label": "Return date", "kind": "text", "required": false, "placeholder": "YYYY-MM-DD"},
      {"id": "travelers", "label": "Travelers", "kind": "single_select", "options": ["Just me", "Couple", "Family with kids", "Group of friends"], "required": true},
      {"id": "budget", "label": "Budget (approx.)", "kind": "single_select", "options": ["Budget", "Mid-range", "Comfort", "Luxury", "Flexible"], "required": true},
      {"id": "travel_style", "label": "Trip style", "kind": "multi_select", "options": ["Sightseeing", "Food & culture", "Relaxation", "Adventure", "Business"], "required": true},
      {"id": "notes", "label": "Must-haves or constraints", "kind": "text", "required": false, "placeholder": "Dietary needs, mobility, visa, etc."}
    ]
  },
  "finance": {
    "title": "Finance snapshot",
    "description": "Helps tailor budgets and savings tasks — stored on your project.",
    "domain": "finance",
    "fields": [
      {"id": "monthly_budget", "label": "Monthly budget focus", "kind": "single_select", "options": ["Tight", "Moderate", "Comfortable", "Not sure"], "required": true},
      {"id": "savings_goal", "label": "Savings goal", "kind": "text", "required": false, "placeholder": "e.g. emergency fund, vacation"},
      {"id": "primary_focus", "label": "Primary focus", "kind": "multi_select", "options": ["Bills", "Debt payoff", "Saving", "Investing", "Tracking spending"], "required": true}
    ]
  },
  "professional": {
    "title": "Career & growth",
    "description": "Saved to personalize professional development plans.",
    "domain": "professional",
    "fields": [
      {"id": "current_role", "label": "Current role", "kind": "text", "required": true},
      {"id": "target_role", "label": "Target (6–12 months)", "kind": "text", "required": false},
      {"id": "growth_focus", "label": "Growth focus", "kind": "multi_select", "options": ["Skills", "Leadership", "Networking", "Certifications", "Job search"], "required": true}
    ]
  }
}"#;

// ---------------------------------------------------------------------------
// GET /assistant/reviews/pending
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct ReviewRow {
    id: i64,
    status: String,
    summary: Option<String>,
    stats_json: Option<String>,
    proposed_actions_json: Option<String>,
    created_at: String,
}

async fn reviews_pending(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<ProjectIdQuery>,
) -> Result<Response, ApiError> {
    let project_id = require_project(&state, &principal, q.project_id).await?;

    let rows: Vec<ReviewRow> = sqlx::query_as(&db::sql(
        "SELECT id, status, summary, stats_json, proposed_actions_json, \
         CAST(created_at AS TEXT) AS created_at \
         FROM assistant_reviews WHERE project_id = ? AND status = 'pending' \
         ORDER BY created_at DESC",
        state.backend,
    ))
    .bind(project_id)
    .fetch_all(&state.any)
    .await?;

    // No `response_model` on this route in Python — it returns the plain dict
    // `{"reviews": [...]}`, so `created_at` is `.isoformat()` verbatim rather
    // than the pydantic-validated shape the other routes carry.
    let reviews: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "id": r.id,
                "status": r.status,
                "summary": r.summary,
                "stats": json_object(r.stats_json),
                "proposed_actions": json_array(r.proposed_actions_json),
                "created_at": iso_from_sql(&r.created_at),
            })
        })
        .collect();

    Ok(Json(json!({ "reviews": reviews })).into_response())
}

async fn require_review(
    state: &AppState,
    principal: &Principal,
    review_id: i64,
) -> Result<i64, ApiError> {
    let project_id: Option<i64> =
        sqlx::query_scalar("SELECT project_id FROM assistant_reviews WHERE id = ?")
            .bind(review_id)
            .fetch_optional(&state.any)
            .await?;
    let Some(project_id) = project_id else {
        return Err(ApiError::not_found("Review not found"));
    };
    crate::projects::assert_access(state, principal, project_id).await?;
    Ok(review_id)
}

// ---------------------------------------------------------------------------
// Applying board-level agent actions — `board_action_apply.py`
// ---------------------------------------------------------------------------

#[derive(Default)]
struct BoardApplyResult {
    applied: Vec<String>,
    skipped: Vec<String>,
    guidance: Vec<String>,
    created_items: Vec<ItemOut>,
    updated_items: Vec<ItemOut>,
}

fn as_str_param(p: &Map<String, Value>, key: &str) -> Option<String> {
    p.get(key).and_then(Value::as_str).map(str::to_string)
}

fn as_int_param(p: &Map<String, Value>, key: &str) -> Option<i64> {
    p.get(key).and_then(Value::as_i64)
}

/// Same parse/drop-the-offset rule as `todos.rs`'s `as_datetime` — `09:00Z`
/// lands as `09:00` in the naive column.
fn as_datetime_param(p: &Map<String, Value>, key: &str) -> Option<String> {
    let raw = as_str_param(p, key)?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    Some(crate::wire::sql_string(parse_naive(raw)?))
}

/// The item-scoped half of an applied action: `move_item_status`,
/// `update_item`, `schedule_item`, `set_due_date`, `adjust_plan`,
/// `log_completion`, `add_subtask`, `break_down_task`. `board_action_apply.py`
/// routes ten action ids through `_apply_item_action`, but only these eight
/// are handled there — `present_planning_form` never reaches this function
/// (an earlier, board-level `elif` already claims it), and
/// `export_markdown_checklist`/`export_ics_event` fall through to `None`
/// (unsupported at board scope, unlike the per-item `agent/apply` route),
/// which is why both read as "no change" below rather than doing anything.
async fn apply_item_action(
    state: &AppState,
    item: &ItemRow,
    aid: &str,
    p: &Map<String, Value>,
) -> Result<Option<ItemOut>, ApiError> {
    let mut patch = crate::todos::ItemPatch::default();
    match aid {
        "move_item_status" => {
            let Some(status) = as_str_param(p, "status").filter(|s| crate::todos::TODO_STATUSES.contains(&s.as_str())) else {
                return Ok(None);
            };
            patch.set_text("status", status);
        }
        "update_item" => {
            let mut changed = false;
            if let Some(title) = as_str_param(p, "title") {
                patch.set_text("title", title);
                changed = true;
            }
            if let Some(desc) = p.get("description").and_then(Value::as_str) {
                patch.set_text("description", desc.to_string());
                changed = true;
            }
            if let Some(priority) = as_int_param(p, "priority") {
                patch.set_int("priority", priority);
                changed = true;
            }
            if !changed {
                return Ok(None);
            }
        }
        "schedule_item" => {
            let Some(scheduled) = as_datetime_param(p, "scheduled_at") else {
                return Ok(None);
            };
            patch.set_text("scheduled_at", scheduled);
            if let Some(horizon) = as_str_param(p, "time_horizon") {
                patch.set_text("time_horizon", horizon);
            }
        }
        "set_due_date" => {
            let Some(due) = as_datetime_param(p, "due_at") else {
                return Ok(None);
            };
            patch.set_text("due_at", due);
        }
        "adjust_plan" => {
            if let Some(title) = as_str_param(p, "title") {
                patch.set_text("title", title);
            }
            if let Some(desc) = p.get("description").and_then(Value::as_str) {
                patch.set_text("description", desc.to_string());
            }
            if let Some(due) = as_datetime_param(p, "due_at") {
                patch.set_text("due_at", due);
            }
            if let Some(scheduled) = as_datetime_param(p, "scheduled_at") {
                patch.set_text("scheduled_at", scheduled);
            }
            if let Some(horizon) = as_str_param(p, "time_horizon") {
                patch.set_text("time_horizon", horizon);
            }
            if let Some(status) = as_str_param(p, "status") {
                patch.set_text("status", status);
            }
            if let Some(priority) = as_int_param(p, "priority") {
                patch.set_int("priority", priority);
            }
        }
        "log_completion" => {
            let mut completion = json_object(item.completion_json.clone());
            completion.insert("completed_at".into(), json!(crate::todos::now_isoformat()));
            if let Some(minutes) = as_int_param(p, "time_spent_minutes") {
                completion.insert("time_spent_minutes".into(), json!(minutes));
            }
            for key in ["difficulty", "notes", "blockers"] {
                if let Some(value) = as_str_param(p, key) {
                    completion.insert(key.into(), json!(value));
                }
            }
            patch.set_json("completion_json", &Value::Object(completion));
            patch.set_text("status", "done");
        }
        "add_subtask" => {
            let Some(step) = as_str_param(p, "step") else {
                return Ok(None);
            };
            let mut plan = json_array(item.plan_json.clone());
            plan.push(json!({ "step": step, "done": p.get("done").is_some_and(py_truthy) }));
            patch.set_json("plan_json", &Value::Array(plan));
        }
        "break_down_task" => {
            let Some(steps_raw) = p.get("steps").and_then(Value::as_array) else {
                return Ok(None);
            };
            let plan: Vec<Value> = steps_raw
                .iter()
                .map(|s| match s.as_object() {
                    Some(obj) => {
                        let step = obj
                            .get("step")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| s.to_string());
                        json!({ "step": step, "done": obj.get("done").is_some_and(py_truthy) })
                    }
                    None => json!({ "step": crate::todos::python_str(s), "done": false }),
                })
                .collect();
            patch.set_json("plan_json", &Value::Array(plan));
        }
        _ => return Ok(None),
    }
    patch.write(state, item.id).await?;
    Ok(Some(crate::todos::load_item(state, item.id).await?))
}

/// `apply_board_actions`. Every action is independent: one that cannot be
/// applied is recorded in `skipped` with the reason and the rest still run.
async fn apply_board_actions(
    state: &AppState,
    board_id: i64,
    actions: &[PlannedAction],
) -> Result<BoardApplyResult, ApiError> {
    let mut result = BoardApplyResult::default();

    for action in actions {
        let p = &action.parameters;
        let aid = action.action_id.as_str();

        match aid {
            "create_item" => {
                let Some(title) = as_str_param(p, "title").filter(|t| !t.is_empty()) else {
                    result.skipped.push("create_item: missing title".into());
                    continue;
                };
                let status = as_str_param(p, "status")
                    .filter(|s| crate::todos::TODO_STATUSES.contains(&s.as_str()))
                    .unwrap_or_else(|| "plan".into());
                let item_kind = as_str_param(p, "item_kind").unwrap_or_else(|| "task".into());
                let now = sql_now();
                let id: i64 = sqlx::query_scalar(&db::sql(
                    "INSERT INTO todo_items (board_id, category_id, title, description, status, \
                     priority, parent_item_id, due_at, scheduled_at, time_horizon, item_kind, \
                     created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
                     RETURNING CAST(id AS BIGINT)",
                    state.backend,
                ))
                .bind(board_id)
                .bind(as_int_param(p, "category_id"))
                .bind(&title)
                .bind(as_str_param(p, "description").unwrap_or_default())
                .bind(&status)
                .bind(as_int_param(p, "priority").unwrap_or(0))
                .bind(as_int_param(p, "parent_item_id"))
                .bind(as_datetime_param(p, "due_at"))
                .bind(as_datetime_param(p, "scheduled_at"))
                .bind(as_str_param(p, "time_horizon"))
                .bind(&item_kind)
                .bind(&now)
                .bind(&now)
                .fetch_one(&state.any)
                .await?;
                result.created_items.push(crate::todos::load_item(state, id).await?);
                result.applied.push(format!("Created: {title}"));
            }

            "create_habit" => {
                let Some(title) = as_str_param(p, "title").filter(|t| !t.is_empty()) else {
                    result.skipped.push("create_habit: missing title".into());
                    continue;
                };
                let recurrence = p
                    .get("recurrence")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_else(|| {
                        let mut m = Map::new();
                        m.insert("cadence".into(), json!("daily"));
                        m
                    });
                let time_horizon = as_str_param(p, "time_horizon").unwrap_or_else(|| "day".into());
                let now = sql_now();
                let id: i64 = sqlx::query_scalar(&db::sql(
                    "INSERT INTO todo_items (board_id, title, description, status, item_kind, \
                     time_horizon, recurrence_json, created_at, updated_at) \
                     VALUES (?, ?, ?, 'backlog', 'habit', ?, ?, ?, ?) RETURNING CAST(id AS BIGINT)",
                    state.backend,
                ))
                .bind(board_id)
                .bind(&title)
                .bind(as_str_param(p, "description").unwrap_or_default())
                .bind(&time_horizon)
                .bind(Value::Object(recurrence).to_string())
                .bind(&now)
                .bind(&now)
                .fetch_one(&state.any)
                .await?;
                result.created_items.push(crate::todos::load_item(state, id).await?);
                result.applied.push(format!("Created habit: {title}"));
            }

            "create_subtask_item" => {
                let parent_id = as_int_param(p, "parent_item_id");
                let title = as_str_param(p, "title").filter(|t| !t.is_empty());
                let (Some(parent_id), Some(title)) = (parent_id, title) else {
                    result.skipped.push("create_subtask_item: missing parent or title".into());
                    continue;
                };
                let parent: Option<ItemRow> = sqlx::query_as(&format!(
                    "SELECT {ITEM_COLUMNS} FROM todo_items WHERE id = ?"
                ))
                .bind(parent_id)
                .fetch_optional(&state.pool)
                .await?;
                let Some(parent) = parent.filter(|it| it.board_id == board_id) else {
                    result.skipped.push("create_subtask_item: invalid parent".into());
                    continue;
                };
                let now = sql_now();
                let id: i64 = sqlx::query_scalar(&db::sql(
                    "INSERT INTO todo_items (board_id, category_id, title, description, status, \
                     priority, parent_item_id, due_at, scheduled_at, time_horizon, item_kind, \
                     created_at, updated_at) VALUES (?, ?, ?, ?, 'plan', 0, ?, ?, ?, ?, 'task', ?, ?) \
                     RETURNING CAST(id AS BIGINT)",
                    state.backend,
                ))
                .bind(board_id)
                .bind(parent.category_id)
                .bind(&title)
                .bind(as_str_param(p, "description").unwrap_or_default())
                .bind(parent_id)
                .bind(as_datetime_param(p, "due_at"))
                .bind(as_datetime_param(p, "scheduled_at"))
                .bind(parent.time_horizon.filter(|h| !h.is_empty()).unwrap_or_else(|| "week".into()))
                .bind(&now)
                .bind(&now)
                .fetch_one(&state.any)
                .await?;
                result.created_items.push(crate::todos::load_item(state, id).await?);
                result.applied.push(format!("Created subtask: {title}"));
            }

            "propose_review" => {
                let reason = as_str_param(p, "reason")
                    .filter(|r| !r.is_empty())
                    .unwrap_or_else(|| "Progress review suggested".into());
                result.guidance.push(reason);
                if let Some(focus) = p.get("focus_areas").and_then(Value::as_array) {
                    result.guidance.extend(
                        focus.iter().map(|x| crate::todos::python_str(x).as_str().unwrap_or_default().to_string()),
                    );
                }
                result.applied.push("Review proposed".into());
            }

            "ask_clarifying_questions" => {
                if let Some(qs) = p.get("questions").and_then(Value::as_array) {
                    for q in qs {
                        if q.is_null() {
                            continue;
                        }
                        let text = crate::todos::python_str(q).as_str().unwrap_or_default().to_string();
                        if !text.trim().is_empty() {
                            result.guidance.push(text);
                        }
                    }
                }
                result.applied.push("Questions noted".into());
            }

            "suggest_next_steps" => {
                if let Some(guidance) = as_str_param(p, "guidance").filter(|g| !g.is_empty()) {
                    result.guidance.push(guidance);
                }
                if let Some(steps) = p.get("steps").and_then(Value::as_array) {
                    for s in steps {
                        if s.is_null() {
                            continue;
                        }
                        let text = crate::todos::python_str(s).as_str().unwrap_or_default().to_string();
                        if !text.trim().is_empty() {
                            result.guidance.push(text);
                        }
                    }
                }
                result.applied.push("Guidance noted".into());
            }

            "present_planning_form" => {
                let Some(form) = p.get("form").and_then(Value::as_object) else {
                    result.skipped.push("present_planning_form: invalid form".into());
                    continue;
                };
                result.applied.push("Planning form ready for user".into());
                let title = form.get("title").and_then(Value::as_str).unwrap_or("Details needed");
                result.guidance.push(format!("Form: {title}"));
            }

            "store_user_profile" => {
                let domain = as_str_param(p, "domain").filter(|d| !d.is_empty());
                let data = p.get("data").and_then(Value::as_object);
                let (Some(domain), Some(data)) = (domain, data) else {
                    result.skipped.push("store_user_profile: invalid domain or data".into());
                    continue;
                };
                let project_id: Option<Option<i64>> = sqlx::query_scalar(
                    "SELECT project_id FROM todo_boards WHERE id = ?",
                )
                .bind(board_id)
                .fetch_optional(&state.pool)
                .await?;
                let Some(Some(project_id)) = project_id else {
                    result.skipped.push("store_user_profile: board not project-scoped".into());
                    continue;
                };
                crate::todos::merge_domain_profile(state, project_id, &domain, data).await?;
                result.applied.push(format!("Saved {domain} profile"));
            }

            "trigger_webhook" => {
                let Some(url) = as_str_param(p, "webhook_url").filter(|u| !u.is_empty()) else {
                    result.skipped.push("trigger_webhook: missing webhook_url".into());
                    continue;
                };
                match crate::todos::trigger_webhook(&state.http, &url, p.get("payload")).await {
                    Ok((status, ok)) => result
                        .applied
                        .push(format!("Webhook {status}{}", if ok { " OK" } else { " failed" })),
                    Err(message) => result.skipped.push(format!("trigger_webhook: {message}")),
                }
            }

            "move_item_status" | "update_item" | "add_subtask" | "break_down_task"
            | "schedule_item" | "set_due_date" | "log_completion" | "adjust_plan"
            | "export_markdown_checklist" | "export_ics_event" => {
                let Some(item_id) = as_int_param(p, "item_id") else {
                    result.skipped.push(format!("{aid}: missing item_id"));
                    continue;
                };
                let row: Option<ItemRow> = sqlx::query_as(&format!(
                    "SELECT {ITEM_COLUMNS} FROM todo_items WHERE id = ?"
                ))
                .bind(item_id)
                .fetch_optional(&state.pool)
                .await?;
                let Some(item) = row.filter(|it| it.board_id == board_id) else {
                    result.skipped.push(format!("{aid}: item not on board"));
                    continue;
                };
                match apply_item_action(state, &item, aid, p).await? {
                    Some(updated) => {
                        result.updated_items.push(updated);
                        result.applied.push(format!("{aid} on item {item_id}"));
                    }
                    None => result.skipped.push(format!("{aid}: no change")),
                }
            }

            other => result.skipped.push(format!("Unknown action: {other}")),
        }
    }

    Ok(result)
}

fn format_apply_summary(result: &BoardApplyResult) -> String {
    let mut parts = Vec::new();
    if !result.applied.is_empty() {
        parts.push(format!("Applied: {}.", result.applied.join("; ")));
    }
    if !result.skipped.is_empty() {
        parts.push(format!("Skipped: {}.", result.skipped.join("; ")));
    }
    if !result.guidance.is_empty() {
        parts.push(result.guidance.join(" "));
    }
    if parts.is_empty() {
        "Done.".into()
    } else {
        parts.join(" ")
    }
}

// ---------------------------------------------------------------------------
// POST /assistant/chat/apply
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ApplyActionsRequest {
    #[serde(default)]
    actions: Vec<PlannedAction>,
    #[serde(default)]
    thread_id: Option<i64>,
    #[serde(default = "default_true")]
    auto_continue: bool,
    #[serde(default)]
    model: Option<String>,
}

async fn chat_apply(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<ProjectIdQuery>,
    Json(body): Json<ApplyActionsRequest>,
) -> Result<Response, ApiError> {
    let project_id = require_project(&state, &principal, q.project_id).await?;
    let board_id = ensure_assistant_board(&state, project_id).await?;

    // Apply task actions only — forms are handled via `chat/submit-form`.
    let task_actions: Vec<PlannedAction> =
        body.actions.iter().filter(|a| a.action_id != "present_planning_form").cloned().collect();
    let result = apply_board_actions(&state, board_id, &task_actions).await?;

    let mut thread = resolve_thread(&state, project_id, body.thread_id).await?;
    let mut messages = thread.messages();
    let status = if task_actions.is_empty() { "dismissed" } else { "approved" };
    crate::assistant_turn::resolve_pending_proposal_in_messages(&mut messages, status);
    thread.messages_json =
        if messages.is_empty() { None } else { Some(serde_json::to_string(&messages).expect("messages serialize")) };
    thread.pending_actions_json = None;
    thread.updated_at = sql_now();
    persist_thread(&state, &thread).await?;

    // The actions above are already applied and committed: a continuation
    // failure (LLM down, timeout) must not turn the whole apply into an
    // error, or the client would re-apply and duplicate the board changes.
    let mut continuation: Option<Value> = None;
    if body.auto_continue && (!result.applied.is_empty() || !result.skipped.is_empty()) {
        let summary = format_apply_summary(&result);
        if let Ok(data) = send_chat_message(
            &state,
            project_id,
            &summary,
            Some(thread.id),
            body.model.as_deref(),
            thread.last_profile_slug.as_deref(),
            true,
        )
        .await
        {
            continuation = Some(data);
        }
    }

    Ok(Json(json!({
        "applied": result.applied,
        "skipped": result.skipped,
        "created_items": result.created_items,
        "updated_items": result.updated_items,
        "guidance": result.guidance,
        "continuation": continuation,
    }))
    .into_response())
}

// ---------------------------------------------------------------------------
// POST /assistant/items/{item_id}/complete
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct CompleteItemRequest {
    #[serde(default)]
    time_spent_minutes: Option<i64>,
    #[serde(default)]
    difficulty: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    blockers: Option<String>,
}

async fn complete_item(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Path(item_id): Path<i64>,
    body: Option<Json<CompleteItemRequest>>,
) -> Result<Response, ApiError> {
    crate::todos::assert_item_access(&state, &principal, item_id).await?;
    let req = body.map(|Json(b)| b).unwrap_or_default();

    let row: Option<ItemRow> =
        sqlx::query_as(&format!("SELECT {ITEM_COLUMNS} FROM todo_items WHERE id = ?"))
            .bind(item_id)
            .fetch_optional(&state.pool)
            .await?;
    let Some(item) = row else {
        return Err(ApiError::not_found("Item not found"));
    };

    let mut completion = json_object(item.completion_json.clone());
    completion.insert("completed_at".into(), json!(crate::todos::now_isoformat()));
    if let Some(minutes) = req.time_spent_minutes {
        completion.insert("time_spent_minutes".into(), json!(minutes));
    }
    if let Some(difficulty) = req.difficulty.filter(|s| !s.is_empty()) {
        completion.insert("difficulty".into(), json!(difficulty));
    }
    if let Some(notes) = req.notes.filter(|s| !s.is_empty()) {
        completion.insert("notes".into(), json!(notes));
    }
    if let Some(blockers) = req.blockers.filter(|s| !s.is_empty()) {
        completion.insert("blockers".into(), json!(blockers));
    }

    let mut patch = crate::todos::ItemPatch::default();
    patch.set_json("completion_json", &Value::Object(completion));
    patch.set_text("status", "done");
    patch.write(&state, item_id).await?;

    Ok(Json(crate::todos::load_item(&state, item_id).await?).into_response())
}

// ---------------------------------------------------------------------------
// POST /assistant/reviews/run, /reviews/{id}/apply, /reviews/{id}/dismiss —
// `review_service.py`
// ---------------------------------------------------------------------------

/// `todos.seeds.REVIEWER_ROLE_PROMPT`, byte-identical.
const REVIEWER_PROMPT: &str = "You are ProgressReviewer on a Personal Assistant planning team. \
Analyze completion stats, overdue items, habit consistency, and reported challenges. \
Propose concrete plan adjustments: reschedule overdue tasks, break down stuck items, adjust habits, \
or suggest focus areas. The user executes — you review direction and keep them on track. \
Be honest but supportive. Prioritize sustainable progress over perfection. \
Prefer propose_review, adjust_plan, log_completion, and break_down_task. \
Use only todo-board-ops actions. Prefer ask_clarifying_questions when scope is unclear \
(always include a questions array with 2–4 specific strings); \
present_planning_form when profile_gaps for your domain is non-empty (see domain_form_templates); \
create_item, schedule_item, and set_due_date for concrete outcomes; \
break_down_task for step lists; export_ics_event when the user wants calendar blocks.";

/// `_compute_stats`. Field order is the dict literal's order, and matters —
/// this crate's `Map` preserves insertion order.
fn compute_review_stats(items: &[ItemRow]) -> Map<String, Value> {
    let now = Utc::now().naive_utc();
    let done: Vec<&ItemRow> = items.iter().filter(|i| i.status == "done").collect();
    let overdue: Vec<&ItemRow> = items
        .iter()
        .filter(|i| {
            i.status != "done" && i.due_at.as_deref().and_then(parse_naive).is_some_and(|d| d < now)
        })
        .collect();
    let habits: Vec<&ItemRow> = items.iter().filter(|i| i.item_kind.as_deref() == Some("habit")).collect();
    let habits_done = habits.iter().filter(|i| i.status == "done").count();

    let mut difficulties = Vec::new();
    let mut time_spent = Vec::new();
    for i in &done {
        let completion = json_object(i.completion_json.clone());
        if let Some(v) = completion.get("difficulty").filter(|v| py_truthy(v)) {
            difficulties.push(v.clone());
        }
        if let Some(v) = completion.get("time_spent_minutes").filter(|v| py_truthy(v)).and_then(Value::as_f64) {
            time_spent.push(v);
        }
    }

    let mut stats = Map::new();
    stats.insert("total_items".into(), json!(items.len()));
    stats.insert("done_count".into(), json!(done.len()));
    stats.insert("active_count".into(), json!(items.len() - done.len()));
    stats.insert("overdue_count".into(), json!(overdue.len()));
    stats.insert(
        "completion_rate".into(),
        if items.is_empty() {
            json!(0)
        } else {
            json!(((done.len() as f64 / items.len() as f64) * 100.0).round() / 100.0)
        },
    );
    stats.insert("habits_total".into(), json!(habits.len()));
    stats.insert("habits_done".into(), json!(habits_done));
    stats.insert(
        "overdue_titles".into(),
        Value::Array(overdue.iter().take(10).map(|i| json!(i.title)).collect()),
    );
    stats.insert("difficulty_breakdown".into(), Value::Array(difficulties));
    stats.insert(
        "avg_time_spent_minutes".into(),
        if time_spent.is_empty() {
            Value::Null
        } else {
            json!((time_spent.iter().sum::<f64>() / time_spent.len() as f64 * 10.0).round() / 10.0)
        },
    );
    stats
}

#[derive(Deserialize, Default)]
struct ReviewRunRequest {
    #[serde(default)]
    model: Option<String>,
}

async fn reviews_run(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<ProjectIdQuery>,
    body: Option<Json<ReviewRunRequest>>,
) -> Result<Response, ApiError> {
    let project_id = require_project(&state, &principal, q.project_id).await?;
    let board_id = ensure_assistant_board(&state, project_id).await?;

    let items: Vec<ItemRow> = sqlx::query_as(&format!(
        "SELECT {ITEM_COLUMNS} FROM todo_items WHERE board_id = ?"
    ))
    .bind(board_id)
    .fetch_all(&state.pool)
    .await?;
    let stats = compute_review_stats(&items);

    let action_set_id: Option<i64> =
        sqlx::query_scalar("SELECT id FROM action_sets WHERE name = 'todo-board-ops'")
            .fetch_optional(&state.any)
            .await?;
    let Some(action_set_id) = action_set_id else {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Action set not configured"));
    };
    let actions = list_actions(&state, action_set_id).await?;

    let profiles = all_profiles(&state, project_id).await?;
    let items_summary: Vec<Value> = items
        .iter()
        .take(30)
        .map(|i| {
            json!({
                "id": i.id,
                "title": i.title,
                "status": i.status,
                "due_at": i.due_at.as_deref().map(iso_from_sql),
                "item_kind": i.item_kind,
                "completion": json_object(i.completion_json.clone()),
            })
        })
        .collect();

    let mut context = Map::new();
    context.insert("reviewer_prompt".into(), json!(REVIEWER_PROMPT));
    context.insert("stats".into(), Value::Object(stats.clone()));
    context.insert("board_id".into(), json!(board_id));
    context.insert("user_domain_profiles".into(), Value::Object(profiles));
    context.insert("items_summary".into(), Value::Array(items_summary));

    let requested_model = body.and_then(|Json(b)| b.model).filter(|m| !m.is_empty());
    let board_default_model: Option<String> =
        sqlx::query_scalar("SELECT default_model FROM todo_boards WHERE id = ?")
            .bind(board_id)
            .fetch_one(&state.pool)
            .await?;
    let llm_model = requested_model
        .or_else(|| board_default_model.filter(|m| !m.is_empty()))
        .unwrap_or_else(|| "gemma4:31b-cloud".to_string());

    let goal = "Review the user's progress and propose plan adjustments. Focus on overdue items, \
                habit consistency, and sustainable next steps.";
    let (planned, thought, _usage) = decide_actions(&state, goal, &context, &actions, &llm_model).await;

    let now = sql_now();
    let summary = thought.unwrap_or_else(|| "Progress review complete.".to_string());
    let stats_json = Value::Object(stats.clone()).to_string();
    let proposed_json = serde_json::to_string(&planned).expect("PlannedAction serializes");

    let review_id: i64 = sqlx::query_scalar(&db::sql(
        "INSERT INTO assistant_reviews (project_id, status, summary, stats_json, \
         proposed_actions_json, created_at, updated_at) VALUES (?, 'pending', ?, ?, ?, ?, ?) \
         RETURNING CAST(id AS BIGINT)",
        state.backend,
    ))
    .bind(project_id)
    .bind(&summary)
    .bind(&stats_json)
    .bind(&proposed_json)
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.any)
    .await?;

    Ok(Json(json!({
        "review_id": review_id,
        "status": "pending",
        "summary": summary,
        "stats": stats,
        "proposed_actions": planned,
    }))
    .into_response())
}

#[derive(Deserialize, Default)]
struct ReviewApplyRequest {
    #[serde(default)]
    actions: Option<Vec<PlannedAction>>,
}

async fn reviews_apply(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Path(review_id): Path<i64>,
    body: Option<Json<ReviewApplyRequest>>,
) -> Result<Response, ApiError> {
    let review_id = require_review(&state, &principal, review_id).await?;

    #[derive(FromRow)]
    struct ReviewFull {
        project_id: i64,
        status: String,
        proposed_actions_json: Option<String>,
    }
    let review: ReviewFull = sqlx::query_as(
        "SELECT project_id, status, proposed_actions_json FROM assistant_reviews WHERE id = ?",
    )
    .bind(review_id)
    .fetch_one(&state.any)
    .await?;

    if review.status == "applied" {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Review already applied"));
    }
    if review.status == "dismissed" {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Review was dismissed"));
    }

    let board_id = ensure_assistant_board(&state, review.project_id).await?;

    let requested = body.and_then(|Json(b)| b.actions).filter(|a| !a.is_empty());
    let to_apply: Vec<PlannedAction> = match requested {
        Some(actions) => actions,
        None => json_array(review.proposed_actions_json)
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect(),
    };

    let result = apply_board_actions(&state, board_id, &to_apply).await?;

    let now = sql_now();
    sqlx::query("UPDATE assistant_reviews SET status = 'applied', updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(review_id)
        .execute(&state.any)
        .await?;

    Ok(Json(json!({
        "review_id": review_id,
        "status": "applied",
        "applied": result.applied,
        "skipped": result.skipped,
        "created_items": result.created_items,
        "updated_items": result.updated_items,
    }))
    .into_response())
}

async fn reviews_dismiss(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Path(review_id): Path<i64>,
) -> Result<Response, ApiError> {
    let review_id = require_review(&state, &principal, review_id).await?;
    let status: String = sqlx::query_scalar("SELECT status FROM assistant_reviews WHERE id = ?")
        .bind(review_id)
        .fetch_one(&state.any)
        .await?;
    if status == "applied" {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Review already applied"));
    }
    if status == "dismissed" {
        return Ok(Json(json!({ "review_id": review_id, "status": status })).into_response());
    }
    let now = sql_now();
    sqlx::query("UPDATE assistant_reviews SET status = 'dismissed', updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(review_id)
        .execute(&state.any)
        .await?;
    Ok(Json(json!({ "review_id": review_id, "status": "dismissed" })).into_response())
}

// ---------------------------------------------------------------------------
// POST /assistant/chat/retry
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ChatRetryRequest {
    thread_id: i64,
    message_index: i64,
    #[serde(default)]
    model: Option<String>,
    #[serde(default = "default_true")]
    propose_actions: bool,
}

/// `retry_chat_message`: drop messages after `message_index` and regenerate.
///
/// Python's `message_index: int = Field(ge=0, ...)` rejects a negative index
/// as a 422 before the handler runs; this validates it as a plain 400
/// instead of reproducing pydantic's envelope, the same known gap as every
/// other field-validation error in this crate.
async fn chat_retry(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<ProjectIdQuery>,
    Json(body): Json<ChatRetryRequest>,
) -> Result<Response, ApiError> {
    let project_id = require_project(&state, &principal, q.project_id).await?;
    let mut thread = get_thread_by_id(&state, project_id, body.thread_id).await?;
    let messages = thread.messages();

    if body.message_index < 0 || body.message_index as usize >= messages.len() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "message_index out of range"));
    }
    let index = body.message_index as usize;
    let target = &messages[index];
    if target.get("role").and_then(Value::as_str) != Some("user") {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "message_index must point to a user message"));
    }
    let message = target.get("content").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if message.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "User message is empty"));
    }

    let mut truncated: Vec<Value> = messages[..=index].to_vec();
    crate::assistant_turn::resolve_pending_proposal_in_messages(&mut truncated, "superseded");
    thread.messages_json =
        if truncated.is_empty() { None } else { Some(serde_json::to_string(&truncated).expect("messages serialize")) };
    thread.pending_actions_json = None;
    thread.updated_at = sql_now();
    persist_thread(&state, &thread).await?;

    let delegate_slug = thread.last_profile_slug.clone();
    let result = generate_assistant_turn(
        &state,
        project_id,
        &mut thread,
        &message,
        body.model.as_deref(),
        delegate_slug.as_deref(),
        body.propose_actions,
    )
    .await?;
    Ok(Json(result).into_response())
}

// ---------------------------------------------------------------------------
// POST /assistant/chat/submit-form
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct FormSubmitRequest {
    domain: String,
    #[serde(default)]
    answers: Map<String, Value>,
    #[serde(default)]
    thread_id: Option<i64>,
    #[serde(default = "default_true")]
    auto_continue: bool,
    #[serde(default)]
    model: Option<String>,
}

/// `_resolve_form_submit_domain`: prefer an explicit domain, else a pending
/// `present_planning_form` action's own domain, else the routed profile's
/// domain.
fn resolve_form_submit_domain(domain: &str, pending: &[PlannedAction], profile_slug: Option<&str>) -> String {
    let d = domain.trim().to_lowercase();
    if !d.is_empty() && d != "general" {
        return d;
    }
    for a in pending {
        if a.action_id != "present_planning_form" {
            continue;
        }
        if let Some(action_domain) = a.parameters.get("domain").and_then(Value::as_str) {
            let ad = action_domain.trim();
            if !ad.is_empty() {
                return ad.to_lowercase();
            }
        }
        if let Some(form_domain) =
            a.parameters.get("form").and_then(Value::as_object).and_then(|f| f.get("domain")).and_then(Value::as_str)
        {
            let fd = form_domain.trim();
            if !fd.is_empty() {
                return fd.to_lowercase();
            }
        }
    }
    if let Some(slug) = profile_slug.filter(|s| !s.is_empty()) {
        return domain_for_profile_slug(slug).to_string();
    }
    if d.is_empty() {
        "general".to_string()
    } else {
        d
    }
}

/// `user_profile_service.format_answers_message`.
fn format_answers_message(domain: &str, answers: &Map<String, Value>) -> String {
    let mut lines = vec![format!("Saved {domain} profile:")];
    for (k, v) in answers {
        let val = match v {
            Value::Array(items) => items
                .iter()
                .map(|x| crate::todos::python_str(x).as_str().unwrap_or_default().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            other => crate::todos::python_str(other).as_str().unwrap_or_default().to_string(),
        };
        lines.push(format!("- {}: {val}", k.replace('_', " ")));
    }
    lines.push("Please continue planning using this information.".to_string());
    lines.join("\n")
}

/// `user_profile_service.get_profile`.
async fn get_profile(state: &AppState, project_id: i64, domain: &str) -> Result<Map<String, Value>, ApiError> {
    let row: Option<ProfileJsonOnly> = sqlx::query_as(&db::sql(
        "SELECT profile_json FROM assistant_domain_profiles WHERE project_id = ? AND domain = ?",
        state.backend,
    ))
    .bind(project_id)
    .bind(domain)
    .fetch_optional(&state.any)
    .await?;
    Ok(row.map(|r| json_object(r.profile_json)).unwrap_or_default())
}

/// `submit_planning_form`: either records answers to an in-thread clarifying
/// Q&A, or saves a domain profile form — the two share nothing but the
/// request shape, matching Python's own branch split.
async fn chat_submit_form(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<ProjectIdQuery>,
    Json(body): Json<FormSubmitRequest>,
) -> Result<Response, ApiError> {
    let project_id = require_project(&state, &principal, q.project_id).await?;
    let mut thread = resolve_thread(&state, project_id, body.thread_id).await?;
    let pending_raw = thread.pending_actions();
    let pending: Vec<PlannedAction> =
        pending_raw.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect();
    let pending_form = crate::assistant_turn::extract_pending_form(&pending);

    if let Some(form) = pending_form.filter(|f| {
        crate::clarifying_form::is_clarifying_form(Some(&Value::Object(f.clone())))
    }) {
        let form_value = Value::Object(form);
        let remaining: Vec<PlannedAction> =
            pending.into_iter().filter(|a| a.action_id != "ask_clarifying_questions").collect();
        thread.pending_actions_json =
            if remaining.is_empty() { None } else { Some(serde_json::to_string(&remaining).expect("serializes")) };
        let summary = crate::clarifying_form::format_clarifying_answers_message(&form_value, &body.answers);
        let mut messages = thread.messages();
        messages.push(json!({ "role": "user", "content": summary }));
        thread.messages_json =
            if messages.is_empty() { None } else { Some(serde_json::to_string(&messages).expect("serializes")) };
        thread.updated_at = sql_now();
        persist_thread(&state, &thread).await?;

        if !body.auto_continue {
            let pending_for_form: Vec<PlannedAction> =
                thread.pending_actions().iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect();
            let pending_form = crate::assistant_turn::extract_pending_form(&pending_for_form).map(Value::Object);
            return Ok(Json(json!({
                "thread_id": thread.id,
                "messages": thread.messages(),
                "pending_actions": thread.pending_actions(),
                "pending_form": pending_form,
            }))
            .into_response());
        }

        let data = send_chat_message(
            &state,
            project_id,
            &summary,
            Some(thread.id),
            body.model.as_deref(),
            thread.last_profile_slug.as_deref(),
            true,
        )
        .await?;
        return Ok(Json(data).into_response());
    }

    let domain = resolve_form_submit_domain(&body.domain, &pending, thread.last_profile_slug.as_deref());
    crate::todos::merge_domain_profile(&state, project_id, &domain, &body.answers).await?;
    let remaining = crate::assistant_turn::actions_without_forms(&pending);
    thread.pending_actions_json =
        if remaining.is_empty() { None } else { Some(serde_json::to_string(&remaining).expect("serializes")) };

    let summary = format_answers_message(&domain, &body.answers);

    if !body.auto_continue {
        let mut messages = thread.messages();
        messages.push(json!({ "role": "user", "content": summary }));
        thread.messages_json =
            if messages.is_empty() { None } else { Some(serde_json::to_string(&messages).expect("serializes")) };
        thread.updated_at = sql_now();
        persist_thread(&state, &thread).await?;
        let profile = get_profile(&state, project_id, &domain).await?;
        return Ok(Json(json!({
            "thread_id": thread.id,
            "saved_domain": domain,
            "profile": profile,
            "messages": thread.messages(),
            "pending_actions": thread.pending_actions(),
        }))
        .into_response());
    }

    persist_thread(&state, &thread).await?;
    let data = send_chat_message(
        &state,
        project_id,
        &summary,
        Some(thread.id),
        body.model.as_deref(),
        thread.last_profile_slug.as_deref(),
        true,
    )
    .await?;
    Ok(Json(data).into_response())
}

// ---------------------------------------------------------------------------
// POST /assistant/reset — `assistant_reset.py`
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct AssistantResetRequest {
    #[serde(default)]
    confirm: bool,
}

/// `_purge_todo_board`, as four statements instead of a row-by-row cascade.
///
/// **Order is the whole point of this function.** SQLite runs with
/// `PRAGMA foreign_keys = OFF` on both servers (see `db.rs`), so nothing here
/// is enforced today — but the schema declares these keys, Postgres will
/// enforce them, and this is the one route where getting the order wrong
/// corrupts rather than errors. Dependents first: events (which are *not*
/// cascaded on item delete), then items, then categories, then the board.
///
/// Items go in two passes because `todo_items.parent_item_id` is
/// self-referential — subtasks before top-level items. That is Python's
/// `sorted(items, key=lambda i: (i.parent_item_id is None, i.id or 0))`
/// reproduced as a `WHERE`: it splits on *has a parent* and nothing finer, so
/// a grandchild is in the same pass as its parent. Matching that rather than
/// topologically sorting keeps the two servers identical; it is latent in
/// Python the same way.
async fn purge_todo_board(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    board_id: i64,
) -> Result<(), ApiError> {
    sqlx::query(
        "DELETE FROM todo_item_events WHERE item_id IN \
         (SELECT id FROM todo_items WHERE board_id = ?)",
    )
    .bind(board_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM todo_items WHERE board_id = ? AND parent_item_id IS NOT NULL")
        .bind(board_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM todo_items WHERE board_id = ?")
        .bind(board_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM todo_categories WHERE board_id = ?")
        .bind(board_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM todo_boards WHERE id = ?")
        .bind(board_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// `reset_assistant_workspace`: drop the project's assistant board (items,
/// their events, categories), every assistant chat thread and every review,
/// then hand back a fresh board and thread.
///
/// **Domain profiles survive on purpose** — `assistant_domain_profiles` is not
/// touched here, matching Python's docstring; those are cleared from the
/// profile page instead.
///
/// The deletes run in one transaction, which Python gets from doing the whole
/// thing in a single session. The project-pointer `UPDATE` is *outside* it,
/// because `project` is the one table already converted to the Postgres-aware
/// `state.any` pool and a transaction cannot span both pools while that
/// migration is mid-flight. Python nulls the pointer and flushes before
/// deleting for the same reason it matters here: the FK from
/// `project.assistant_board_id` would otherwise block the board delete.
async fn assistant_reset(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<ProjectIdQuery>,
    body: Option<Json<AssistantResetRequest>>,
) -> Result<Response, ApiError> {
    let project_id = require_project(&state, &principal, q.project_id).await?;
    let req = body.map(|Json(b)| b).unwrap_or_default();
    if !req.confirm {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Confirmation required: send confirm=true to reset the assistant workspace",
        ));
    }

    #[derive(FromRow)]
    struct ProjectPointers {
        assistant_board_id: Option<i64>,
        last_todo_board_id: Option<i64>,
    }
    let pointers: Option<ProjectPointers> = sqlx::query_as(&db::sql(
        "SELECT assistant_board_id, last_todo_board_id FROM project WHERE id = ?",
        state.backend,
    ))
    .bind(project_id)
    .fetch_optional(&state.any)
    .await?;
    let Some(pointers) = pointers else {
        return Err(ApiError::not_found("Project not found"));
    };
    let board_id = pointers.assistant_board_id;

    // `last_todo_board_id` is cleared only when it pointed at the board being
    // deleted; an unrelated board the user was last on survives the reset.
    let clear_last = board_id.is_some() && pointers.last_todo_board_id == board_id;
    let now = sql_now();
    let sql = if clear_last {
        "UPDATE project SET assistant_board_id = NULL, last_todo_board_id = NULL, \
         planning_prefs_json = NULL, updated_at = ? WHERE id = ?"
    } else {
        "UPDATE project SET assistant_board_id = NULL, planning_prefs_json = NULL, \
         updated_at = ? WHERE id = ?"
    };
    sqlx::query(&db::sql(sql, state.backend))
        .bind(&now)
        .bind(project_id)
        .execute(&state.any)
        .await?;

    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM assistant_chat_threads WHERE project_id = ?")
        .bind(project_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM assistant_reviews WHERE project_id = ?")
        .bind(project_id)
        .execute(&mut *tx)
        .await?;
    if let Some(board_id) = board_id {
        purge_todo_board(&mut tx, board_id).await?;
    }
    tx.commit().await?;

    let new_board_id = ensure_assistant_board(&state, project_id).await?;
    let thread = create_thread_row(&state, project_id, Some("New chat")).await?;

    Ok(Json(json!({
        "project_id": project_id,
        "board_id": new_board_id,
        "thread_id": thread.id,
    }))
    .into_response())
}

// ---------------------------------------------------------------------------
// Chat — profile routing, `assistant_router.py`
// ---------------------------------------------------------------------------

const DEFAULT_PROFILE: &str = "personal-assistant";

/// `DOMAIN_KEYWORDS`, in declaration order — order is load-bearing: a tied
/// score picks the *first* slug scanned, matching Python's `max(scores,
/// key=scores.get)` on a dict (first-seen key wins ties).
const DOMAIN_KEYWORDS: &[(&str, &[&str])] = &[
    (
        "code-task-planner",
        &[
            "bug", "feature", "code", "coding", "implement", "refactor", "api", "pull request",
            "github", "typescript", "python", "debug", "deploy", "software", "dev sprint",
            "unit test", "lint",
        ],
    ),
    (
        "research-scout",
        &[
            "research", "read up", "sources", "literature", "survey", "compare options",
            "investigate", "learn about", "notes on",
        ],
    ),
    (
        "sprint-planner",
        &["sprint", "epic", "story points", "backlog grooming", "iteration plan"],
    ),
    (
        "shopping-planner",
        &[
            "shop", "shopping", "grocery", "groceries", "costco", "supermarket", "shopping list",
            "buy list", "aisle", "pantry",
        ],
    ),
    (
        "fitness-coach",
        &[
            "workout", "workouts", "exercise", "exercises", "gym", "fitness", "run", "running",
            "lift", "lifting", "cardio", "recovery", "stretch", "weights",
        ],
    ),
    (
        "finance-planner",
        &[
            "budget", "finance", "money", "savings", "bill", "expense", "invest", "debt",
            "paycheck", "subscription",
        ],
    ),
    (
        "professional-planner",
        &[
            "career", "professional", "promotion", "interview", "resume", "cv", "networking",
            "salary", "performance review",
        ],
    ),
    (
        "travel-planner",
        &[
            "travel", "trip", "flight", "hotel", "vacation", "itinerary", "packing", "booking",
            "passport",
        ],
    ),
    (
        "nutrition-coach",
        &[
            "meal", "meals", "nutrition", "diet", "food", "cook", "recipe", "calorie",
            "breakfast", "lunch", "dinner",
        ],
    ),
    (
        "habit-coach",
        &[
            "habit", "routine", "streak", "daily practice", "morning routine", "evening routine",
            "consistency",
        ],
    ),
    (
        "mentorship-coach",
        &[
            "mentor", "milestone", "growth plan", "reflect", "reflection", "long-term goal",
            "personal growth",
        ],
    ),
    (
        "calendar-organizer",
        &[
            "schedule", "calendar", "time block", "appointment", "meeting", "block time",
            "availability",
        ],
    ),
    (
        "day-prioritizer",
        &[
            "prioritize", "priority", "what should i do", "focus today", "top 3", "today's plan",
            "overwhelmed", "pick three", "daily plan",
        ],
    ),
    (
        "progress-reviewer",
        &[
            "weekly review", "progress review", "retrospective", "how am i doing", "catch up",
            "review my week", "what did i finish",
        ],
    ),
    (
        "life-admin",
        &["errand", "chore", "admin", "clean", "laundry", "dry cleaning", "renew", "paperwork"],
    ),
];

const WORD_BOUNDARY_KEYWORDS: &[(&str, &[&str])] =
    &[("day-prioritizer", &["today", "tomorrow"]), ("life-admin", &["todo", "task"])];

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// `re.search(rf"\b{kw}\b", text)` for an ASCII keyword. Python's `\b` is
/// Unicode-aware (a letter like 'é' counts as a word character); this checks
/// ASCII word-ness only, so a keyword immediately touching a non-ASCII letter
/// with no ASCII separator is a narrower gap here than in Python. Every
/// keyword in this module is a plain ASCII word, so it is not reachable from
/// this file's own data — only from an adversarial message.
fn word_boundary_contains(text: &str, kw: &str) -> bool {
    let bytes = text.as_bytes();
    let mut start = 0;
    while let Some(pos) = text.get(start..).and_then(|t| t.find(kw)) {
        let idx = start + pos;
        let before_ok = idx == 0 || !is_word_byte(bytes[idx - 1]);
        let after = idx + kw.len();
        let after_ok = after >= bytes.len() || !is_word_byte(bytes[after]);
        if before_ok && after_ok {
            return true;
        }
        start = idx + 1;
    }
    false
}

/// `_keyword_matches`: a keyword with a space or over 8 characters is a plain
/// substring test; a short single word gets the word-boundary check (so
/// "run" does not match "running" — Python's rule, not this port's).
fn keyword_matches(text: &str, keyword: &str) -> bool {
    let kw = keyword.trim();
    if kw.is_empty() {
        return false;
    }
    if kw.contains(' ') || kw.chars().count() > 8 {
        text.contains(kw)
    } else {
        word_boundary_contains(text, kw)
    }
}

/// `assistant_router.route_profile_slug`.
fn route_profile_slug(message: &str, explicit: Option<&str>) -> String {
    if let Some(e) = explicit.map(str::trim).filter(|e| !e.is_empty()) {
        return e.to_string();
    }
    let text = message.to_lowercase();

    // Insertion-ordered like a Python dict: an existing slug's score updates
    // in place, a new one is appended — needed for the tie-break below.
    let mut scores: Vec<(&str, i64)> = Vec::new();
    let mut bump = |slug: &'static str, delta: i64| {
        if delta == 0 {
            return;
        }
        if let Some(entry) = scores.iter_mut().find(|(s, _)| *s == slug) {
            entry.1 += delta;
        } else {
            scores.push((slug, delta));
        }
    };
    for (slug, keywords) in DOMAIN_KEYWORDS {
        let score = keywords.iter().filter(|kw| keyword_matches(&text, kw)).count() as i64;
        bump(slug, score);
    }
    for (slug, keywords) in WORD_BOUNDARY_KEYWORDS {
        let extra = keywords.iter().filter(|kw| keyword_matches(&text, kw)).count() as i64;
        bump(slug, extra);
    }

    if scores.is_empty() {
        return DEFAULT_PROFILE.to_string();
    }
    // `max(scores, key=scores.get)`: the *first* maximum in iteration order,
    // not `Iterator::max_by_key`'s last — so this is a manual fold with a
    // strict `>` to keep the earlier entry on a tie.
    let mut best = scores[0];
    for &(slug, score) in &scores[1..] {
        if score > best.1 {
            best = (slug, score);
        }
    }
    best.0.to_string()
}

// ---------------------------------------------------------------------------
// Chat — domain profile helpers, `assistant/domain_forms.py`
// ---------------------------------------------------------------------------

/// `PROFILE_SLUG_TO_DOMAIN`, falling back to `"general"` like
/// `domain_for_profile_slug`.
fn domain_for_profile_slug(slug: &str) -> &'static str {
    match slug {
        "fitness-coach" => "fitness",
        "travel-planner" => "travel",
        "nutrition-coach" => "nutrition",
        "finance-planner" => "finance",
        "professional-planner" => "professional",
        "mentorship-coach" => "professional",
        "personal-assistant" => "general",
        "life-admin" => "general",
        "shopping-planner" => "nutrition",
        "calendar-organizer" => "general",
        "sprint-planner" => "general",
        "code-task-planner" => "general",
        "research-scout" => "general",
        "day-prioritizer" => "general",
        "habit-coach" => "general",
        "progress-reviewer" => "general",
        _ => "general",
    }
}

/// `DOMAIN_PROFILE_FIELDS` — order matches `build_profile_context`'s gap scan.
const DOMAIN_PROFILE_FIELDS: &[(&str, &[&str])] = &[
    (
        "fitness",
        &["sex", "age", "height_cm", "weight_kg", "fitness_goal", "experience_level", "equipment"],
    ),
    (
        "travel",
        &["destination", "departure_date", "return_date", "travelers", "budget", "travel_style"],
    ),
    ("nutrition", &["diet_style", "meals_per_day", "cooking_time_minutes"]),
    ("finance", &["monthly_budget", "savings_goal", "primary_focus"]),
    ("professional", &["current_role", "target_role", "growth_focus"]),
];

/// `missing_profile_fields`: `None`, `""` and `[]` all count as missing.
fn missing_profile_fields(domain: &str, profile: &Map<String, Value>) -> Vec<String> {
    let Some((_, fields)) = DOMAIN_PROFILE_FIELDS.iter().find(|(d, _)| *d == domain) else {
        return Vec::new();
    };
    fields
        .iter()
        .filter(|key| {
            let value = profile.get(**key);
            match value {
                None => true,
                Some(Value::Null) => true,
                Some(Value::String(s)) => s.is_empty(),
                Some(Value::Array(a)) => a.is_empty(),
                _ => false,
            }
        })
        .map(|s| s.to_string())
        .collect()
}

/// `get_domain_form_spec`: a lookup into the same constant `list_profile_forms`
/// serves, parsed fresh — six small objects, not worth caching.
pub(crate) fn domain_form_spec(domain: &str) -> Option<Value> {
    let forms: Value = serde_json::from_str(DOMAIN_FORMS_JSON).expect("DOMAIN_FORMS_JSON is valid");
    forms.as_object()?.get(domain).cloned()
}

// ---------------------------------------------------------------------------
// Chat — prompt context, `user_profile_service.build_profile_context` and
// `assistant_chat.build_board_context`
// ---------------------------------------------------------------------------

/// `build_profile_context`. Field order (`user_domain_profiles`, `profile_gaps`,
/// `active_domain`, `active_profile`, `active_profile_gaps`,
/// `domain_form_templates`) is what a prompt built from this dict shows the
/// model, via [`py_repr`](crate::todos::py_repr) — not cosmetic.
async fn build_profile_context(
    state: &AppState,
    project_id: i64,
    profile_slug: &str,
) -> Result<Map<String, Value>, ApiError> {
    let domain = domain_for_profile_slug(profile_slug);
    let profiles = all_profiles(state, project_id).await?;

    let mut gaps = Map::new();
    for (dom, _fields) in DOMAIN_PROFILE_FIELDS {
        let empty = Map::new();
        let profile_obj = profiles.get(*dom).and_then(Value::as_object).unwrap_or(&empty);
        let missing = missing_profile_fields(dom, profile_obj);
        if !missing.is_empty() {
            gaps.insert((*dom).to_string(), Value::Array(missing.into_iter().map(Value::String).collect()));
        }
    }

    let active_domain = (domain != "general").then_some(domain);
    let active_profile = active_domain
        .and_then(|d| profiles.get(d))
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let active_gaps = active_domain
        .and_then(|d| gaps.get(d))
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));

    let mut form_templates = Map::new();
    if let Some(d) = active_domain {
        if matches!(&active_gaps, Value::Array(a) if !a.is_empty()) {
            if let Some(spec) = domain_form_spec(d) {
                form_templates.insert(d.to_string(), spec);
            }
        }
    }

    let mut ctx = Map::new();
    ctx.insert("user_domain_profiles".into(), Value::Object(profiles));
    ctx.insert("profile_gaps".into(), Value::Object(gaps));
    ctx.insert("active_domain".into(), active_domain.map(Value::from).unwrap_or(Value::Null));
    ctx.insert("active_profile".into(), active_profile);
    ctx.insert("active_profile_gaps".into(), active_gaps);
    ctx.insert("domain_form_templates".into(), Value::Object(form_templates));
    Ok(ctx)
}

/// `assistant_chat.build_board_context` — Python's `project_id` parameter is
/// unused in the function body there too, so it is dropped here rather than
/// carried as dead weight. No `ORDER BY` on the categories query — Python's has
/// none either, unlike the dashboard's — and the item query caps at 50,
/// most-recently-updated first. `board`/`categories` carry the same category
/// list twice, like Python's dict.
async fn build_board_context(state: &AppState, board_id: i64) -> Result<Map<String, Value>, ApiError> {
    let board_name: Option<String> = sqlx::query_scalar("SELECT name FROM todo_boards WHERE id = ?")
        .bind(board_id)
        .fetch_optional(&state.pool)
        .await?;

    let categories: Vec<CategoryRow> = sqlx::query_as(&format!(
        "SELECT {CATEGORY_COLUMNS} FROM todo_categories WHERE board_id = ?"
    ))
    .bind(board_id)
    .fetch_all(&state.pool)
    .await?;
    let category_briefs: Vec<Value> = categories
        .iter()
        .map(|c| json!({ "id": c.id, "name": c.name, "planner_profile_id": c.planner_profile_id }))
        .collect();

    let items: Vec<ItemRow> = sqlx::query_as(&format!(
        "SELECT {ITEM_COLUMNS} FROM todo_items WHERE board_id = ? ORDER BY updated_at DESC LIMIT 50"
    ))
    .bind(board_id)
    .fetch_all(&state.pool)
    .await?;
    let item_briefs: Vec<Value> = items
        .iter()
        .map(|i| {
            json!({
                "id": i.id,
                "title": i.title,
                "status": i.status,
                "item_kind": i.item_kind,
                "time_horizon": i.time_horizon,
                "due_at": i.due_at.as_deref().map(iso_from_sql),
                "scheduled_at": i.scheduled_at.as_deref().map(iso_from_sql),
                "category_id": i.category_id,
                "parent_item_id": i.parent_item_id,
            })
        })
        .collect();

    let mut ctx = Map::new();
    ctx.insert(
        "board".into(),
        json!({ "id": board_id, "name": board_name, "categories": category_briefs.clone() }),
    );
    ctx.insert("items".into(), Value::Array(item_briefs));
    ctx.insert("board_id".into(), json!(board_id));
    ctx.insert("categories".into(), Value::Array(category_briefs));
    Ok(ctx)
}

// ---------------------------------------------------------------------------
// Chat — the planner profile, `_resolve_profile`
// ---------------------------------------------------------------------------

async fn load_profile_by_slug(
    state: &AppState,
    slug: &str,
) -> Result<Option<PlannerProfileRow>, ApiError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {PROFILE_COLUMNS} FROM planner_agent_profiles WHERE slug = ?"
    ))
    .bind(slug)
    .fetch_optional(&state.pool)
    .await?)
}

/// `_resolve_profile`: the named slug, falling back to `"personal-assistant"`
/// — a slug this project seeds, but a missing seed resolves to `None` like
/// Python's `session.exec(...).first()` on an empty result, not an error.
async fn resolve_profile(state: &AppState, slug: &str) -> Result<Option<PlannerProfileRow>, ApiError> {
    if let Some(row) = load_profile_by_slug(state, slug).await? {
        return Ok(Some(row));
    }
    load_profile_by_slug(state, DEFAULT_PROFILE).await
}

// ---------------------------------------------------------------------------
// Chat — thread resolution, `assistant_chat._resolve_thread` and friends
// ---------------------------------------------------------------------------

#[derive(FromRow, Clone)]
struct ChatThreadRow {
    id: i64,
    project_id: i64,
    title: Option<String>,
    messages_json: Option<String>,
    pending_actions_json: Option<String>,
    last_profile_slug: Option<String>,
    created_at: String,
    updated_at: String,
}

const CHAT_THREAD_COLUMNS: &str = "id, project_id, title, messages_json, pending_actions_json, \
     last_profile_slug, created_at, updated_at";

impl ChatThreadRow {
    fn messages(&self) -> Vec<Value> {
        json_array(self.messages_json.clone())
    }

    fn pending_actions(&self) -> Vec<Value> {
        json_array(self.pending_actions_json.clone())
    }
}

/// `_create_thread_row`. `set_messages([])` leaves `messages_json` `NULL`
/// (Python's `json.dumps(messages) if messages else None`), so the insert
/// does too rather than writing the literal string `"[]"`.
async fn create_thread_row(
    state: &AppState,
    project_id: i64,
    title: Option<&str>,
) -> Result<ChatThreadRow, ApiError> {
    let now = sql_now();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO assistant_chat_threads (project_id, title, created_at, updated_at) \
         VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(project_id)
    .bind(title)
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.pool)
    .await?;
    Ok(ChatThreadRow {
        id,
        project_id,
        title: title.map(str::to_string),
        messages_json: None,
        pending_actions_json: None,
        last_profile_slug: None,
        created_at: now.clone(),
        updated_at: now,
    })
}

async fn get_thread_by_id(
    state: &AppState,
    project_id: i64,
    thread_id: i64,
) -> Result<ChatThreadRow, ApiError> {
    let row: Option<ChatThreadRow> = sqlx::query_as(&format!(
        "SELECT {CHAT_THREAD_COLUMNS} FROM assistant_chat_threads WHERE id = ?"
    ))
    .bind(thread_id)
    .fetch_optional(&state.pool)
    .await?;
    match row {
        Some(row) if row.project_id == project_id => Ok(row),
        _ => Err(ApiError::not_found("Chat thread not found")),
    }
}

/// `_resolve_thread`: a named thread, else the most recently updated one, else
/// a freshly created `"New chat"` — the insert-when-empty that forces every
/// route touching it to move together (`plan.md`'s sub-step 6 note).
async fn resolve_thread(
    state: &AppState,
    project_id: i64,
    thread_id: Option<i64>,
) -> Result<ChatThreadRow, ApiError> {
    if let Some(thread_id) = thread_id {
        return get_thread_by_id(state, project_id, thread_id).await;
    }
    let row: Option<ChatThreadRow> = sqlx::query_as(&format!(
        "SELECT {CHAT_THREAD_COLUMNS} FROM assistant_chat_threads \
         WHERE project_id = ? ORDER BY updated_at DESC LIMIT 1"
    ))
    .bind(project_id)
    .fetch_optional(&state.pool)
    .await?;
    match row {
        Some(row) => Ok(row),
        None => create_thread_row(state, project_id, Some("New chat")).await,
    }
}

// ---------------------------------------------------------------------------
// Chat — the turn generator, `assistant_chat._generate_assistant_turn` and
// `send_chat_message`
// ---------------------------------------------------------------------------

const PA_SYSTEM_PROMPT: &str = "You are the Personal Assistant for a user's daily life planning board.\n\
\n\
Your role:\n\
- Understand what the user needs and create organized, actionable plans\n\
- The USER executes tasks — you plan, schedule, and organize\n\
- Check user_domain_profiles and profile_gaps in context BEFORE creating tasks\n\
- When required profile fields are missing, use present_planning_form (prefer domain_form_templates)\n\
- When the user shares personal facts in chat, use store_user_profile to save them\n\
- Use create_item, schedule_item, set_due_date for actionable plans after you have enough context\n\
\n\
Never invent body stats, travel dates, or budget numbers — ask via form or chat first.";

/// `action_set_id`, filtered the way every other reader of it in this crate
/// does — `if item.assigned_profile_id:` is falsy on `0`, and Python's ORM
/// leaves the same column `0` rather than `NULL` on an unconfigured profile.
fn active_action_set(profile: &PlannerProfileRow) -> Option<i64> {
    profile.action_set_id.filter(|id| *id != 0)
}

/// `_assistant_context_usage`.
fn assistant_context_usage(
    profile: &PlannerProfileRow,
    messages: &[Value],
    board_context: &Map<String, Value>,
    profile_ctx: &Map<String, Value>,
    tools: Option<&[Value]>,
) -> ContextUsageOut {
    let system_parts: Vec<&str> =
        [PA_SYSTEM_PROMPT, profile.system_prompt.as_str()].into_iter().filter(|s| !s.is_empty()).collect();
    let system = system_parts.join("\n\n");

    let mut merged = board_context.clone();
    for (k, v) in profile_ctx {
        merged.insert(k.clone(), v.clone());
    }
    let injected = crate::dag_schema::python_json(&Value::Object(merged), false);

    estimate_context_usage(&ContextInputs {
        system_prompt: Some(&system),
        tools,
        conversation_messages: Some(messages),
        injected_context: Some(&injected),
        ..Default::default()
    })
}

/// `_chat_only`: one buffered completion with no tools, board + profile
/// context folded into the system prompt as Python's `str(dict)`.
async fn chat_only(
    state: &AppState,
    profile: &PlannerProfileRow,
    message: &str,
    history: &[Value],
    model: &str,
    board_id: i64,
    project_id: i64,
    profile_slug: &str,
) -> Result<(String, LlmStepUsageOut), ApiError> {
    let profile_ctx = build_profile_context(state, project_id, profile_slug).await?;
    let mut context = build_board_context(state, board_id).await?;
    for (k, v) in profile_ctx {
        context.insert(k, v);
    }

    // Not filtered like `assistant_context_usage`'s system prompt — a blank
    // `profile.system_prompt` still leaves the doubled `\n\n` Python's plain
    // `"\n\n".join` produces, the same divergence `todos.rs::agent_chat` notes.
    let system_parts = [
        PA_SYSTEM_PROMPT.to_string(),
        profile.system_prompt.clone(),
        format!("Context:\n{}", py_repr(&Value::Object(context))),
    ];
    let mut messages = vec![json!({ "role": "system", "content": system_parts.join("\n\n") })];
    let prior = &history[..history.len().saturating_sub(1)];
    for h in prior {
        let Some(obj) = h.as_object() else { continue };
        let Some(content) = obj.get("content").filter(|v| py_truthy(v)) else { continue };
        let role = obj.get("role").and_then(Value::as_str).unwrap_or("user");
        messages.push(json!({ "role": role, "content": content }));
    }
    messages.push(json!({ "role": "user", "content": message }));

    if state.master_key.is_none() {
        return Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "AGENT_PLATFORM_MASTER_KEY is not set."));
    }

    let (fitted, _) = crate::context_budget::fit_chat_messages_for_request(messages);
    let mut payload = Map::new();
    payload.insert("messages".into(), Value::Array(fitted));
    payload.insert("max_tokens".into(), json!(crate::context_budget::max_output_tokens_default()));
    if let Some(sm) = sanitize_llm_model_alias(model) {
        payload.insert("model".into(), json!(sm));
    }

    let data = crate::llm::complete_internal(state, payload).await.map_err(|e| {
        ApiError::new(StatusCode::BAD_GATEWAY, format!("LLM proxy returned HTTP {}", e.status.as_u16()))
    })?;
    let usage = crate::chat_usage::parse_llm_usage(&data, Some("chat_only"));
    let content = data
        .get("choices")
        .and_then(Value::as_array)
        .filter(|c| !c.is_empty())
        .and_then(|c| c.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Ok((content, usage))
}

/// `_generate_assistant_turn`. `thread` is mutated and persisted here — the
/// caller (`send_chat_message`) has already committed the user's own turn
/// before this runs, so an LLM failure loses nothing already saved.
async fn generate_assistant_turn(
    state: &AppState,
    project_id: i64,
    thread: &mut ChatThreadRow,
    message: &str,
    model: Option<&str>,
    delegate_slug: Option<&str>,
    propose_actions: bool,
) -> Result<Value, ApiError> {
    let board_id = ensure_assistant_board(state, project_id).await?;
    let board_default_model: Option<String> =
        sqlx::query_scalar("SELECT default_model FROM todo_boards WHERE id = ?")
            .bind(board_id)
            .fetch_one(&state.pool)
            .await?;

    let profile_slug = route_profile_slug(message, delegate_slug);
    let profile = resolve_profile(state, &profile_slug)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "No planner profile configured"))?;

    let mut messages = thread.messages();
    let llm_model = model
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| profile.default_model.clone().filter(|s| !s.is_empty()))
        .or_else(|| board_default_model.filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "gemma4:31b-cloud".to_string());

    let mut content = String::new();
    let mut planned_out: Vec<PlannedAction> = Vec::new();
    let mut thought: Option<String> = None;
    let mut pending_form: Option<Value> = None;

    let profile_ctx = build_profile_context(state, project_id, &profile_slug).await?;
    let board_context = build_board_context(state, board_id).await?;
    let action_set_id = active_action_set(&profile);
    let actions = match action_set_id {
        Some(set_id) => list_actions(state, set_id).await?,
        None => Vec::new(),
    };
    let tools = (!actions.is_empty()).then(|| build_action_tools(&actions));
    let context_usage =
        assistant_context_usage(&profile, &messages, &board_context, &profile_ctx, tools.as_deref());
    let mut usage_steps: Vec<LlmStepUsageOut> = Vec::new();

    if propose_actions && action_set_id.is_some() {
        let mut turn_context = board_context.clone();
        for (k, v) in &profile_ctx {
            turn_context.insert(k.clone(), v.clone());
        }
        turn_context.insert("planner_system_prompt".into(), json!(profile.system_prompt));
        turn_context.insert("personal_assistant_prompt".into(), json!(PA_SYSTEM_PROMPT));
        let prior = &messages[..messages.len().saturating_sub(1)];
        turn_context.insert(
            "conversation_history".into(),
            Value::Array(crate::assistant_turn::format_conversation_for_planner(prior)),
        );

        let (planned, decided_thought, decide_steps) =
            decide_actions(state, message, &turn_context, &actions, &llm_model).await;
        usage_steps.extend(decide_steps);
        thought = decided_thought;

        planned_out = crate::assistant_turn::strip_redundant_profile_saves(planned, message);
        let active_profile = profile_ctx.get("active_profile").and_then(Value::as_object).cloned();
        planned_out = crate::assistant_turn::normalize_planned_actions(planned_out, active_profile.as_ref());
        planned_out = crate::assistant_turn::maybe_inject_domain_form(&profile_ctx, planned_out);
        pending_form = crate::assistant_turn::extract_pending_form(&planned_out).map(Value::Object);
        let task_actions = crate::assistant_turn::actions_without_forms(&planned_out);
        let needs_approval = crate::assistant_turn::pending_requires_approval(&planned_out);

        if pending_form.is_some() && task_actions.is_empty() {
            let clarify_actions: Vec<PlannedAction> =
                planned_out.iter().filter(|a| a.action_id == "ask_clarifying_questions").cloned().collect();
            if !clarify_actions.is_empty() && is_clarifying_form(pending_form.as_ref()) {
                content = crate::assistant_turn::assistant_reply_for_actions(&clarify_actions, thought.as_deref());
            } else {
                content = pending_form
                    .as_ref()
                    .and_then(|f| f.get("description"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| "Please fill in a few details so I can plan this properly.".to_string());
                if crate::assistant_turn::thought_is_user_facing(thought.as_deref()) {
                    content = thought.as_deref().unwrap().trim().to_string();
                }
            }
        } else if !task_actions.is_empty() {
            content = crate::assistant_turn::assistant_reply_for_actions(&task_actions, thought.as_deref());
        } else if crate::assistant_turn::thought_is_user_facing(thought.as_deref()) {
            content = thought.as_deref().unwrap().trim().to_string();
        } else {
            let (chat_content, chat_step) =
                chat_only(state, &profile, message, &messages, &llm_model, board_id, project_id, &profile_slug)
                    .await?;
            usage_steps.push(chat_step);
            content = chat_content;
            if content.trim().is_empty() {
                content = "Tell me a bit more about what you want on your board — for example \
                            meals for the week, prep day, or dietary constraints."
                    .to_string();
            }
        }

        let persist_pending =
            needs_approval || crate::assistant_turn::pending_has_interactive_form(&planned_out);
        let proposed = (!planned_out.is_empty()).then_some(planned_out.as_slice());
        if persist_pending {
            thread.pending_actions_json = if planned_out.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&planned_out).expect("PlannedAction serializes"))
            };
            messages.push(crate::assistant_turn::assistant_message_with_usage(
                &content,
                usage_steps.clone(),
                proposed,
            ));
        } else {
            thread.pending_actions_json = None;
            messages.push(crate::assistant_turn::assistant_message_with_usage(&content, usage_steps.clone(), None));
        }
    } else {
        let (chat_content, chat_step) =
            chat_only(state, &profile, message, &messages, &llm_model, board_id, project_id, &profile_slug).await?;
        usage_steps.push(chat_step);
        content = chat_content;
        messages.push(crate::assistant_turn::assistant_message_with_usage(&content, usage_steps.clone(), None));
    }

    let turn_usage = merge_llm_usages(usage_steps);

    thread.messages_json =
        if messages.is_empty() { None } else { Some(serde_json::to_string(&messages).expect("messages serialize")) };
    thread.last_profile_slug = Some(profile_slug.clone());
    thread.updated_at = sql_now();
    persist_thread(state, thread).await?;

    let pending_raw = thread.pending_actions();
    if pending_form.is_none() {
        let pending_for_form: Vec<PlannedAction> =
            pending_raw.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect();
        pending_form = crate::assistant_turn::extract_pending_form(&pending_for_form).map(Value::Object);
    }

    Ok(json!({
        "thread_id": thread.id,
        "content": content,
        "model": llm_model,
        "profile_slug": profile_slug,
        "thought": thought,
        "actions": pending_raw,
        "messages": messages,
        "pending_actions": thread.pending_actions(),
        "pending_form": pending_form,
        "board_id": board_id,
        "domain_profiles": profile_ctx.get("user_domain_profiles").cloned().unwrap_or_else(|| json!({})),
        "context_window": context_usage.context_window,
        "context_usage": context_usage,
        "usage": turn_usage,
    }))
}

/// Write back the columns every commit in this flow touches. No two-writer
/// hazard on this table (Rust is the only writer once this route ships), so —
/// unlike the CRUD-discipline modules elsewhere in this crate — there is no
/// need to track exactly which of these five columns a given call actually
/// changed; writing all five back is equivalent to Python's per-attribute
/// dirty tracking here, not a regression of it.
async fn persist_thread(state: &AppState, thread: &ChatThreadRow) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE assistant_chat_threads SET title = ?, messages_json = ?, pending_actions_json = ?, \
         last_profile_slug = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&thread.title)
    .bind(&thread.messages_json)
    .bind(&thread.pending_actions_json)
    .bind(&thread.last_profile_slug)
    .bind(&thread.updated_at)
    .bind(thread.id)
    .execute(&state.pool)
    .await?;
    Ok(())
}

/// `send_chat_message`: append the user's turn (committed on its own, so an
/// LLM failure cannot lose it), generate the reply, then resolve and persist
/// the smart title alongside whatever the turn itself already wrote.
async fn send_chat_message(
    state: &Arc<AppState>,
    project_id: i64,
    message: &str,
    thread_id: Option<i64>,
    model: Option<&str>,
    delegate_slug: Option<&str>,
    propose_actions: bool,
) -> Result<Value, ApiError> {
    let mut thread = resolve_thread(state, project_id, thread_id).await?;

    let mut fallback_title =
        thread.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| "New chat".to_string());
    let mut title_task = None;
    if is_placeholder_title(thread.title.as_deref(), &DEFAULT_PLACEHOLDERS) {
        fallback_title = fallback_title_from_message(message, "New chat");
        thread.title = Some(fallback_title.clone());
        title_task = start_smart_title_task(state.clone(), message, model);
    }

    let mut messages = thread.messages();
    crate::assistant_turn::resolve_pending_proposal_in_messages(&mut messages, "superseded");
    let stale_pending: Vec<PlannedAction> =
        thread.pending_actions().iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect();
    if crate::assistant_turn::pending_is_informational_only(&stale_pending) {
        thread.pending_actions_json = None;
    }
    messages.push(json!({ "role": "user", "content": message }));
    thread.messages_json = Some(serde_json::to_string(&messages).expect("messages serialize"));
    thread.updated_at = sql_now();
    persist_thread(state, &thread).await?;

    let mut result = generate_assistant_turn(
        state,
        project_id,
        &mut thread,
        message,
        model,
        delegate_slug,
        propose_actions,
    )
    .await?;

    let final_title = await_smart_title(title_task, &fallback_title).await;
    if thread.title.as_deref().unwrap_or("") != final_title {
        thread.title = Some(final_title.clone());
        thread.updated_at = sql_now();
        persist_thread(state, &thread).await?;
    }
    if let Some(obj) = result.as_object_mut() {
        obj.insert("title".into(), Value::String(final_title));
    }
    Ok(result)
}

/// `_thread_payload`.
async fn thread_payload(state: &AppState, thread: &ChatThreadRow, project_id: i64) -> Result<Value, ApiError> {
    let board_id = ensure_assistant_board(state, project_id).await?;
    let pending_raw = thread.pending_actions();
    let pending: Vec<PlannedAction> =
        pending_raw.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect();
    let pending_form = crate::assistant_turn::extract_pending_form(&pending).map(Value::Object);

    let profile_slug = thread.last_profile_slug.clone().unwrap_or_else(|| DEFAULT_PROFILE.to_string());
    let profile_ctx = build_profile_context(state, project_id, &profile_slug).await?;
    let profile = resolve_profile(state, &profile_slug).await?;
    let messages = thread.messages();
    let board_context = build_board_context(state, board_id).await?;

    let mut context_window: Option<i64> = None;
    let mut context_usage: Option<ContextUsageOut> = None;
    if let Some(profile) = &profile {
        let tools = match active_action_set(profile) {
            Some(set_id) => Some(build_action_tools(&list_actions(state, set_id).await?)),
            None => None,
        };
        let usage = assistant_context_usage(profile, &messages, &board_context, &profile_ctx, tools.as_deref());
        context_window = Some(usage.context_window);
        context_usage = Some(usage);
    }

    Ok(json!({
        "thread_id": thread.id,
        "project_id": project_id,
        "board_id": board_id,
        "title": thread.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| "New chat".to_string()),
        "messages": messages,
        "pending_actions": pending_raw,
        "pending_form": pending_form,
        "last_profile_slug": thread.last_profile_slug,
        "domain_profiles": profile_ctx.get("user_domain_profiles").cloned().unwrap_or_else(|| json!({})),
        "context_window": context_window,
        "context_usage": context_usage,
    }))
}

/// `get_context_usage`. A pure read on its face, but `_resolve_thread` inserts
/// a `"New chat"` row on an empty database like `GET /chat/thread` does — see
/// the module docs.
async fn get_context_usage_payload(
    state: &AppState,
    project_id: i64,
    thread_id: Option<i64>,
) -> Result<ContextUsageOut, ApiError> {
    let thread = resolve_thread(state, project_id, thread_id).await?;
    let board_id = ensure_assistant_board(state, project_id).await?;
    let profile_slug = thread.last_profile_slug.clone().unwrap_or_else(|| DEFAULT_PROFILE.to_string());
    let profile = resolve_profile(state, &profile_slug)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "No planner profile configured"))?;
    let profile_ctx = build_profile_context(state, project_id, &profile_slug).await?;
    let board_context = build_board_context(state, board_id).await?;
    let tools = match active_action_set(&profile) {
        Some(set_id) => Some(build_action_tools(&list_actions(state, set_id).await?)),
        None => None,
    };
    Ok(assistant_context_usage(&profile, &thread.messages(), &board_context, &profile_ctx, tools.as_deref()))
}

// ---------------------------------------------------------------------------
// Routes: GET /chat/context-usage, GET /chat/thread, POST /chat/send
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ThreadQuery {
    project_id: Option<String>,
    #[serde(default)]
    thread_id: Option<String>,
}

/// `Query(default=None, ge=1)` on `thread_id` — `None` is fine, a present but
/// non-positive value is the same 422 shape `parse_project_id` produces.
fn parse_optional_thread_id(raw: Option<String>) -> Result<Option<i64>, ApiError> {
    let Some(raw) = raw.filter(|r| !r.is_empty()) else { return Ok(None) };
    match raw.parse::<i64>() {
        Ok(id) if id >= 1 => Ok(Some(id)),
        Ok(_) => Err(ApiError::validation(vec![json!({
            "type": "greater_than_equal", "loc": ["query", "thread_id"],
            "msg": "Input should be greater than or equal to 1",
        })])),
        Err(_) => Err(ApiError::validation(vec![json!({
            "type": "int_parsing", "loc": ["query", "thread_id"],
            "msg": "Input should be a valid integer, unable to parse string as an integer",
        })])),
    }
}

async fn chat_context_usage(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<ThreadQuery>,
) -> Result<Response, ApiError> {
    let project_id = require_project(&state, &principal, q.project_id).await?;
    let thread_id = parse_optional_thread_id(q.thread_id)?;
    let usage = get_context_usage_payload(&state, project_id, thread_id).await?;
    Ok(Json(usage).into_response())
}

async fn chat_thread(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<ThreadQuery>,
) -> Result<Response, ApiError> {
    let project_id = require_project(&state, &principal, q.project_id).await?;
    let thread_id = parse_optional_thread_id(q.thread_id)?;
    let thread = resolve_thread(&state, project_id, thread_id).await?;
    let payload = thread_payload(&state, &thread, project_id).await?;
    Ok(Json(payload).into_response())
}

#[derive(Deserialize)]
struct ChatSendBody {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    thread_id: Option<i64>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    delegate_slug: Option<String>,
    #[serde(default = "default_true")]
    propose_actions: bool,
}

fn default_true() -> bool {
    true
}

async fn chat_send(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<ProjectIdQuery>,
    body: Option<Json<ChatSendBody>>,
) -> Result<Response, ApiError> {
    let project_id = require_project(&state, &principal, q.project_id).await?;
    let Some(Json(body)) = body else {
        return Err(ApiError::validation(vec![json!({
            "type": "missing", "loc": ["body"], "msg": "Field required",
        })]));
    };
    let message = match body.message.as_deref() {
        None => {
            return Err(ApiError::validation(vec![ApiError::field_error(
                "message",
                "missing",
                "Field required",
            )]))
        }
        Some(m) if m.is_empty() => {
            return Err(ApiError::validation(vec![ApiError::field_error(
                "message",
                "string_too_short",
                "String should have at least 1 character",
            )]))
        }
        Some(m) => m,
    };

    let mut result = send_chat_message(
        &state,
        project_id,
        message,
        body.thread_id,
        body.model.as_deref(),
        body.delegate_slug.as_deref(),
        body.propose_actions,
    )
    .await?;
    // `ChatSendResponse` has no `title` field, and pydantic's default `extra`
    // is to drop unknown keys on validation — the smart title is persisted by
    // `send_chat_message` above, but never reaches this response body.
    if let Some(obj) = result.as_object_mut() {
        obj.remove("title");
    }
    Ok(Json(result).into_response())
}

#[cfg(test)]
mod router_tests {
    use super::*;

    /// Every case here is `test_assistant_router.py` verbatim.
    #[test]
    fn matches_pythons_router_fixtures() {
        assert_eq!(route_profile_slug("buy groceries", Some("finance-planner")), "finance-planner");
        assert_eq!(
            route_profile_slug("Build my grocery list for Costco this week", None),
            "shopping-planner"
        );
        assert_eq!(
            route_profile_slug("Debug the failing API unit test in Python", None),
            "code-task-planner"
        );
        assert_eq!(
            route_profile_slug("Research and compare options for note-taking apps", None),
            "research-scout"
        );
        assert_eq!(
            route_profile_slug("What should I focus on today? I'm overwhelmed", None),
            "day-prioritizer"
        );
        assert_eq!(
            route_profile_slug("Run my weekly review — how am I doing?", None),
            "progress-reviewer"
        );
        assert_eq!(
            route_profile_slug("Help me build a morning routine streak", None),
            "habit-coach"
        );
        assert_ne!(route_profile_slug("Finish math homework tonight", None), "professional-planner");
        assert_eq!(route_profile_slug("Hello there", None), "personal-assistant");
    }

    #[test]
    fn missing_fields_treats_none_empty_string_and_empty_list_as_missing() {
        let mut profile = Map::new();
        profile.insert("sex".to_string(), json!("Female"));
        profile.insert("age".to_string(), json!(""));
        profile.insert("equipment".to_string(), json!([]));
        let missing = missing_profile_fields("fitness", &profile);
        assert!(missing.contains(&"age".to_string()));
        assert!(missing.contains(&"equipment".to_string()));
        assert!(missing.contains(&"height_cm".to_string()));
        assert!(!missing.contains(&"sex".to_string()));
    }

    fn planned_form(params: Value) -> PlannedAction {
        PlannedAction {
            action_id: "present_planning_form".into(),
            name: "Present planning form".into(),
            parameters: params.as_object().cloned().unwrap_or_default(),
            confidence: 0.9,
            reasoning: None,
        }
    }

    /// `_resolve_form_submit_domain`'s precedence, which decides which
    /// profile a submitted form is written to — an explicit non-`general`
    /// domain wins, then the pending action's own `domain`, then its form's,
    /// then the routed slug's.
    #[test]
    fn form_submit_domain_follows_pythons_precedence() {
        assert_eq!(resolve_form_submit_domain("  Fitness ", &[], None), "fitness");
        // "general" is treated as unset, so a pending action can override it.
        assert_eq!(
            resolve_form_submit_domain("general", &[planned_form(json!({"domain": " Travel "}))], None),
            "travel"
        );
        assert_eq!(
            resolve_form_submit_domain("", &[planned_form(json!({"form": {"domain": "NUTRITION"}}))], None),
            "nutrition"
        );
        // A blank action domain falls through to the form's, not to the slug.
        assert_eq!(
            resolve_form_submit_domain(
                "",
                &[planned_form(json!({"domain": "  ", "form": {"domain": "finance"}}))],
                Some("fitness-coach"),
            ),
            "finance"
        );
        assert_eq!(resolve_form_submit_domain("", &[], Some("fitness-coach")), "fitness");
        assert_eq!(resolve_form_submit_domain("", &[], None), "general");
    }

    /// Both submit-form branches build a synthetic *user* turn that the model
    /// then answers, so their wording is a prompt, not just copy.
    #[test]
    fn answer_summaries_render_the_way_python_writes_them() {
        let mut answers = Map::new();
        answers.insert("diet_style".into(), json!("vegetarian"));
        answers.insert("meals_per_day".into(), json!(3));
        answers.insert("allergies".into(), json!(["nuts", "shellfish"]));
        assert_eq!(
            format_answers_message("nutrition", &answers),
            "Saved nutrition profile:\n- diet style: vegetarian\n- meals per day: 3\n\
             - allergies: nuts, shellfish\nPlease continue planning using this information."
        );

        // The clarifying branch labels by field id, and has its own empty,
        // boolean and empty-list renderings.
        let form = json!({
            "purpose": "clarifying",
            "fields": [{"id": "budget", "label": "What is your budget?"}],
        });
        let mut clarifying = Map::new();
        clarifying.insert("budget".into(), json!("  1200  "));
        clarifying.insert("has_passport".into(), json!(true));
        clarifying.insert("stops".into(), json!([]));
        clarifying.insert("notes".into(), json!("   "));
        assert_eq!(
            crate::clarifying_form::format_clarifying_answers_message(&form, &clarifying),
            "My answers to your questions:\n\n- What is your budget?: 1200\n- has passport: Yes\n\
             - stops: (none)\n- notes: (skipped)\n\nPlease continue planning using these answers."
        );
    }
}
