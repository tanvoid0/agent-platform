//! Todo boards, ported from `app/todos/routes.py` + `services/board_service.py`.
//!
//! **The domain is whole.** Every route in `app/todos/routes.py` answers from
//! here, Python keeps no writer of `todo_boards`, `todo_categories`,
//! `todo_items` or `todo_item_events`, and the two-writer exception ADR 0007
//! carried for this domain is closed — not narrowed, closed.
//!
//! The discipline that kept it survivable while it was split stays house style:
//! only ever write the columns a user edited (`UPDATE … SET title = ?`), never
//! a whole row.
//!
//! `spawn-process` is also the one place here that writes the `process` table.
//! It inserts a `pending` row and stops — the response tells the caller to
//! `POST /processes/{id}/sync` — so it needs no executor. Its snapshot builder
//! is `pub(crate)` because `POST /processes` has to produce the same bytes.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::FromRow;

use crate::auth::Principal;
use crate::dag_schema::sanitize_llm_model_alias;
use crate::error::{ApiError, PathId};
// `resolved_team_color`, `with_default_accents` and `parse_roster` are the read
// path of `app/team_schema.py`; `spawn-process` snapshots a template through
// exactly the same three, so they are used, not copied.
use crate::teams::{
    parse_roster, random_palette_color, resolved_team_color, stable_palette_color,
    with_default_accents, TeamRoster,
};
use crate::wire::{check_len, datetime_to_sql, parse_body_typed, sql_now, sql_time, sql_time_opt};
use crate::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/todos/board-templates", get(board_templates))
        .route("/api/v1/todos/boards", get(list_boards).post(create_board))
        .route(
            "/api/v1/todos/boards/{board_id}",
            get(get_board).patch(update_board).delete(delete_board),
        )
        .route(
            "/api/v1/todos/boards/{board_id}/categories",
            get(list_categories).post(create_category),
        )
        .route(
            "/api/v1/todos/boards/{board_id}/categories/{category_id}",
            patch(update_category),
        )
        .route(
            "/api/v1/todos/boards/{board_id}/items",
            get(list_items).post(create_item),
        )
        .route(
            "/api/v1/todos/items/{item_id}",
            get(get_item).patch(update_item).delete(delete_item),
        )
        .route("/api/v1/todos/items/{item_id}/events", get(item_events))
        .route("/api/v1/todos/items/{item_id}/agent/apply", post(agent_apply))
        .route("/api/v1/todos/items/{item_id}/agent/chat", post(agent_chat))
        .route("/api/v1/todos/items/{item_id}/agent/step", post(agent_step))
        .route(
            "/api/v1/todos/items/{item_id}/planning-form/submit",
            post(planning_form_submit),
        )
        .route(
            "/api/v1/todos/items/{item_id}/spawn-process",
            post(spawn_process),
        )
        .route("/api/v1/todos/planner-profiles", get(planner_profiles))
}

pub(crate) const TODO_STATUSES: [&str; 5] = ["plan", "backlog", "in_progress", "review", "done"];
const TODO_TIME_HORIZONS: [&str; 4] = ["day", "week", "month", "goal"];
const TODO_ITEM_KINDS: [&str; 5] = ["task", "habit", "goal", "review", "chore"];

// ---------------------------------------------------------------------------
// Access
// ---------------------------------------------------------------------------

/// `require_scope`: the router-level check only proves the token is valid.
fn require_scope(principal: &Principal, scope: &str) -> Result<(), ApiError> {
    if principal.has_scope(scope) {
        return Ok(());
    }
    Err(ApiError {
        status: StatusCode::FORBIDDEN,
        code: "INSUFFICIENT_SCOPE",
        message: format!("Token lacks required scope '{scope}'."),
        extra: None,
    })
}

#[derive(FromRow)]
struct BoardOwner {
    project_id: Option<i64>,
}

/// `assert_token_board_access`: a board belongs to a project, a project to a
/// workspace. A board with no project is master-key-only.
async fn assert_board_access(
    state: &AppState,
    principal: &Principal,
    board_id: i64,
) -> Result<(), ApiError> {
    let board: Option<BoardOwner> =
        sqlx::query_as(&crate::db::sql("SELECT project_id FROM todo_boards WHERE id = ?", state.backend))
            .bind(board_id)
            .fetch_optional(&state.any)
            .await?;
    let Some(board) = board else {
        return Err(ApiError::not_found("Not found"));
    };
    match board.project_id {
        None if principal.workspace_id.is_some() => Err(ApiError::not_found("Not found")),
        None => Ok(()),
        Some(project_id) => crate::projects::assert_access(state, principal, project_id).await,
    }
}

pub(crate) async fn assert_item_access(
    state: &AppState,
    principal: &Principal,
    item_id: i64,
) -> Result<(), ApiError> {
    let board_id: Option<i64> = sqlx::query_scalar(&crate::db::sql("SELECT board_id FROM todo_items WHERE id = ?", state.backend))
        .bind(item_id)
        .fetch_optional(&state.any)
        .await?;
    match board_id {
        None => Err(ApiError::not_found("Not found")),
        Some(board_id) => assert_board_access(state, principal, board_id).await,
    }
}

// ---------------------------------------------------------------------------
// Wire shapes
// ---------------------------------------------------------------------------

#[derive(FromRow, Serialize)]
struct BoardOut {
    id: i64,
    project_id: Option<i64>,
    name: String,
    description: Option<String>,
    default_model: Option<String>,
    #[serde(serialize_with = "sql_time")]
    created_at: String,
    #[serde(serialize_with = "sql_time")]
    updated_at: String,
    #[sqlx(default)]
    category_count: i64,
    #[sqlx(default)]
    item_count: i64,
}

pub const BOARD_COLUMNS: &str = "CAST(id AS BIGINT) AS id, \
     CAST(project_id AS BIGINT) AS project_id, name, description, default_model, \
     CAST(created_at AS TEXT) AS created_at, CAST(updated_at AS TEXT) AS updated_at, \
     (SELECT COUNT(*) FROM todo_categories c WHERE c.board_id = b.id) AS category_count, \
     (SELECT COUNT(*) FROM todo_items i WHERE i.board_id = b.id) AS item_count";

// `pub(crate)`: `assistant.rs`'s dashboard reuses these rather than re-querying
// `todo_categories`/`todo_items` with a second copy of the same column list and
// palette-fallback logic.
#[derive(FromRow)]
pub(crate) struct CategoryRow {
    pub(crate) id: i64,
    pub(crate) board_id: i64,
    pub(crate) name: String,
    pub(crate) color: Option<String>,
    pub(crate) sort_order: i64,
    pub(crate) planner_profile_id: Option<i64>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Serialize)]
pub(crate) struct CategoryOut {
    id: i64,
    board_id: i64,
    name: String,
    /// Never null on the way out: an old row without one gets a deterministic
    /// palette color keyed by its id, so it does not change between reads.
    color: String,
    sort_order: i64,
    planner_profile_id: Option<i64>,
    #[serde(serialize_with = "sql_time")]
    created_at: String,
    #[serde(serialize_with = "sql_time")]
    updated_at: String,
}

impl From<CategoryRow> for CategoryOut {
    fn from(row: CategoryRow) -> Self {
        let color = match row.color.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            Some(c) => c.to_string(),
            None => stable_palette_color(&format!("category:{}", row.id)).to_string(),
        };
        Self {
            id: row.id,
            board_id: row.board_id,
            name: row.name,
            color,
            sort_order: row.sort_order,
            planner_profile_id: row.planner_profile_id,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

pub const CATEGORY_COLUMNS: &str = "CAST(id AS BIGINT) AS id, \
     CAST(board_id AS BIGINT) AS board_id, name, color, \
     CAST(sort_order AS BIGINT) AS sort_order, \
     CAST(planner_profile_id AS BIGINT) AS planner_profile_id, \
     CAST(created_at AS TEXT) AS created_at, CAST(updated_at AS TEXT) AS updated_at";

#[derive(FromRow, Clone)]
pub(crate) struct ItemRow {
    pub(crate) id: i64,
    pub(crate) board_id: i64,
    pub(crate) category_id: Option<i64>,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) status: String,
    pub(crate) priority: i64,
    pub(crate) tags_json: Option<String>,
    pub(crate) plan_json: Option<String>,
    pub(crate) metadata_json: Option<String>,
    pub(crate) assigned_profile_id: Option<i64>,
    pub(crate) linked_process_id: Option<i64>,
    pub(crate) parent_item_id: Option<i64>,
    pub(crate) due_at: Option<String>,
    pub(crate) scheduled_at: Option<String>,
    pub(crate) time_horizon: Option<String>,
    pub(crate) item_kind: Option<String>,
    pub(crate) recurrence_json: Option<String>,
    pub(crate) completion_json: Option<String>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

pub const ITEM_COLUMNS: &str = "CAST(id AS BIGINT) AS id, \
     CAST(board_id AS BIGINT) AS board_id, CAST(category_id AS BIGINT) AS category_id, \
     title, description, status, CAST(priority AS BIGINT) AS priority, \
     tags_json, plan_json, metadata_json, \
     CAST(assigned_profile_id AS BIGINT) AS assigned_profile_id, \
     CAST(linked_process_id AS BIGINT) AS linked_process_id, \
     CAST(parent_item_id AS BIGINT) AS parent_item_id, \
     CAST(due_at AS TEXT) AS due_at, CAST(scheduled_at AS TEXT) AS scheduled_at, \
     time_horizon, item_kind, recurrence_json, completion_json, \
     CAST(created_at AS TEXT) AS created_at, CAST(updated_at AS TEXT) AS updated_at";

#[derive(Serialize)]
pub(crate) struct ItemOut {
    id: i64,
    board_id: i64,
    category_id: Option<i64>,
    title: String,
    description: String,
    status: String,
    priority: i64,
    tags: Vec<Value>,
    plan: Vec<Value>,
    metadata: Map<String, Value>,
    assigned_profile_id: Option<i64>,
    linked_process_id: Option<i64>,
    parent_item_id: Option<i64>,
    #[serde(serialize_with = "sql_time_opt")]
    due_at: Option<String>,
    #[serde(serialize_with = "sql_time_opt")]
    scheduled_at: Option<String>,
    time_horizon: Option<String>,
    item_kind: Option<String>,
    recurrence: Map<String, Value>,
    completion: Map<String, Value>,
    #[serde(serialize_with = "sql_time")]
    created_at: String,
    #[serde(serialize_with = "sql_time")]
    updated_at: String,
}

/// Every JSON column here is best-effort on read: the accessors on the Python
/// model swallow a decode error and return an empty value rather than 500 on a
/// row an older build wrote.
fn json_array(raw: Option<String>) -> Vec<Value> {
    raw.and_then(|r| serde_json::from_str::<Value>(&r).ok())
        .and_then(|v| match v {
            Value::Array(a) => Some(a),
            _ => None,
        })
        .unwrap_or_default()
}

fn json_object(raw: Option<String>) -> Map<String, Value> {
    raw.and_then(|r| serde_json::from_str::<Value>(&r).ok())
        .and_then(|v| match v {
            Value::Object(o) => Some(o),
            _ => None,
        })
        .unwrap_or_default()
}

impl From<ItemRow> for ItemOut {
    fn from(row: ItemRow) -> Self {
        Self {
            id: row.id,
            board_id: row.board_id,
            category_id: row.category_id,
            title: row.title,
            description: row.description,
            status: row.status,
            priority: row.priority,
            tags: json_array(row.tags_json),
            plan: json_array(row.plan_json),
            metadata: json_object(row.metadata_json),
            assigned_profile_id: row.assigned_profile_id,
            linked_process_id: row.linked_process_id,
            parent_item_id: row.parent_item_id,
            due_at: row.due_at,
            scheduled_at: row.scheduled_at,
            time_horizon: row.time_horizon,
            item_kind: row.item_kind,
            recurrence: json_object(row.recurrence_json),
            completion: json_object(row.completion_json),
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

// ---------------------------------------------------------------------------
// Board templates (a constant table in Python, so a constant here too)
// ---------------------------------------------------------------------------

/// (slug, name, description, [(category, color, profile_slug)])
type Template = (&'static str, &'static str, &'static str, &'static [(&'static str, &'static str, &'static str)]);

const BOARD_TEMPLATES: [Template; 6] = [
    (
        "life-weekly",
        "This week",
        "Personal errands, health habits, and day-to-day admin for the week ahead.",
        &[
            ("Personal", "#10b981", "life-admin"),
            ("Errands", "#6366f1", "life-admin"),
            ("Health", "#f59e0b", "fitness-coach"),
        ],
    ),
    (
        "meal-plan",
        "Meal plan",
        "Weekly meals, nutrition goals, and grocery shopping.",
        &[
            ("Meals", "#22c55e", "nutrition-coach"),
            ("Shopping", "#8b5cf6", "shopping-planner"),
        ],
    ),
    (
        "travel-trip",
        "Trip planner",
        "Research, bookings, and packing for an upcoming trip.",
        &[
            ("Research", "#0ea5e9", "travel-planner"),
            ("Bookings", "#6366f1", "travel-planner"),
            ("Packing", "#10b981", "life-admin"),
        ],
    ),
    (
        "coding-sprint",
        "Dev sprint",
        "Features, bugs, and learning for a software sprint.",
        &[
            ("Features", "#4285F4", "code-task-planner"),
            ("Bugs", "#EA4335", "code-task-planner"),
            ("Learning", "#FBBC05", "research-scout"),
        ],
    ),
    (
        "mentorship",
        "Growth",
        "Goals, skills, and reflection with a mentorship coach.",
        &[
            ("Goals", "#a855f7", "mentorship-coach"),
            ("Skills", "#6366f1", "mentorship-coach"),
            ("Reflection", "#10b981", "life-admin"),
        ],
    ),
    (
        "personal-assistant",
        "Personal Assistant",
        "Daily planning board with domain categories for fitness, finance, professional \
         growth, travel, health, life admin, and goals.",
        &[
            ("Fitness", "#f59e0b", "fitness-coach"),
            ("Finance", "#10b981", "finance-planner"),
            ("Professional", "#6366f1", "professional-planner"),
            ("Travel", "#0ea5e9", "travel-planner"),
            ("Health", "#22c55e", "nutrition-coach"),
            ("Life Admin", "#64748b", "life-admin"),
            ("Goals", "#a855f7", "mentorship-coach"),
        ],
    ),
];

async fn board_templates() -> Response {
    let templates: Vec<Value> = BOARD_TEMPLATES
        .iter()
        .map(|(slug, name, description, categories)| {
            json!({
                "slug": slug,
                "name": name,
                "description": description,
                "categories": categories.iter().map(|(name, color, profile_slug)| json!({
                    "name": name, "color": color, "profile_slug": profile_slug,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    Json(json!({ "templates": templates })).into_response()
}

// ---------------------------------------------------------------------------
// Boards
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ProjectQuery {
    #[serde(default)]
    project_id: Option<i64>,
}

async fn load_board(state: &AppState, board_id: i64) -> Result<BoardOut, ApiError> {
    sqlx::query_as(&crate::db::sql(&format!("SELECT {BOARD_COLUMNS} FROM todo_boards b WHERE id = ?"), state.backend))
        .bind(board_id)
        .fetch_optional(&state.any)
        .await?
        .ok_or_else(|| ApiError::not_found("Board not found"))
}

async fn list_boards(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<ProjectQuery>,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:read")?;
    if principal.workspace_id.is_some() {
        let project_id = q.project_id.ok_or_else(|| {
            ApiError::bad_request("project_id is required for a workspace-scoped token.")
        })?;
        crate::projects::assert_access(&state, &principal, project_id).await?;
    }

    let boards: Vec<BoardOut> = match q.project_id {
        Some(project_id) => sqlx::query_as(&crate::db::sql(&format!(
            "SELECT {BOARD_COLUMNS} FROM todo_boards b WHERE project_id = ? ORDER BY id ASC"
        ), state.backend))
        .bind(project_id)
        .fetch_all(&state.any)
        .await?,
        None => {
            sqlx::query_as(&crate::db::sql(&format!("SELECT {BOARD_COLUMNS} FROM todo_boards b ORDER BY id ASC"), state.backend))
                .fetch_all(&state.any)
                .await?
        }
    };
    Ok(Json(json!({ "boards": boards })).into_response())
}

#[derive(Deserialize)]
struct BoardCreate {
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default = "default_board_model")]
    default_model: Option<String>,
    #[serde(default)]
    template_slug: Option<String>,
}

pub(crate) fn default_board_model() -> Option<String> {
    Some("gemma4:31b-cloud".into())
}

async fn create_board(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<ProjectQuery>,
    // Raw bytes, not `Option<Json<BoardCreate>>`: axum's `Json` extractor
    // only yields `None` for a body-less request with no `Content-Type` at
    // all — an empty body sent *with* `application/json` (an argument-less
    // POST from most clients) fails to parse and axum answers its own
    // plain-text 400 before this handler runs.
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:write")?;
    if principal.workspace_id.is_some() {
        let project_id = q.project_id.ok_or_else(|| {
            ApiError::bad_request("project_id is required for a workspace-scoped token.")
        })?;
        crate::projects::assert_access(&state, &principal, project_id).await?;
    }

    if body.is_empty() {
        return Err(ApiError::validation(vec![ApiError::field_error(
            "name", "missing", "Field required",
        )]));
    }
    let req: BoardCreate = parse_body_typed(&body)?;
    let mut errors = Vec::new();
    match req.name.as_deref() {
        None => errors.push(ApiError::field_error("name", "missing", "Field required")),
        Some(name) => check_len(&mut errors, &["name"], Some(name), 1, 256),
    }
    check_len(&mut errors, &["description"], req.description.as_deref(), 0, 4096);
    check_len(&mut errors, &["default_model"], req.default_model.as_deref(), 0, 128);
    check_len(&mut errors, &["template_slug"], req.template_slug.as_deref(), 0, 64);
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    let now = sql_now();
    let board_id: i64 = sqlx::query_scalar(&crate::db::sql(
        "INSERT INTO todo_boards (project_id, name, description, default_model, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?) RETURNING CAST(id AS BIGINT)", state.backend)
    )
    .bind(q.project_id)
    .bind(req.name.unwrap_or_default().trim())
    .bind(req.description.map(|d| d.trim().to_string()).filter(|d| !d.is_empty()))
    .bind(req.default_model)
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.any)
    .await?;

    if let Some(slug) = req.template_slug.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        // Deliberately after the insert, like Python: an unknown slug 400s and
        // leaves the (already committed) empty board behind.
        apply_board_template(&state, board_id, slug).await?;
    }

    Ok((StatusCode::CREATED, Json(load_board(&state, board_id).await?)).into_response())
}

pub(crate) async fn apply_board_template(
    state: &AppState,
    board_id: i64,
    slug: &str,
) -> Result<(), ApiError> {
    let Some((_, _, _, categories)) = BOARD_TEMPLATES.iter().find(|(s, ..)| *s == slug) else {
        return Err(ApiError::bad_request(format!("Unknown board template: {slug}")));
    };
    let now = sql_now();
    for (i, (name, color, profile_slug)) in categories.iter().enumerate() {
        let profile_id: Option<i64> =
            sqlx::query_scalar(&crate::db::sql("SELECT id FROM planner_agent_profiles WHERE slug = ?", state.backend))
                .bind(profile_slug)
                .fetch_optional(&state.any)
                .await?;
        sqlx::query(&crate::db::sql(
            "INSERT INTO todo_categories \
             (board_id, name, color, sort_order, planner_profile_id, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)", state.backend)
        )
        .bind(board_id)
        .bind(name)
        .bind(color)
        .bind(i as i64)
        .bind(profile_id)
        .bind(&now)
        .bind(&now)
        .execute(&state.any)
        .await?;
    }
    Ok(())
}

async fn get_board(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(board_id): PathId<i64>,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:read")?;
    assert_board_access(&state, &principal, board_id).await?;
    let board = load_board(&state, board_id).await?;

    // `record_board_visit`: opening a board is what makes "Continue planning"
    // point at it, so the read has a write in it.
    if let Some(project_id) = board.project_id {
        sqlx::query(&crate::db::sql("UPDATE project SET last_todo_board_id = ? WHERE id = ?", state.backend))
            .bind(board_id)
            .bind(project_id)
            .execute(&state.any)
            .await?;
    }

    let categories: Vec<CategoryOut> = sqlx::query_as::<_, CategoryRow>(&crate::db::sql(&format!(
        "SELECT {CATEGORY_COLUMNS} FROM todo_categories WHERE board_id = ? \
         ORDER BY sort_order ASC, id ASC"
    ), state.backend))
    .bind(board_id)
    .fetch_all(&state.any)
    .await?
    .into_iter()
    .map(CategoryOut::from)
    .collect();

    let items: Vec<ItemOut> = sqlx::query_as::<_, ItemRow>(&crate::db::sql(&format!(
        "SELECT {ITEM_COLUMNS} FROM todo_items WHERE board_id = ? ORDER BY updated_at DESC"
    ), state.backend))
    .bind(board_id)
    .fetch_all(&state.any)
    .await?
    .into_iter()
    .map(ItemOut::from)
    .collect();

    // BoardDetailOut extends BoardOut, so the board's own fields come first.
    let mut body = serde_json::to_value(&board).unwrap_or_else(|_| json!({}));
    body["categories"] = json!(categories);
    body["items"] = json!(items);
    Ok(Json(body).into_response())
}

#[derive(Deserialize)]
struct BoardUpdate {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    default_model: Option<String>,
}

async fn update_board(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(board_id): PathId<i64>,
    // Raw bytes, not `Option<Json<BoardUpdate>>` — see `create_board`'s
    // comment.
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:write")?;
    assert_board_access(&state, &principal, board_id).await?;
    load_board(&state, board_id).await?;

    if body.is_empty() {
        return Err(ApiError::validation(vec![]));
    }
    let req: BoardUpdate = parse_body_typed(&body)?;
    let mut errors = Vec::new();
    check_len(&mut errors, &["name"], req.name.as_deref(), 1, 256);
    check_len(&mut errors, &["default_model"], req.default_model.as_deref(), 0, 128);
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    if let Some(name) = req.name {
        sqlx::query(&crate::db::sql("UPDATE todo_boards SET name = ? WHERE id = ?", state.backend))
            .bind(name.trim())
            .bind(board_id)
            .execute(&state.any)
            .await?;
    }
    if let Some(description) = req.description {
        sqlx::query(&crate::db::sql("UPDATE todo_boards SET description = ? WHERE id = ?", state.backend))
            .bind(Some(description.trim().to_string()).filter(|d| !d.is_empty()))
            .bind(board_id)
            .execute(&state.any)
            .await?;
    }
    if let Some(model) = req.default_model {
        sqlx::query(&crate::db::sql("UPDATE todo_boards SET default_model = ? WHERE id = ?", state.backend))
            .bind(Some(model.trim().to_string()).filter(|m| !m.is_empty()))
            .bind(board_id)
            .execute(&state.any)
            .await?;
    }
    sqlx::query(&crate::db::sql("UPDATE todo_boards SET updated_at = ? WHERE id = ?", state.backend))
        .bind(sql_now())
        .bind(board_id)
        .execute(&state.any)
        .await?;

    Ok(Json(load_board(&state, board_id).await?).into_response())
}

async fn delete_board(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(board_id): PathId<i64>,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:write")?;
    assert_board_access(&state, &principal, board_id).await?;
    load_board(&state, board_id).await?;
    // No cascade, matching `session.delete(board)` with SQLite foreign keys off:
    // categories and items outlive the board. Changing that here would be a
    // behaviour change hiding inside a port.
    sqlx::query(&crate::db::sql("DELETE FROM todo_boards WHERE id = ?", state.backend))
        .bind(board_id)
        .execute(&state.any)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---------------------------------------------------------------------------
// Categories
// ---------------------------------------------------------------------------

async fn list_categories(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(board_id): PathId<i64>,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:read")?;
    assert_board_access(&state, &principal, board_id).await?;
    load_board(&state, board_id).await?;
    let categories: Vec<CategoryOut> = sqlx::query_as::<_, CategoryRow>(&crate::db::sql(&format!(
        "SELECT {CATEGORY_COLUMNS} FROM todo_categories WHERE board_id = ? \
         ORDER BY sort_order ASC, id ASC"
    ), state.backend))
    .bind(board_id)
    .fetch_all(&state.any)
    .await?
    .into_iter()
    .map(CategoryOut::from)
    .collect();
    Ok(Json(json!({ "categories": categories })).into_response())
}

#[derive(Deserialize)]
struct CategoryCreate {
    name: Option<String>,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    sort_order: i64,
    #[serde(default)]
    planner_profile_id: Option<i64>,
}

async fn create_category(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(board_id): PathId<i64>,
    // Raw bytes, not `Option<Json<CategoryCreate>>` — see `create_board`'s
    // comment.
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:write")?;
    assert_board_access(&state, &principal, board_id).await?;
    load_board(&state, board_id).await?;

    if body.is_empty() {
        return Err(ApiError::validation(vec![ApiError::field_error(
            "name", "missing", "Field required",
        )]));
    }
    let req: CategoryCreate = parse_body_typed(&body)?;
    let mut errors = Vec::new();
    match req.name.as_deref() {
        None => errors.push(ApiError::field_error("name", "missing", "Field required")),
        Some(name) => check_len(&mut errors, &["name"], Some(name), 1, 128),
    }
    check_len(&mut errors, &["color"], req.color.as_deref(), 0, 32);
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    // Blank normalises to absent, and absent means a random palette color —
    // categories are meant to be visually distinct without anyone picking.
    let color = match req.color.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => c.to_string(),
        None => random_palette_color(&[]),
    };
    let now = sql_now();
    let id: i64 = sqlx::query_scalar(&crate::db::sql(
        "INSERT INTO todo_categories \
         (board_id, name, color, sort_order, planner_profile_id, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING CAST(id AS BIGINT)", state.backend)
    )
    .bind(board_id)
    .bind(req.name.unwrap_or_default().trim())
    .bind(&color)
    .bind(req.sort_order)
    .bind(req.planner_profile_id)
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.any)
    .await?;

    Ok((StatusCode::CREATED, Json(load_category(&state, board_id, id).await?)).into_response())
}

async fn load_category(
    state: &AppState,
    board_id: i64,
    category_id: i64,
) -> Result<CategoryOut, ApiError> {
    let row: Option<CategoryRow> =
        sqlx::query_as(&crate::db::sql(&format!("SELECT {CATEGORY_COLUMNS} FROM todo_categories WHERE id = ?"), state.backend))
            .bind(category_id)
            .fetch_optional(&state.any)
            .await?;
    match row {
        Some(row) if row.board_id == board_id => Ok(row.into()),
        _ => Err(ApiError::not_found("Category not found")),
    }
}

#[derive(Deserialize)]
struct CategoryUpdate {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    sort_order: Option<i64>,
    #[serde(default)]
    planner_profile_id: Option<i64>,
}

async fn update_category(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId((board_id, category_id)): PathId<(i64, i64)>,
    // Raw bytes, not `Option<Json<CategoryUpdate>>` — see `update_board`'s
    // comment.
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:write")?;
    assert_board_access(&state, &principal, board_id).await?;
    load_category(&state, board_id, category_id).await?;

    if body.is_empty() {
        return Err(ApiError::validation(vec![]));
    }
    let req: CategoryUpdate = parse_body_typed(&body)?;
    let mut errors = Vec::new();
    check_len(&mut errors, &["name"], req.name.as_deref(), 1, 128);
    check_len(&mut errors, &["color"], req.color.as_deref(), 0, 32);
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    if let Some(name) = req.name {
        sqlx::query(&crate::db::sql("UPDATE todo_categories SET name = ? WHERE id = ?", state.backend))
            .bind(name.trim())
            .bind(category_id)
            .execute(&state.any)
            .await?;
    }
    if let Some(color) = req.color {
        sqlx::query(&crate::db::sql("UPDATE todo_categories SET color = ? WHERE id = ?", state.backend))
            .bind(color)
            .bind(category_id)
            .execute(&state.any)
            .await?;
    }
    if let Some(sort_order) = req.sort_order {
        sqlx::query(&crate::db::sql("UPDATE todo_categories SET sort_order = ? WHERE id = ?", state.backend))
            .bind(sort_order)
            .bind(category_id)
            .execute(&state.any)
            .await?;
    }
    if let Some(profile_id) = req.planner_profile_id {
        sqlx::query(&crate::db::sql("UPDATE todo_categories SET planner_profile_id = ? WHERE id = ?", state.backend))
            .bind(profile_id)
            .bind(category_id)
            .execute(&state.any)
            .await?;
    }
    sqlx::query(&crate::db::sql("UPDATE todo_categories SET updated_at = ? WHERE id = ?", state.backend))
        .bind(sql_now())
        .bind(category_id)
        .execute(&state.any)
        .await?;

    Ok(Json(load_category(&state, board_id, category_id).await?).into_response())
}

// ---------------------------------------------------------------------------
// Items
// ---------------------------------------------------------------------------

pub(crate) async fn load_item(state: &AppState, item_id: i64) -> Result<ItemOut, ApiError> {
    let row: Option<ItemRow> =
        sqlx::query_as(&crate::db::sql(&format!("SELECT {ITEM_COLUMNS} FROM todo_items WHERE id = ?"), state.backend))
            .bind(item_id)
            .fetch_optional(&state.any)
            .await?;
    row.map(ItemOut::from).ok_or_else(|| ApiError::not_found("Item not found"))
}

async fn list_items(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(board_id): PathId<i64>,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:read")?;
    assert_board_access(&state, &principal, board_id).await?;
    load_board(&state, board_id).await?;
    let items: Vec<ItemOut> = sqlx::query_as::<_, ItemRow>(&crate::db::sql(&format!(
        "SELECT {ITEM_COLUMNS} FROM todo_items WHERE board_id = ? ORDER BY updated_at DESC"
    ), state.backend))
    .bind(board_id)
    .fetch_all(&state.any)
    .await?
    .into_iter()
    .map(ItemOut::from)
    .collect();
    Ok(Json(json!({ "items": items })).into_response())
}

#[derive(Deserialize)]
struct ItemCreate {
    title: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default = "plan_status")]
    status: String,
    #[serde(default)]
    category_id: Option<i64>,
    #[serde(default)]
    priority: i64,
    #[serde(default)]
    tags: Vec<Value>,
    #[serde(default)]
    assigned_profile_id: Option<i64>,
    #[serde(default)]
    parent_item_id: Option<i64>,
    #[serde(default)]
    due_at: Option<String>,
    #[serde(default)]
    scheduled_at: Option<String>,
    #[serde(default)]
    time_horizon: Option<String>,
    #[serde(default = "task_kind")]
    item_kind: Option<String>,
    #[serde(default)]
    recurrence: Option<Map<String, Value>>,
}

fn plan_status() -> String {
    "plan".into()
}

fn task_kind() -> Option<String> {
    Some("task".into())
}

/// Pydantic rejects an out-of-range enum at validation time (422), so these are
/// 422s and not the 400 the service layer would raise.
fn check_enum(errors: &mut Vec<Value>, field: &str, value: Option<&str>, allowed: &[&str]) {
    let Some(value) = value else { return };
    if !allowed.contains(&value) {
        errors.push(ApiError::field_error_at(
            &[field],
            "value_error",
            &format!("Value error, {field} must be one of {}", tuple_repr(allowed)),
        ));
    }
}

/// Python interpolates a tuple into the message, so the message carries Python's
/// tuple syntax.
fn tuple_repr(values: &[&str]) -> String {
    let inner: Vec<String> = values.iter().map(|v| format!("'{v}'")).collect();
    format!("({})", inner.join(", "))
}

async fn create_item(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(board_id): PathId<i64>,
    // Raw bytes, not `Option<Json<ItemCreate>>` — see `create_board`'s
    // comment.
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:write")?;
    assert_board_access(&state, &principal, board_id).await?;
    load_board(&state, board_id).await?;

    if body.is_empty() {
        return Err(ApiError::validation(vec![ApiError::field_error(
            "title", "missing", "Field required",
        )]));
    }
    let req: ItemCreate = parse_body_typed(&body)?;
    let mut errors = Vec::new();
    match req.title.as_deref() {
        None => errors.push(ApiError::field_error("title", "missing", "Field required")),
        Some(title) => check_len(&mut errors, &["title"], Some(title), 1, 512),
    }
    check_enum(&mut errors, "status", Some(&req.status), &TODO_STATUSES);
    check_enum(&mut errors, "time_horizon", req.time_horizon.as_deref(), &TODO_TIME_HORIZONS);
    check_enum(&mut errors, "item_kind", req.item_kind.as_deref(), &TODO_ITEM_KINDS);
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    if let Some(category_id) = req.category_id {
        load_category(&state, board_id, category_id).await?;
    }

    let now = sql_now();
    let id: i64 = sqlx::query_scalar(&crate::db::sql(
        "INSERT INTO todo_items (board_id, category_id, title, description, status, priority, \
         tags_json, assigned_profile_id, parent_item_id, due_at, scheduled_at, time_horizon, \
         item_kind, recurrence_json, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING CAST(id AS BIGINT)", state.backend)
    )
    .bind(board_id)
    .bind(req.category_id)
    .bind(req.title.unwrap_or_default().trim())
    .bind(&req.description)
    .bind(&req.status)
    .bind(req.priority)
    // An empty list is stored as NULL, not "[]" — `set_tags` treats them alike.
    .bind((!req.tags.is_empty()).then(|| Value::Array(req.tags.clone()).to_string()))
    .bind(req.assigned_profile_id)
    .bind(req.parent_item_id)
    .bind(req.due_at.as_deref().map(datetime_to_sql))
    .bind(req.scheduled_at.as_deref().map(datetime_to_sql))
    .bind(req.time_horizon)
    .bind(req.item_kind.or_else(task_kind))
    .bind(
        req.recurrence
            .filter(|r| !r.is_empty())
            .map(|r| Value::Object(r).to_string()),
    )
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.any)
    .await?;

    Ok((StatusCode::CREATED, Json(load_item(&state, id).await?)).into_response())
}

async fn get_item(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(item_id): PathId<i64>,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:read")?;
    assert_item_access(&state, &principal, item_id).await?;
    Ok(Json(load_item(&state, item_id).await?).into_response())
}

async fn update_item(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(item_id): PathId<i64>,
    // Raw bytes, not `Option<Json<Value>>` — see `create_board`'s comment.
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:write")?;
    assert_item_access(&state, &principal, item_id).await?;
    let item = load_item(&state, item_id).await?;

    let patch: Map<String, Value> = match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };
    let str_field = |key: &str| patch.get(key).and_then(Value::as_str).map(str::to_owned);

    let mut errors = Vec::new();
    check_len(&mut errors, &["title"], str_field("title").as_deref(), 1, 512);
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    // `ItemUpdate` has no enum validators, so an unknown status is the service's
    // 400 rather than a 422 like it is on create.
    if let Some(status) = str_field("status") {
        if !TODO_STATUSES.contains(&status.as_str()) {
            return Err(ApiError::bad_request(format!("Invalid status: {status}")));
        }
        set_item_column(&state, item_id, "status", status).await?;
    }
    if let Some(title) = str_field("title") {
        set_item_column(&state, item_id, "title", title.trim().to_string()).await?;
    }
    if let Some(description) = str_field("description") {
        set_item_column(&state, item_id, "description", description).await?;
    }
    if let Some(category_id) = patch.get("category_id").and_then(Value::as_i64) {
        load_category(&state, item.board_id, category_id).await?;
        set_item_column(&state, item_id, "category_id", category_id).await?;
    }
    for (key, column) in [
        ("priority", "priority"),
        ("assigned_profile_id", "assigned_profile_id"),
        ("parent_item_id", "parent_item_id"),
    ] {
        if let Some(value) = patch.get(key).and_then(Value::as_i64) {
            set_item_column(&state, item_id, column, value).await?;
        }
    }
    for (key, column) in [("time_horizon", "time_horizon"), ("item_kind", "item_kind")] {
        if let Some(value) = str_field(key) {
            set_item_column(&state, item_id, column, value).await?;
        }
    }
    for (key, column) in [("due_at", "due_at"), ("scheduled_at", "scheduled_at")] {
        if let Some(value) = str_field(key) {
            set_item_column(&state, item_id, column, datetime_to_sql(&value)).await?;
        }
    }
    // JSON columns: an empty collection stores NULL, matching the setters.
    if let Some(Value::Array(tags)) = patch.get("tags") {
        let stored = (!tags.is_empty()).then(|| Value::Array(tags.clone()).to_string());
        set_item_column(&state, item_id, "tags_json", stored).await?;
    }
    if let Some(Value::Array(plan)) = patch.get("plan") {
        let stored = (!plan.is_empty()).then(|| Value::Array(plan.clone()).to_string());
        set_item_column(&state, item_id, "plan_json", stored).await?;
    }
    for (key, column) in [
        ("metadata", "metadata_json"),
        ("recurrence", "recurrence_json"),
        ("completion", "completion_json"),
    ] {
        if let Some(Value::Object(map)) = patch.get(key) {
            let stored = (!map.is_empty()).then(|| Value::Object(map.clone()).to_string());
            set_item_column(&state, item_id, column, stored).await?;
        }
    }

    sqlx::query(&crate::db::sql("UPDATE todo_items SET updated_at = ? WHERE id = ?", state.backend))
        .bind(sql_now())
        .bind(item_id)
        .execute(&state.any)
        .await?;

    Ok(Json(load_item(&state, item_id).await?).into_response())
}

/// One column at a time, never the whole row: the agent routes still write this
/// table from Python, and a full-row write here would undo whatever they set.
async fn set_item_column<T>(
    state: &AppState,
    item_id: i64,
    column: &str,
    value: T,
) -> Result<(), ApiError>
where
    T: for<'q> sqlx::Encode<'q, sqlx::Any> + sqlx::Type<sqlx::Any> + Send,
{
    // The column name is from a fixed list in this module, never from input.
    let sql = format!("UPDATE todo_items SET {column} = ? WHERE id = ?");
    sqlx::query(&crate::db::sql(&sql, state.backend)).bind(value).bind(item_id).execute(&state.any).await?;
    Ok(())
}

async fn delete_item(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(item_id): PathId<i64>,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:write")?;
    assert_item_access(&state, &principal, item_id).await?;
    load_item(&state, item_id).await?;
    sqlx::query(&crate::db::sql("DELETE FROM todo_items WHERE id = ?", state.backend))
        .bind(item_id)
        .execute(&state.any)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---------------------------------------------------------------------------
// Events and planner profiles
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct EventQuery {
    #[serde(default)]
    after_id: i64,
    #[serde(default = "default_event_limit")]
    limit: i64,
}

fn default_event_limit() -> i64 {
    200
}

#[derive(FromRow)]
struct EventRow {
    id: i64,
    item_id: i64,
    event_type: String,
    content_json: String,
    created_at: String,
}

/// Append to the item's event log. The log is append-only, which is why it was
/// never part of the two-writer hazard: nothing here can overwrite a row Python
/// wrote, or the other way round.
pub(crate) async fn append_item_event(
    state: &AppState,
    item_id: i64,
    event_type: &str,
    content: Value,
) -> Result<(), ApiError> {
    sqlx::query(&crate::db::sql(
        "INSERT INTO todo_item_events (item_id, event_type, content_json, created_at) \
         VALUES (?, ?, ?, ?)", state.backend)
    )
    .bind(item_id)
    .bind(event_type)
    .bind(content.to_string())
    .bind(sql_now())
    .execute(&state.any)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Applying agent-planned actions
// ---------------------------------------------------------------------------

/// One value bound into the generated `UPDATE`. Only the columns an action
/// actually touched are written, so a concurrent edit elsewhere in the row
/// survives — the rule that kept this table usable while it had two writers.
enum Bind {
    Text(String),
    Int(i64),
}

/// The item fields the actions can change, plus which of them were changed.
#[derive(Default)]
pub(crate) struct ItemPatch {
    columns: Vec<(&'static str, Bind)>,
}

impl ItemPatch {
    pub(crate) fn set_text(&mut self, column: &'static str, value: impl Into<String>) {
        self.columns.retain(|(name, _)| *name != column);
        self.columns.push((column, Bind::Text(value.into())));
    }

    pub(crate) fn set_int(&mut self, column: &'static str, value: i64) {
        self.columns.retain(|(name, _)| *name != column);
        self.columns.push((column, Bind::Int(value)));
    }

    pub(crate) fn set_json(&mut self, column: &'static str, value: &Value) {
        self.set_text(column, value.to_string());
    }

    pub(crate) async fn write(self, state: &AppState, item_id: i64) -> Result<(), ApiError> {
        let mut assignments: Vec<String> =
            self.columns.iter().map(|(name, _)| format!("{name} = ?")).collect();
        assignments.push("updated_at = ?".into());

        // Column names come from the fixed list above, never from a request.
        let sql = format!("UPDATE todo_items SET {} WHERE id = ?", assignments.join(", "));
        // Bound to a local: the query borrows the rewritten string while the
        // binds are added one at a time.
        let sql = crate::db::sql(&sql, state.backend).into_owned();
        let mut query = sqlx::query(&sql);
        for (_, value) in self.columns {
            query = match value {
                Bind::Text(text) => query.bind(text),
                Bind::Int(number) => query.bind(number),
            };
        }
        query.bind(sql_now()).bind(item_id).execute(&state.any).await?;
        Ok(())
    }
}

#[derive(Deserialize)]
struct PlannedAction {
    #[serde(default)]
    action_id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    parameters: Map<String, Value>,
    #[serde(default)]
    confidence: Option<f64>,
    #[serde(default)]
    reasoning: Option<String>,
}

#[derive(Deserialize)]
struct ApplyActionsRequest {
    #[serde(default)]
    actions: Vec<PlannedAction>,
}

#[derive(Default)]
struct ApplyResult {
    applied: Vec<String>,
    skipped: Vec<String>,
    guidance: Vec<String>,
    exports: Vec<Value>,
}

fn as_str(params: &Map<String, Value>, key: &str) -> Option<String> {
    params.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Python's `isinstance(v, int)`, which is false for a float and — because
/// `bool` is an `int` there — true for a boolean. serde_json keeps them apart,
/// so a boolean would not match; nothing sends one.
fn as_int(params: &Map<String, Value>, key: &str) -> Option<i64> {
    params.get(key).and_then(|v| v.as_i64())
}

/// `datetime.fromisoformat` after swapping a trailing `Z`, then stored the way
/// SQLAlchemy stores it in a naive column.
///
/// The offset is **dropped, not applied**: writing an aware datetime into a
/// `DateTime` column keeps the wall clock and discards the tzinfo, so `09:00Z`
/// lands as `09:00`. Converting to UTC first would move every scheduled item by
/// the caller's offset relative to what Python does with the same request.
fn as_datetime(params: &Map<String, Value>, key: &str) -> Option<String> {
    let raw = as_str(params, key)?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    // Same parse as the item CRUD's `datetime_to_sql`, which is the point: this
    // one used to reject a space separator that pydantic accepts, so an action
    // planned with `"2026-08-06 09:00"` was silently skipped here and applied
    // there.
    Some(crate::wire::sql_string(crate::wire::parse_naive(raw)?))
}

pub(crate) fn now_isoformat() -> String {
    chrono::Utc::now().naive_utc().format("%Y-%m-%dT%H:%M:%S%.6f").to_string()
}

/// Apply the actions an agent planned, on the server, so the same rules run
/// whichever client asked.
///
/// Every action is independent: one that cannot be applied is recorded in
/// `skipped` with the reason and the rest still run. Nothing here fails the
/// request except a missing item.
async fn agent_apply(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(item_id): PathId<i64>,
    raw: axum::body::Bytes,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:write")?;
    assert_item_access(&state, &principal, item_id).await?;

    let request: ApplyActionsRequest = serde_json::from_slice(&raw).unwrap_or(ApplyActionsRequest {
        actions: Vec::new(),
    });

    let row: Option<ItemRow> =
        sqlx::query_as(&crate::db::sql(&format!("SELECT {ITEM_COLUMNS} FROM todo_items WHERE id = ?"), state.backend))
            .bind(item_id)
            .fetch_optional(&state.any)
            .await?;
    let Some(item) = row else {
        return Err(ApiError::not_found("Item not found"));
    };

    let mut result = ApplyResult::default();
    let mut patch = ItemPatch::default();
    let mut plan = Value::Array(json_array(item.plan_json.clone()));
    let mut metadata = json_object(item.metadata_json.clone());
    let mut completion = json_object(item.completion_json.clone());
    let mut title = item.title.clone();
    let mut description = item.description.clone();

    for action in &request.actions {
        let p = &action.parameters;
        let aid = action.action_id.as_str();

        // An action addressed at a different item is not applied here.
        if as_int(p, "item_id").is_some_and(|target| target != item_id) {
            result.skipped.push(format!("{aid}: wrong item_id"));
            continue;
        }

        match aid {
            "move_item_status" => match as_str(p, "status") {
                Some(status) if TODO_STATUSES.contains(&status.as_str()) => {
                    patch.set_text("status", &status);
                    result.applied.push(format!("Moved to {status}"));
                }
                _ => result.skipped.push("move_item_status: invalid status".into()),
            },

            "update_item" => {
                let mut changed = false;
                if let Some(new_title) = as_str(p, "title").filter(|t| !t.is_empty()) {
                    title = new_title.trim().to_string();
                    patch.set_text("title", &title);
                    changed = true;
                }
                if let Some(new_description) = p.get("description").and_then(Value::as_str) {
                    description = new_description.to_string();
                    patch.set_text("description", &description);
                    changed = true;
                }
                if let Some(priority) = as_int(p, "priority") {
                    patch.set_int("priority", priority);
                    changed = true;
                }
                if changed {
                    result.applied.push("Updated item".into());
                } else {
                    result.skipped.push("update_item: empty patch".into());
                }
            }

            "add_subtask" => match as_str(p, "step").filter(|s| !s.is_empty()) {
                Some(step) => {
                    let done = p.get("done").is_some_and(truthy);
                    if let Some(steps) = plan.as_array_mut() {
                        steps.push(json!({ "step": step, "done": done }));
                    }
                    patch.set_json("plan_json", &plan);
                    result.applied.push(format!("Added subtask: {step}"));
                }
                None => result.skipped.push("add_subtask: missing step".into()),
            },

            "break_down_task" => {
                let grocery = p.get("grocery_groups").and_then(Value::as_array);
                if let Some(groups) = grocery.filter(|g| !g.is_empty()) {
                    let rows: Vec<Value> = groups
                        .iter()
                        .filter_map(|group| {
                            let group = group.as_object()?;
                            let category = group
                                .get("category")
                                .and_then(Value::as_str)
                                .filter(|c| !c.is_empty())
                                .unwrap_or("Other");
                            let items: Vec<Value> = group
                                .get("items")
                                .and_then(Value::as_array)
                                .map(|items| items.iter().map(python_str).collect())
                                .unwrap_or_default();
                            Some(json!({ "category": category, "items": items, "done": false }))
                        })
                        .collect();
                    if rows.is_empty() {
                        result.skipped.push("break_down_task: empty grocery_groups".into());
                        continue;
                    }
                    let count = rows.len();
                    plan = Value::Array(rows);
                    patch.set_json("plan_json", &plan);
                    metadata.insert("plan_kind".into(), json!("grocery_list"));
                    patch.set_json("metadata_json", &Value::Object(metadata.clone()));
                    result.applied.push(format!("Grocery list: {count} groups"));
                } else {
                    let steps = p.get("steps").and_then(Value::as_array);
                    let Some(steps) = steps.filter(|s| !s.is_empty()) else {
                        result.skipped.push("break_down_task: no steps".into());
                        continue;
                    };
                    let rows: Vec<Value> = steps
                        .iter()
                        .map(|entry| match entry.as_object() {
                            Some(map) => {
                                let step = map
                                    .get("step")
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                                    .unwrap_or_else(|| entry.to_string());
                                json!({ "step": step, "done": map.get("done").is_some_and(truthy) })
                            }
                            None => json!({ "step": python_str(entry), "done": false }),
                        })
                        .collect();
                    let count = rows.len();
                    plan = Value::Array(rows);
                    patch.set_json("plan_json", &plan);
                    result.applied.push(format!("Plan: {count} steps"));
                }
            }

            "suggest_next_steps" => {
                if let Some(guidance) = as_str(p, "guidance").filter(|g| !g.is_empty()) {
                    result.guidance.push(guidance);
                }
                result.applied.push("Guidance received".into());
            }

            "ask_clarifying_questions" => {
                if let Some(questions) = p.get("questions").and_then(Value::as_array) {
                    result.guidance.extend(questions.iter().map(|q| {
                        python_str(q).as_str().unwrap_or_default().to_string()
                    }));
                }
                result.applied.push("Questions received".into());
            }

            "present_planning_form" => {
                let Some(form) = p.get("form").filter(|f| f.is_object()) else {
                    result.skipped.push("present_planning_form: invalid form".into());
                    continue;
                };
                let mut forms = match metadata.get("planning_forms") {
                    Some(Value::Array(existing)) => existing.clone(),
                    _ => Vec::new(),
                };
                forms.push(json!({ "spec": form, "status": "open", "answers": null }));
                metadata.insert("pending_form_index".into(), json!(forms.len() - 1));
                metadata.insert("planning_forms".into(), Value::Array(forms));
                patch.set_json("metadata_json", &Value::Object(metadata.clone()));
                result.applied.push("Planning form presented".into());
            }

            "export_markdown_checklist" => {
                let export_title =
                    as_str(p, "title").filter(|t| !t.is_empty()).unwrap_or_else(|| title.clone());
                let lines: Vec<String> = match p.get("lines").and_then(Value::as_array) {
                    Some(lines) => lines
                        .iter()
                        .map(|l| python_str(l).as_str().unwrap_or_default().to_string())
                        .collect(),
                    None => plan
                        .as_array()
                        .map(|steps| {
                            steps
                                .iter()
                                .map(|s| {
                                    s.get("step")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string()
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                };
                let body = lines
                    .iter()
                    .filter(|line| !line.trim().is_empty())
                    .map(|line| format!("- [ ] {line}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                let content = format!("# {export_title}\n\n{body}\n");
                let stem: String = export_title.chars().take(48).collect();
                let stem = stem.trim();
                let filename =
                    format!("{}.md", if stem.is_empty() { "checklist" } else { stem });
                result.exports.push(
                    json!({ "kind": "markdown", "filename": filename, "content": content }),
                );
                result.applied.push("Markdown checklist ready".into());
            }

            "export_ics_event" => {
                let summary =
                    as_str(p, "summary").filter(|s| !s.is_empty()).unwrap_or_else(|| title.clone());
                let start = as_str(p, "start").unwrap_or_default();
                if start.is_empty() {
                    result.skipped.push("export_ics_event: missing start".into());
                    continue;
                }
                let end = as_str(p, "end").filter(|e| !e.is_empty()).unwrap_or_else(|| start.clone());
                let ics_description = as_str(p, "description")
                    .filter(|d| !d.is_empty())
                    .unwrap_or_else(|| description.clone())
                    .replace('\n', "\\n");
                let ics = format!(
                    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Agent Platform//Todo Planner//EN\r\n\
                     BEGIN:VEVENT\r\n\
                     UID:todo-item-{item_id}@agent-platform\r\n\
                     SUMMARY:{summary}\r\n\
                     DTSTART:{start}\r\n\
                     DTEND:{end}\r\n\
                     DESCRIPTION:{ics_description}\r\n\
                     END:VEVENT\r\nEND:VCALENDAR\r\n"
                );
                let stem: String = summary.chars().take(48).collect();
                let stem = stem.trim();
                let filename = format!("{}.ics", if stem.is_empty() { "event" } else { stem });
                result
                    .exports
                    .push(json!({ "kind": "ics", "filename": filename, "content": ics }));
                result.applied.push("Calendar event ready".into());
            }

            "schedule_item" => match as_datetime(p, "scheduled_at") {
                Some(scheduled) => {
                    patch.set_text("scheduled_at", scheduled);
                    if let Some(horizon) = as_str(p, "time_horizon").filter(|h| !h.is_empty()) {
                        patch.set_text("time_horizon", horizon);
                    }
                    result.applied.push("Scheduled item".into());
                }
                None => result.skipped.push("schedule_item: invalid scheduled_at".into()),
            },

            "set_due_date" => match as_datetime(p, "due_at") {
                Some(due) => {
                    patch.set_text("due_at", due);
                    result.applied.push("Due date set".into());
                }
                None => result.skipped.push("set_due_date: invalid due_at".into()),
            },

            "log_completion" => {
                completion.insert("completed_at".into(), json!(now_isoformat()));
                if let Some(minutes) = as_int(p, "time_spent_minutes") {
                    completion.insert("time_spent_minutes".into(), json!(minutes));
                }
                for key in ["difficulty", "notes", "blockers"] {
                    if let Some(value) = as_str(p, key).filter(|v| !v.is_empty()) {
                        completion.insert(key.into(), json!(value));
                    }
                }
                patch.set_json("completion_json", &Value::Object(completion.clone()));
                patch.set_text("status", "done");
                result.applied.push("Completion logged".into());
            }

            "adjust_plan" => {
                if let Some(new_title) = as_str(p, "title").filter(|t| !t.is_empty()) {
                    title = new_title.trim().to_string();
                    patch.set_text("title", &title);
                }
                if let Some(new_description) = p.get("description").and_then(Value::as_str) {
                    description = new_description.to_string();
                    patch.set_text("description", &description);
                }
                if let Some(due) = as_datetime(p, "due_at") {
                    patch.set_text("due_at", due);
                }
                if let Some(scheduled) = as_datetime(p, "scheduled_at") {
                    patch.set_text("scheduled_at", scheduled);
                }
                if let Some(horizon) = as_str(p, "time_horizon").filter(|h| !h.is_empty()) {
                    patch.set_text("time_horizon", horizon);
                }
                if let Some(status) =
                    as_str(p, "status").filter(|s| TODO_STATUSES.contains(&s.as_str()))
                {
                    patch.set_text("status", status);
                }
                if let Some(priority) = as_int(p, "priority") {
                    patch.set_int("priority", priority);
                }
                result.applied.push("Plan adjusted".into());
            }

            "create_subtask_item" => {
                let parent_id = as_int(p, "parent_item_id").unwrap_or(item_id);
                let Some(subtask_title) = as_str(p, "title").filter(|t| !t.is_empty()) else {
                    result.skipped.push("create_subtask_item: missing title".into());
                    continue;
                };
                let parent: Option<(i64, Option<i64>, Option<String>)> = sqlx::query_as(&crate::db::sql(
                    "SELECT board_id, category_id, time_horizon FROM todo_items WHERE id = ?", state.backend)
                )
                .bind(parent_id)
                .fetch_optional(&state.any)
                .await?;
                let Some((board_id, category_id, parent_horizon)) = parent else {
                    result.skipped.push("create_subtask_item: parent not found".into());
                    continue;
                };
                let now = sql_now();
                sqlx::query(&crate::db::sql(
                    "INSERT INTO todo_items (board_id, category_id, title, description, status, \
                     priority, parent_item_id, due_at, scheduled_at, time_horizon, item_kind, \
                     created_at, updated_at) \
                     VALUES (?, ?, ?, ?, 'plan', 0, ?, ?, ?, ?, 'task', ?, ?)", state.backend)
                )
                .bind(board_id)
                .bind(category_id)
                .bind(&subtask_title)
                .bind(as_str(p, "description").unwrap_or_default())
                .bind(parent_id)
                .bind(as_datetime(p, "due_at"))
                .bind(as_datetime(p, "scheduled_at"))
                .bind(parent_horizon.filter(|h| !h.is_empty()).unwrap_or_else(|| "week".into()))
                .bind(&now)
                .bind(&now)
                .execute(&state.any)
                .await?;
                result.applied.push(format!("Created subtask: {subtask_title}"));
            }

            "propose_review" => {
                result.guidance.push(
                    as_str(p, "reason").filter(|r| !r.is_empty()).unwrap_or_else(|| "Review suggested".into()),
                );
                result.applied.push("Review proposed".into());
            }

            "store_user_profile" => {
                let domain = as_str(p, "domain").filter(|d| !d.is_empty());
                let data = p.get("data").and_then(Value::as_object);
                let (Some(domain), Some(data)) = (domain, data) else {
                    result.skipped.push("store_user_profile: invalid domain or data".into());
                    continue;
                };
                let project_id: Option<Option<i64>> =
                    sqlx::query_scalar(&crate::db::sql("SELECT project_id FROM todo_boards WHERE id = ?", state.backend))
                        .bind(item.board_id)
                        .fetch_optional(&state.any)
                        .await?;
                let Some(Some(project_id)) = project_id else {
                    result.skipped.push("store_user_profile: no project".into());
                    continue;
                };
                merge_domain_profile(&state, project_id, &domain, data).await?;
                result.applied.push(format!("Saved {domain} profile"));
            }

            "trigger_webhook" => {
                let Some(url) = as_str(p, "webhook_url").filter(|u| !u.is_empty()) else {
                    result.skipped.push("trigger_webhook: missing webhook_url".into());
                    continue;
                };
                match trigger_webhook(&state.http, &url, p.get("payload")).await {
                    Ok((status, ok)) => result
                        .applied
                        .push(format!("Webhook {status}{}", if ok { " OK" } else { " failed" })),
                    Err(message) => result.skipped.push(format!("trigger_webhook: {message}")),
                }
            }

            other => result.skipped.push(format!("Unknown action: {other}")),
        }
    }

    patch.write(&state, item_id).await?;

    let planned: Vec<Value> = request
        .actions
        .iter()
        .map(|a| {
            json!({
                "action_id": a.action_id,
                "name": a.name,
                "parameters": a.parameters,
                "confidence": a.confidence,
                "reasoning": a.reasoning,
            })
        })
        .collect();
    append_item_event(
        &state,
        item_id,
        "actions_applied",
        json!({
            "applied": result.applied,
            "skipped": result.skipped,
            "guidance": result.guidance,
            "export_count": result.exports.len(),
            "actions": planned,
        }),
    )
    .await?;

    Ok(Json(json!({
        "item": load_item(&state, item_id).await?,
        "applied": result.applied,
        "skipped": result.skipped,
        "guidance": result.guidance,
        "exports": result.exports,
    }))
    .into_response())
}

/// Python's `str(x)` for the list entries these actions stringify.
pub(crate) fn python_str(value: &Value) -> Value {
    match value {
        Value::String(_) => value.clone(),
        Value::Bool(true) => json!("True"),
        Value::Bool(false) => json!("False"),
        Value::Null => json!("None"),
        other => json!(other.to_string()),
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        Value::Null => false,
    }
}

/// `assistant.services.user_profile_service.merge_profile` — the one action here
/// that writes another domain's table, and also `assistant.rs`'s `PATCH
/// /profile/{domain}` handler; both need the merged profile back, so this
/// returns it rather than `()`. Empty and null values are dropped rather than
/// stored, so a partial patch cannot erase what is already known.
pub(crate) async fn merge_domain_profile(
    state: &AppState,
    project_id: i64,
    domain: &str,
    patch: &Map<String, Value>,
) -> Result<Map<String, Value>, ApiError> {
    let existing: Option<(i64, Option<String>)> = sqlx::query_as(&crate::db::sql(
        "SELECT id, profile_json FROM assistant_domain_profiles WHERE project_id = ? AND domain = ?", state.backend)
    )
    .bind(project_id)
    .bind(domain)
    .fetch_optional(&state.any)
    .await?;

    let (id, mut profile) = match existing {
        Some((id, raw)) => (Some(id), json_object(raw)),
        None => (None, Map::new()),
    };
    for (key, value) in patch {
        if !value.is_null() && value.as_str() != Some("") {
            profile.insert(key.clone(), value.clone());
        }
    }

    let now = sql_now();
    let body = Value::Object(profile.clone()).to_string();
    match id {
        Some(id) => {
            sqlx::query(&crate::db::sql(
                "UPDATE assistant_domain_profiles SET profile_json = ?, updated_at = ? WHERE id = ?", state.backend)
            )
            .bind(body)
            .bind(&now)
            .bind(id)
            .execute(&state.any)
            .await?;
        }
        None => {
            sqlx::query(&crate::db::sql(
                "INSERT INTO assistant_domain_profiles \
                 (project_id, domain, profile_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?)", state.backend)
            )
            .bind(project_id)
            .bind(domain)
            .bind(body)
            .bind(&now)
            .bind(&now)
            .execute(&state.any)
            .await?;
        }
    }
    Ok(profile)
}

/// POST a JSON payload to an external webhook (n8n, Zapier, …). The URL comes
/// from the plan, so the scheme is checked before anything is sent.
pub(crate) async fn trigger_webhook(
    http: &reqwest::Client,
    url: &str,
    payload: Option<&Value>,
) -> Result<(u16, bool), String> {
    let url = url.trim();
    let parsed = url::Url::parse(url).map_err(|_| "webhook_url must be an http(s) URL")?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host().is_none() {
        return Err("webhook_url must be an http(s) URL".into());
    }
    let body = match payload {
        Some(value @ Value::Object(_)) => value.clone(),
        _ => json!({}),
    };
    let response = http
        .post(url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    Ok((status.as_u16(), status.is_success()))
}

/// Record a user's answers to a planning form the agent presented.
///
/// The form lives in the item's `metadata_json` — the agent appends a `{spec,
/// status, answers}` entry and points `pending_form_index` at it; this fills the
/// answers in and clears that pointer.
async fn planning_form_submit(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(item_id): PathId<i64>,
    raw: axum::body::Bytes,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:write")?;
    assert_item_access(&state, &principal, item_id).await?;

    let body = serde_json::from_slice::<Value>(&raw)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();

    let mut errors = Vec::new();
    let form_index = match body.get("form_index") {
        None | Some(Value::Null) => {
            errors.push(ApiError::field_error("form_index", "missing", "Field required"));
            0
        }
        Some(Value::Number(n)) if n.is_i64() => {
            let value = n.as_i64().unwrap_or(-1);
            if value < 0 {
                errors.push(ApiError::field_error(
                    "form_index",
                    "greater_than_equal",
                    "Input should be greater than or equal to 0",
                ));
            }
            value
        }
        Some(_) => {
            errors.push(ApiError::field_error(
                "form_index",
                "int_type",
                "Input should be a valid integer",
            ));
            0
        }
    };
    let answers = match body.get("answers") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(map)) => map.clone(),
        Some(_) => {
            errors.push(ApiError::field_error(
                "answers",
                "dict_type",
                "Input should be a valid dictionary",
            ));
            Map::new()
        }
    };
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    let stored: Option<(Option<String>,)> =
        sqlx::query_as(&crate::db::sql("SELECT metadata_json FROM todo_items WHERE id = ?", state.backend))
            .bind(item_id)
            .fetch_optional(&state.any)
            .await?;
    let Some((metadata_json,)) = stored else {
        return Err(ApiError::not_found("Item not found"));
    };

    let mut metadata = json_object(metadata_json);
    let index = form_index as usize;
    let Some(Value::Array(mut forms)) = metadata.get("planning_forms").cloned() else {
        return Err(ApiError::bad_request("Invalid planning form index"));
    };
    if index >= forms.len() {
        return Err(ApiError::bad_request("Invalid planning form index"));
    }
    let Some(Value::Object(mut entry)) = forms.get(index).cloned() else {
        return Err(ApiError::bad_request("Invalid planning form entry"));
    };

    entry.insert("answers".into(), Value::Object(answers.clone()));
    entry.insert("status".into(), json!("submitted"));
    forms[index] = Value::Object(entry);
    metadata.insert("planning_forms".into(), Value::Array(forms));
    if metadata.get("pending_form_index").and_then(Value::as_i64) == Some(form_index) {
        metadata.remove("pending_form_index");
    }

    // One column, plus the timestamp — the rule that keeps this table safe while
    // Python still writes the agent routes beside it.
    sqlx::query(&crate::db::sql("UPDATE todo_items SET metadata_json = ?, updated_at = ? WHERE id = ?", state.backend))
        .bind(Value::Object(metadata).to_string())
        .bind(sql_now())
        .bind(item_id)
        .execute(&state.any)
        .await?;

    append_item_event(
        &state,
        item_id,
        "planning_form_submitted",
        json!({ "form_index": form_index, "answers": answers }),
    )
    .await?;

    Ok(Json(load_item(&state, item_id).await?).into_response())
}

async fn item_events(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(item_id): PathId<i64>,
    Query(q): Query<EventQuery>,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:read")?;
    assert_item_access(&state, &principal, item_id).await?;
    load_item(&state, item_id).await?;

    let rows: Vec<EventRow> = sqlx::query_as(&crate::db::sql(
        "SELECT id, item_id, event_type, content_json, created_at FROM todo_item_events \
         WHERE item_id = ? AND id > ? ORDER BY id ASC LIMIT ?", state.backend)
    )
    .bind(item_id)
    .bind(q.after_id)
    .bind(q.limit)
    .fetch_all(&state.any)
    .await?;

    let events: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            json!({
                "id": row.id,
                "item_id": row.item_id,
                "event_type": row.event_type,
                "content": json_object(Some(row.content_json)),
                "created_at": crate::wire::iso_from_sql(&row.created_at),
            })
        })
        .collect();
    Ok(Json(json!({ "events": events })).into_response())
}

// ---------------------------------------------------------------------------
// Spawning a process (`app/todos/services/process_spawn.py`)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SpawnProcessRequest {
    team_template_id: Option<i64>,
    #[serde(default)]
    goal: Option<String>,
    #[serde(default)]
    auto_approve: bool,
}

#[derive(FromRow)]
struct TemplateRow {
    id: i64,
    name: String,
    description: Option<String>,
    color: Option<String>,
    roster_json: String,
}

#[derive(Serialize)]
struct SpawnProcessResponse {
    process_id: i64,
    status: &'static str,
    item: ItemOut,
    auto_approve: bool,
    /// Verbatim from Python: this route deliberately schedules nothing, and the
    /// note is the only place a caller is told so.
    note: &'static str,
}

/// `json.dumps` escapes every non-ASCII character — `ensure_ascii=True` is the
/// default — where serde_json emits it raw. The rest of the compact form
/// already matches, so only string fragments are overridden.
/// `team_schema.build_process_team_snapshot`.
///
/// Two details are load-bearing, because `process.team_snapshot_json` is stored
/// and never re-derived: the payload is built from structs so the keys keep
/// pydantic's field-declaration order (a `serde_json::Map` would sort them),
/// and it is written through [`crate::dag_schema::python_json_compact`] so a
/// non-ASCII team name escapes the way `json.dumps` escapes it, with the tight
/// separators this column has always been stored with.
pub(crate) fn build_process_team_snapshot(
    team_template_id: i64,
    name: &str,
    description: Option<&str>,
    color: &str,
    roster: &TeamRoster,
) -> String {
    #[derive(Serialize)]
    struct Snapshot<'a> {
        team_template_id: i64,
        name: &'a str,
        description: Option<&'a str>,
        color: &'a str,
        roster: &'a TeamRoster,
    }

    let payload = Snapshot { team_template_id, name, description, color, roster };
    crate::dag_schema::python_json_compact(&payload, true)
}

/// The template as it is snapshotted onto a process: the same read-path colour
/// resolution the teams API uses, keyed by the template id.
fn snapshot_template(row: &TemplateRow) -> Result<String, ApiError> {
    let key = row.id.to_string();
    let color = resolved_team_color(row.color.as_deref(), Some(&key));
    let roster = with_default_accents(&parse_roster(&row.roster_json)?, Some(&color), &key);
    Ok(build_process_team_snapshot(
        row.id,
        &row.name,
        row.description.as_deref(),
        &color,
        &roster,
    ))
}

/// Create a `pending` process from an item and link the two.
///
/// Nothing is scheduled: no planner call, no DAG, no executor. The row sits at
/// `pending` until someone calls `POST /processes/{id}/sync`, which is what the
/// `note` in the response says.
async fn spawn_process(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(item_id): PathId<i64>,
    // Raw bytes, not `Option<Json<SpawnProcessRequest>>` — see
    // `create_board`'s comment.
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:write")?;
    assert_item_access(&state, &principal, item_id).await?;

    let req: Option<SpawnProcessRequest> =
        if body.is_empty() { None } else { Some(parse_body_typed(&body)?) };
    let team_template_id = match req.as_ref().and_then(|r| r.team_template_id) {
        None => {
            return Err(ApiError::validation(vec![ApiError::field_error(
                "team_template_id",
                "missing",
                "Field required",
            )]))
        }
        Some(id) if id < 1 => {
            return Err(ApiError::validation(vec![ApiError::field_error(
                "team_template_id",
                "greater_than_equal",
                "Input should be greater than or equal to 1",
            )]))
        }
        Some(id) => id,
    };
    let auto_approve = req.as_ref().is_some_and(|r| r.auto_approve);
    let requested_goal = req.and_then(|r| r.goal).unwrap_or_default();

    let item = load_item(&state, item_id).await?;

    // Python looks the template up with a bare `session.get` — no visibility
    // check, unlike every read in `teams.rs`. Kept as-is on purpose: this is a
    // parity port, and narrowing it here would 404 requests Python answers.
    let template: TemplateRow = sqlx::query_as(&crate::db::sql(
        "SELECT id, name, description, color, roster_json FROM teamtemplate WHERE id = ?", state.backend)
    )
    .bind(team_template_id)
    .fetch_optional(&state.any)
    .await?
    .ok_or_else(|| ApiError::not_found("Team template not found"))?;

    // The board is read for its project, not for access — that was checked
    // above. A board with no project spawns an unassigned process; a project
    // that has vanished is a 404 rather than a process pointing at nothing.
    let project_id: Option<Option<i64>> =
        sqlx::query_scalar(&crate::db::sql("SELECT project_id FROM todo_boards WHERE id = ?", state.backend))
            .bind(item.board_id)
            .fetch_optional(&state.any)
            .await?;
    let project_id = project_id.flatten();
    if let Some(project_id) = project_id {
        let exists: Option<i64> = sqlx::query_scalar(&crate::db::sql("SELECT id FROM project WHERE id = ?", state.backend))
            .bind(project_id)
            .fetch_optional(&state.any)
            .await?;
        if exists.is_none() {
            return Err(ApiError::not_found("Project not found"));
        }
    }

    let team_snapshot_json = snapshot_template(&template)?;
    // An empty goal falls back to the item itself, title and body separated by a
    // blank line, then trimmed — an item with no description must not spawn a
    // goal with two trailing newlines.
    let goal = match requested_goal.trim() {
        "" => format!("{}\n\n{}", item.title, item.description).trim().to_string(),
        explicit => explicit.to_string(),
    };

    // Column for column with SQLModel's `Process(...)` defaults. Left out, and
    // therefore NULL: `dag_json`, `failure_reason`, `client_id`, `token_id`,
    // `model_build_job_id`. `token_id` in particular is *not* set here — this
    // path never calls `record_api_token_usage`, so a todo-spawned process is
    // outside the token-counter two-writer hazard.
    let status = "pending";
    let now = sql_now();
    let process_id: i64 = sqlx::query_scalar(&crate::db::sql(
        "INSERT INTO process \
         (goal, status, total_tokens, total_cost, tool_invocations_used, team_template_id, \
          team_snapshot_json, project_id, created_at, updated_at) \
         VALUES (?, ?, 0, 0.0, 0, ?, ?, ?, ?, ?) RETURNING CAST(id AS BIGINT)", state.backend)
    )
    .bind(&goal)
    .bind(status)
    .bind(team_template_id)
    .bind(&team_snapshot_json)
    .bind(project_id)
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.any)
    .await?;

    set_item_column(&state, item_id, "linked_process_id", process_id).await?;
    set_item_column(&state, item_id, "updated_at", sql_now()).await?;

    append_item_event(
        &state,
        item_id,
        "process_spawned",
        json!({
            "process_id": process_id,
            "team_template_id": team_template_id,
            "goal": goal,
            "auto_approve": auto_approve,
        }),
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(SpawnProcessResponse {
            process_id,
            status,
            item: load_item(&state, item_id).await?,
            auto_approve,
            note: "Process created. Start planning via POST /api/v1/processes/{id}/sync \
                   or the process UI.",
        }),
    )
        .into_response())
}

// `pub(crate)`: `assistant.rs`'s turn generation resolves a profile by slug,
// the one lookup this file does not already have.
#[derive(FromRow, Clone)]
pub(crate) struct ProfileRow {
    pub(crate) id: i64,
    pub(crate) slug: String,
    pub(crate) name: String,
    pub(crate) requirement_type: String,
    pub(crate) system_prompt: String,
    pub(crate) default_model: Option<String>,
    pub(crate) action_set_id: Option<i64>,
    pub(crate) skill_paths_json: Option<String>,
}

pub const PROFILE_COLUMNS: &str = "CAST(id AS BIGINT) AS id, slug, name, requirement_type, \
     system_prompt, default_model, CAST(action_set_id AS BIGINT) AS action_set_id, \
     skill_paths_json";

async fn planner_profiles(
    State(state): State<Arc<AppState>>,
    principal: Principal,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:read")?;
    let rows: Vec<ProfileRow> = sqlx::query_as(&crate::db::sql(&format!(
        "SELECT {PROFILE_COLUMNS} FROM planner_agent_profiles ORDER BY id ASC"
    ), state.backend))
    .fetch_all(&state.any)
    .await?;

    let profiles: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            json!({
                "id": row.id,
                "slug": row.slug,
                "name": row.name,
                "requirement_type": row.requirement_type,
                "system_prompt": row.system_prompt,
                "default_model": row.default_model,
                "action_set_id": row.action_set_id,
                "skill_paths": json_array(row.skill_paths_json),
            })
        })
        .collect();
    Ok(Json(json!({ "profiles": profiles })).into_response())
}

// ---------------------------------------------------------------------------
// Agent chat
// ---------------------------------------------------------------------------

/// `resolve_profile_for_item` (`services/board_service.py`): the item's own
/// profile, else its category's, else the lowest-id profile on the board.
///
/// The first two branches are terminal: an `assigned_profile_id` pointing at a
/// row that no longer exists resolves to *no profile*, not to the fallback,
/// because Python's `session.get` returns `None` and returns it.
async fn resolve_profile_for_item(
    state: &AppState,
    item: &ItemRow,
) -> Result<Option<ProfileRow>, ApiError> {
    // `if item.assigned_profile_id:` — 0 is falsy there, so it is not an id here.
    if let Some(profile_id) = item.assigned_profile_id.filter(|id| *id != 0) {
        return load_profile(state, profile_id).await;
    }
    if let Some(category_id) = item.category_id.filter(|id| *id != 0) {
        let category_profile: Option<Option<i64>> =
            sqlx::query_scalar(&crate::db::sql("SELECT planner_profile_id FROM todo_categories WHERE id = ?", state.backend))
                .bind(category_id)
                .fetch_optional(&state.any)
                .await?;
        if let Some(profile_id) = category_profile.flatten().filter(|id| *id != 0) {
            return load_profile(state, profile_id).await;
        }
    }
    Ok(sqlx::query_as(&crate::db::sql(&format!(
        "SELECT {PROFILE_COLUMNS} FROM planner_agent_profiles ORDER BY id ASC LIMIT 1"
    ), state.backend))
    .fetch_optional(&state.any)
    .await?)
}

async fn load_profile(state: &AppState, profile_id: i64) -> Result<Option<ProfileRow>, ApiError> {
    Ok(sqlx::query_as(&crate::db::sql(&format!(
        "SELECT {PROFILE_COLUMNS} FROM planner_agent_profiles WHERE id = ?"
    ), state.backend))
    .bind(profile_id)
    .fetch_optional(&state.any)
    .await?)
}

/// `agent_bridge.build_item_context`, rendered straight to the string the prompt
/// carries. Python interpolates the dict itself (`f"…{context}"`), so what the
/// model sees is `str(dict)` — key order included. Building a `Value` here would
/// sort the keys (serde_json's map is a `BTreeMap`) and quote them the JSON way,
/// which is a different prompt.
async fn build_item_context(
    state: &AppState,
    item: &ItemRow,
    profile: Option<&ProfileRow>,
) -> Result<String, ApiError> {
    let (board, category) = item_context_rows(state, item).await?;

    let item_repr = py_dict(&[
        ("id", py_repr(&json!(item.id))),
        ("title", py_repr(&json!(item.title))),
        ("description", py_repr(&json!(item.description))),
        ("status", py_repr(&json!(item.status))),
        ("priority", py_repr(&json!(item.priority))),
        ("tags", py_repr(&Value::Array(json_array(item.tags_json.clone())))),
        ("plan", py_repr(&Value::Array(json_array(item.plan_json.clone())))),
        ("metadata", py_repr(&Value::Object(json_object(item.metadata_json.clone())))),
    ]);
    let board_repr = py_dict(&[
        // A missing board keeps the item's own `board_id`, like Python's
        // `board.id if board else item.board_id`.
        ("id", py_repr(&json!(board.as_ref().map_or(item.board_id, |b| b.0)))),
        ("name", py_repr(&json!(board.as_ref().map(|b| b.1.clone())))),
        ("default_model", py_repr(&json!(board.as_ref().and_then(|b| b.2.clone())))),
    ]);
    let category_repr = match &category {
        Some((id, name, planner_profile_id)) => py_dict(&[
            ("id", py_repr(&json!(id))),
            ("name", py_repr(&json!(name))),
            ("planner_profile_id", py_repr(&json!(planner_profile_id))),
        ]),
        None => "None".to_string(),
    };
    let profile_repr = match profile {
        Some(profile) => py_dict(&[
            ("slug", py_repr(&json!(profile.slug))),
            ("name", py_repr(&json!(profile.name))),
            ("requirement_type", py_repr(&json!(profile.requirement_type))),
        ]),
        None => "None".to_string(),
    };

    Ok(py_dict(&[
        ("item", item_repr),
        ("board", board_repr),
        ("category", category_repr),
        ("planner_profile", profile_repr),
    ]))
}

type BoardBrief = Option<(i64, String, Option<String>)>;
type CategoryBrief = Option<(i64, String, Option<i64>)>;

/// The two lookups both renderings of the item context need.
async fn item_context_rows(
    state: &AppState,
    item: &ItemRow,
) -> Result<(BoardBrief, CategoryBrief), ApiError> {
    let board: BoardBrief =
        sqlx::query_as(&crate::db::sql("SELECT id, name, default_model FROM todo_boards WHERE id = ?", state.backend))
            .bind(item.board_id)
            .fetch_optional(&state.any)
            .await?;
    let category: CategoryBrief = match item.category_id {
        Some(category_id) => {
            sqlx::query_as(&crate::db::sql("SELECT id, name, planner_profile_id FROM todo_categories WHERE id = ?", state.backend))
                .bind(category_id)
                .fetch_optional(&state.any)
                .await?
        }
        None => None,
    };
    Ok((board, category))
}

/// The same context as [`build_item_context`], as the JSON `decide_actions`
/// actually serialises.
///
/// `agent/chat` interpolates the dict into an f-string and so needs Python's
/// `str(dict)`; `action_orchestrator.engine.build_user_message` calls
/// `json.dumps(…, indent=2)` on it instead. Same fields, different renderer —
/// which is why only the two queries above are shared.
async fn build_item_context_json(
    state: &AppState,
    item: &ItemRow,
    profile: &ProfileRow,
) -> Result<Map<String, Value>, ApiError> {
    let (board, category) = item_context_rows(state, item).await?;
    let context = json!({
        "item": {
            "id": item.id,
            "title": item.title,
            "description": item.description,
            "status": item.status,
            "priority": item.priority,
            "tags": json_array(item.tags_json.clone()),
            "plan": json_array(item.plan_json.clone()),
            "metadata": json_object(item.metadata_json.clone()),
        },
        "board": {
            // A missing board keeps the item's own `board_id`.
            "id": board.as_ref().map_or(item.board_id, |b| b.0),
            "name": board.as_ref().map(|b| b.1.clone()),
            "default_model": board.as_ref().and_then(|b| b.2.clone()),
        },
        "category": category.as_ref().map(|(id, name, planner_profile_id)| json!({
            "id": id,
            "name": name,
            "planner_profile_id": planner_profile_id,
        })),
        "planner_profile": {
            "slug": profile.slug,
            "name": profile.name,
            "requirement_type": profile.requirement_type,
        },
    });
    Ok(context.as_object().cloned().unwrap_or_default())
}

/// `{'k': v, …}` — Python's dict repr, with the entries in the order given.
fn py_dict(entries: &[(&str, String)]) -> String {
    let rendered: Vec<String> =
        entries.iter().map(|(key, value)| format!("{}: {value}", py_str(key))).collect();
    format!("{{{}}}", rendered.join(", "))
}

/// Python's `repr` for a JSON value: `None`/`True`/`False`, single-quoted
/// strings, `, ` and `: ` separators, `[]`/`{}` for empties.
///
/// Nested objects come from a stored JSON column, so their keys are ordered by
/// serde_json's `BTreeMap` (alphabetical) where Python keeps the order they were
/// written in. Only `metadata`, `plan` and `tags` can carry one.
pub(crate) fn py_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => py_str(s),
        Value::Array(items) => {
            let rendered: Vec<String> = items.iter().map(py_repr).collect();
            format!("[{}]", rendered.join(", "))
        }
        Value::Object(map) => {
            let entries: Vec<(&str, String)> =
                map.iter().map(|(key, value)| (key.as_str(), py_repr(value))).collect();
            py_dict(&entries)
        }
    }
}

/// Python's `repr` for a `str`: single quotes, unless the string contains a `'`
/// and no `"` — then double quotes, so the apostrophe needs no backslash.
///
/// ponytail: control characters below U+00A0 escape as `\xNN`; Python also
/// escapes every other non-printable code point (zero-width spaces, unassigned
/// planes) and that needs a Unicode category table. A todo title with one in it
/// would render unescaped here.
fn py_str(value: &str) -> String {
    let quote = if value.contains('\'') && !value.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(value.len() + 2);
    out.push(quote);
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || (0x7f..0xa0).contains(&(c as u32)) => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `agent_bridge.resolve_model`: an override wins, then the profile's default,
/// then the board's, then the hard-coded fallback. Sanitising the override can
/// blank it, and Python keeps the raw text in that case rather than falling
/// through — the user asked for that model by name.
async fn resolve_model(
    state: &AppState,
    item: &ItemRow,
    profile: Option<&ProfileRow>,
    override_model: Option<&str>,
) -> Result<String, ApiError> {
    if let Some(raw) = override_model.map(str::trim).filter(|m| !m.is_empty()) {
        return Ok(sanitize_llm_model_alias(raw).unwrap_or_else(|| raw.to_string()));
    }
    if let Some(model) = profile.and_then(|p| p.default_model.clone()).filter(|m| !m.is_empty()) {
        return Ok(model);
    }
    let board_model: Option<Option<String>> =
        sqlx::query_scalar(&crate::db::sql("SELECT default_model FROM todo_boards WHERE id = ?", state.backend))
            .bind(item.board_id)
            .fetch_optional(&state.any)
            .await?;
    match board_model.flatten().filter(|m| !m.is_empty()) {
        Some(model) => Ok(model),
        None => Ok("gemma4:31b-cloud".to_string()),
    }
}

/// `AgentChatRequest`: `message: str`, `model: str | None`, and a
/// `list[dict[str, str]]` of prior turns.
fn parse_chat_request(raw: &[u8]) -> Result<(String, Option<String>, Vec<Value>), ApiError> {
    let body = serde_json::from_slice::<Value>(raw)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let mut errors = Vec::new();

    // Absent is `missing`; present-but-wrong-type is a type error, which is the
    // difference pydantic draws.
    let message = match body.get("message") {
        None => {
            errors.push(ApiError::field_error("message", "missing", "Field required"));
            String::new()
        }
        Some(Value::String(text)) => text.clone(),
        Some(_) => {
            errors.push(ApiError::field_error(
                "message",
                "string_type",
                "Input should be a valid string",
            ));
            String::new()
        }
    };
    let model = match body.get("model") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(_) => {
            errors.push(ApiError::field_error(
                "model",
                "string_type",
                "Input should be a valid string",
            ));
            None
        }
    };
    let history = match body.get("history") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(turns)) => {
            for (i, turn) in turns.iter().enumerate() {
                let index = i.to_string();
                match turn.as_object() {
                    None => errors.push(ApiError::field_error_at(
                        &["history", &index],
                        "dict_type",
                        "Input should be a valid dictionary",
                    )),
                    Some(fields) => {
                        for (key, value) in fields {
                            if !value.is_string() {
                                errors.push(ApiError::field_error_at(
                                    &["history", &index, key],
                                    "string_type",
                                    "Input should be a valid string",
                                ));
                            }
                        }
                    }
                }
            }
            turns.clone()
        }
        Some(_) => {
            errors.push(ApiError::field_error(
                "history",
                "list_type",
                "Input should be a valid list",
            ));
            Vec::new()
        }
    };

    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }
    Ok((message, model, history))
}

/// Talk to the planner agent about one item.
///
/// The prompt is the contract: two fixed lines, then the profile's system
/// prompt, then the item context as Python's `str(dict)` — see `py_repr`. The
/// event is appended only after a successful call, so a failed turn leaves no
/// trace on the item, exactly like Python.
async fn agent_chat(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(item_id): PathId<i64>,
    raw: axum::body::Bytes,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:write")?;
    assert_item_access(&state, &principal, item_id).await?;
    let (message, model_override, history) = parse_chat_request(&raw)?;

    let row: Option<ItemRow> =
        sqlx::query_as(&crate::db::sql(&format!("SELECT {ITEM_COLUMNS} FROM todo_items WHERE id = ?"), state.backend))
            .bind(item_id)
            .fetch_optional(&state.any)
            .await?;
    let Some(item) = row else {
        return Err(ApiError::not_found("Item not found"));
    };

    let profile = resolve_profile_for_item(&state, &item).await?;
    let context = build_item_context(&state, &item, profile.as_ref()).await?;
    let llm_model = resolve_model(&state, &item, profile.as_ref(), model_override.as_deref()).await?;

    let mut system = String::from(
        "You are a helpful planning assistant for a personal todo board.\n\n\
         Guide the user on what to do next. Be concise and actionable.",
    );
    if let Some(profile) = &profile {
        // Appended even when blank, which is what leaves the doubled separator
        // Python's `"\n\n".join` produces for a profile with no prompt.
        system.push_str("\n\n");
        system.push_str(&profile.system_prompt);
    }
    system.push_str(&format!("\n\nCurrent task context:\n{context}"));

    let mut messages = vec![json!({ "role": "system", "content": system })];
    for turn in &history {
        let role = turn.get("role").and_then(Value::as_str).unwrap_or("user");
        let content = turn.get("content").and_then(Value::as_str).unwrap_or_default();
        if !content.is_empty() {
            messages.push(json!({ "role": role, "content": content }));
        }
    }
    messages.push(json!({ "role": "user", "content": message }));

    // The call below is in-process and needs no credential, but Python's client
    // does, and a user who never set the key sees this 503 rather than a reply.
    // Dropping the check would change the status on that machine.
    if state.master_key.is_none() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "AGENT_PLATFORM_MASTER_KEY is not set.",
        ));
    }

    let (fitted, _) = crate::context_budget::fit_chat_messages_for_request(messages);
    let mut payload = Map::new();
    payload.insert("messages".into(), Value::Array(fitted));
    payload.insert("max_tokens".into(), json!(crate::context_budget::max_output_tokens_default()));
    // No `model` at all when sanitising blanked it: the proxy's own default is
    // better than a slug it cannot resolve.
    if let Some(model) = sanitize_llm_model_alias(&llm_model) {
        payload.insert("model".into(), json!(model));
    }

    // Python POSTs this to its own `/v1/chat/completions` over loopback and maps
    // every non-200 to a 502 carrying the proxy's status. `complete_internal` is
    // that route's code without the socket, and its error already carries the
    // status the route would have answered with — so the mapping is the same one
    // line. Python's `httpx.RequestError` branch guarded the loopback hop, which
    // no longer exists: a vendor transport failure is a 502 from the proxy
    // either way, and reads as `LLM proxy returned HTTP 502` on both servers.
    let data = crate::llm::complete_internal(&state, payload).await.map_err(|e| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            format!("LLM proxy returned HTTP {}", e.status.as_u16()),
        )
    })?;

    let content = data
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let profile_slug = profile.as_ref().map(|p| p.slug.clone());
    append_item_event(
        &state,
        item_id,
        "agent_chat",
        // `model` is the resolved name, not the sanitized one — it is what the
        // user picked, and the reason a reply came back on a different model.
        json!({ "message": message, "model": llm_model, "profile_slug": profile_slug }),
    )
    .await?;

    Ok(Json(json!({
        "content": content,
        "model": llm_model,
        "profile_slug": profile_slug,
    }))
    .into_response())
}

// ---------------------------------------------------------------------------
// Agent step
// ---------------------------------------------------------------------------

/// `AgentStepRequest`. Every field has a default, so a body that is merely
/// empty is still valid JSON-wise — the only failures are shape ones.
struct StepRequest {
    goal: String,
    model: Option<String>,
    context: Map<String, Value>,
    document_paths: Vec<String>,
}

const DEFAULT_STEP_GOAL: &str = "What should I do next for this task?";

impl StepRequest {
}

/// FastAPI validates the body before the handler runs, so every error here
/// comes out ahead of the scope and access checks.
fn parse_step_request(raw: &[u8]) -> Result<StepRequest, ApiError> {
    if raw.is_empty() {
        return Err(ApiError::validation(vec![
            json!({"type": "missing", "loc": ["body"], "msg": "Field required"}),
        ]));
    }
    let body = match serde_json::from_slice::<Value>(raw) {
        Ok(Value::Object(body)) => body,
        Ok(_) => {
            return Err(ApiError::validation(vec![json!({
                "type": "model_attributes_type",
                "loc": ["body"],
                "msg": "Input should be a valid dictionary or object to extract fields from",
            })]))
        }
        Err(_) => {
            return Err(ApiError::validation(vec![json!({
                "type": "json_invalid",
                "loc": ["body", 0],
                "msg": "JSON decode error",
            })]))
        }
    };

    let mut errors = Vec::new();
    // A default does not make a field nullable: an explicit `null` is a type
    // error for everything except `model`, which is declared `str | None`.
    let goal = match body.get("goal") {
        None => DEFAULT_STEP_GOAL.to_string(),
        Some(Value::String(goal)) => goal.clone(),
        Some(_) => {
            errors.push(ApiError::field_error(
                "goal",
                "string_type",
                "Input should be a valid string",
            ));
            String::new()
        }
    };
    let model = match body.get("model") {
        None | Some(Value::Null) => None,
        Some(Value::String(model)) => Some(model.clone()),
        Some(_) => {
            errors.push(ApiError::field_error(
                "model",
                "string_type",
                "Input should be a valid string",
            ));
            None
        }
    };
    let context = match body.get("context") {
        None => Map::new(),
        Some(Value::Object(context)) => context.clone(),
        Some(_) => {
            errors.push(ApiError::field_error(
                "context",
                "dict_type",
                "Input should be a valid dictionary",
            ));
            Map::new()
        }
    };
    let document_paths = match body.get("document_paths") {
        None => Vec::new(),
        Some(Value::Array(paths)) => paths
            .iter()
            .enumerate()
            .filter_map(|(i, path)| match path {
                Value::String(path) => Some(path.clone()),
                _ => {
                    errors.push(ApiError::field_error_at(
                        &["document_paths", &i.to_string()],
                        "string_type",
                        "Input should be a valid string",
                    ));
                    None
                }
            })
            .collect(),
        Some(_) => {
            errors.push(ApiError::field_error(
                "document_paths",
                "list_type",
                "Input should be a valid list",
            ));
            Vec::new()
        }
    };

    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }
    Ok(StepRequest { goal, model, context, document_paths })
}

/// `agent_bridge.merge_workspace_documents`: attach an excerpt of each named
/// document to the planner's context.
///
/// Every failure is per-document and non-fatal — an unreadable path becomes a
/// `{"path", "error"}` entry so the model can say so, rather than failing the
/// step. A board with no project reads nothing at all.
async fn merge_workspace_documents(state: &AppState, item: &ItemRow, context: &mut Map<String, Value>) {
    // `context.get("document_paths")`, falling back to a single
    // `document_path` **only when the list key is absent or null** — a present
    // non-list makes the function return without reading anything.
    let paths: Vec<String> = match context.get("document_paths") {
        Some(Value::Array(items)) => {
            items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
        }
        Some(other) if !other.is_null() => return,
        _ => match context.get("document_path").and_then(Value::as_str) {
            Some(one) if !one.is_empty() => vec![one.to_string()],
            _ => return,
        },
    };
    if paths.is_empty() {
        return;
    }

    let project_id: Option<Option<i64>> =
        sqlx::query_scalar(&crate::db::sql("SELECT project_id FROM todo_boards WHERE id = ?", state.backend))
            .bind(item.board_id)
            .fetch_optional(&state.any)
            .await
            .ok()
            .flatten();
    let Some(project_id) = project_id.flatten().filter(|id| *id != 0) else { return };

    let mut docs: Vec<Value> = Vec::new();
    for raw in paths {
        let rel = raw.trim();
        if rel.is_empty() {
            continue;
        }
        match crate::documents::read_for_llm(project_id, rel) {
            Ok(payload) => {
                let content = payload.get("content").and_then(Value::as_str).unwrap_or_default();
                // `len(content) > 8000` — characters, and the marker replaces
                // nothing, it is appended to the first 8000.
                let excerpt = if content.chars().count() > 8000 {
                    let head: String = content.chars().take(8000).collect();
                    format!("{head}

_(truncated)_")
                } else {
                    content.to_string()
                };
                docs.push(json!({
                    "path": payload.get("path").cloned().unwrap_or_else(|| Value::from(rel)),
                    "content_kind": payload.get("content_kind").cloned().unwrap_or(Value::Null),
                    "excerpt": excerpt,
                }));
            }
            Err(e) => docs.push(json!({ "path": rel, "error": e.code() })),
        }
    }

    if !docs.is_empty() {
        context.insert("workspace_documents".into(), Value::Array(docs));
    }
}

#[derive(Serialize)]
struct AgentStepResponse {
    thought: Option<String>,
    actions: Vec<crate::action_orchestrator::PlannedAction>,
    profile_slug: String,
    action_set_id: i64,
}

/// Ask the planner agent which actions to propose for one item.
///
/// Nothing is executed and nothing on the item changes: the proposals come back
/// for the user to accept through `agent/apply`, and the only write is one
/// appended `agent_step` event.
///
/// A step that names workspace documents reads them here, through
/// [`merge_workspace_documents`]. That single call is what kept this route
/// reaching Python: the read goes through the sandbox guard and, for a PDF,
/// through text extraction, and neither had a Rust side. Both do now
/// ([`crate::documents`]), so the handover is gone.
async fn agent_step(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(item_id): PathId<i64>,
    raw: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let req = parse_step_request(&raw)?;

    require_scope(&principal, "todos:write")?;
    assert_item_access(&state, &principal, item_id).await?;

    let row: Option<ItemRow> =
        sqlx::query_as(&crate::db::sql(&format!("SELECT {ITEM_COLUMNS} FROM todo_items WHERE id = ?"), state.backend))
            .bind(item_id)
            .fetch_optional(&state.any)
            .await?;
    let Some(item) = row else {
        return Err(ApiError::not_found("Item not found"));
    };

    // `if not profile or not profile.action_set_id` — one message for both, and
    // a stored `0` is falsy there just like a missing id.
    let missing = || ApiError::bad_request("No planner profile or action set configured");
    let Some(profile) = resolve_profile_for_item(&state, &item).await? else {
        return Err(missing());
    };
    let Some(action_set_id) = profile.action_set_id.filter(|id| *id != 0) else {
        return Err(missing());
    };

    let actions = crate::action_orchestrator::list_actions(&state, action_set_id).await?;
    if actions.is_empty() {
        return Err(ApiError::bad_request("Action set has no actions"));
    }

    let mut context = build_item_context_json(&state, &item, &profile).await?;
    // `context.update(extra_context)`: the caller's keys win over the item's.
    context.extend(req.context);
    // `if req.document_paths: ctx["document_paths"] = list(...)` — the request
    // field overwrites whatever the caller's own `context` put under that key,
    // and an empty list leaves it alone.
    if !req.document_paths.is_empty() {
        context.insert("document_paths".into(), Value::from(req.document_paths.clone()));
    }
    merge_workspace_documents(&state, &item, &mut context).await;
    if !profile.system_prompt.is_empty() {
        context.insert("planner_system_prompt".into(), json!(profile.system_prompt));
    }

    let llm_model = resolve_model(&state, &item, Some(&profile), req.model.as_deref()).await?;
    // Infallible by construction: an LLM failure comes back as an empty action
    // list and a `thought` that explains it, with a 200, exactly as Python's
    // blanket `except Exception` produces.
    let (planned, thought, _usage) = crate::action_orchestrator::decide_actions(
        &state,
        &req.goal,
        &context,
        &actions,
        // `agent/step` passes no history — the prompt's "previous actions"
        // block belongs to the session routes.
        &[],
        &llm_model,
    )
    .await;

    append_item_event(
        &state,
        item_id,
        "agent_step",
        json!({
            "goal": &req.goal,
            "thought": &thought,
            "actions": &planned,
            "profile_slug": &profile.slug,
        }),
    )
    .await?;

    Ok(Json(AgentStepResponse {
        thought,
        actions: planned,
        profile_slug: profile.slug,
        action_set_id,
    })
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_messages_carry_pythons_tuple_syntax() {
        let mut errors = Vec::new();
        check_enum(&mut errors, "status", Some("nope"), &TODO_STATUSES);
        assert_eq!(
            errors[0]["msg"],
            "Value error, status must be one of ('plan', 'backlog', 'in_progress', 'review', 'done')"
        );
    }

    // `datetime_to_sql` moved to `wire.rs` and is covered by its tests: the old
    // one here asserted a shape that dropped `Z` but kept `+02:00`, which is what
    // the offset bug was.

    /// Both expectations are the literal output of
    /// `python -c "from team_schema import …; print(repr(build_process_team_snapshot(…)))"`
    /// run against this repo's `app/`, pasted verbatim.
    ///
    /// This string is stored on the process and never recomputed, so every way a
    /// port can silently drift shows up here: sorted keys instead of pydantic's
    /// declaration order, an accent emitted raw where `json.dumps` writes
    /// `\u00e9`, an emoji as one escape instead of a surrogate pair, or a space
    /// after a separator.
    #[test]
    fn team_snapshot_matches_pythons_json_dumps() {
        // No stored colour, so the team colour and every missing accent come
        // from the stable palette keyed by the template id.
        let stored: TeamRoster = serde_json::from_str(
            r##"{"roles":[
                {"id":"lead","name":"Planner"},
                {"id":"b","name":"Résumé bot","description":"Writes","parent_id":"lead"},
                {"id":"c","name":"C","accent_color":"  #abcdef  "}
            ]}"##,
        )
        .unwrap();
        let color = resolved_team_color(None, Some("1"));
        let roster = with_default_accents(&stored, Some(&color), "1");
        assert_eq!(
            build_process_team_snapshot(1, "Research pod", None, &color, &roster),
            r##"{"team_template_id":1,"name":"Research pod","description":null,"color":"#16a34a","roster":{"roles":[{"id":"lead","name":"Planner","description":"","modality":"text","parent_id":null,"accent_color":"#16a34a"},{"id":"b","name":"R\u00e9sum\u00e9 bot","description":"Writes","modality":"text","parent_id":"lead","accent_color":"#9333ea"},{"id":"c","name":"C","description":"","modality":"text","parent_id":null,"accent_color":"#abcdef"}]}}"##
        );

        let solo: TeamRoster =
            serde_json::from_str(r##"{"roles":[{"id":"a","name":"A","accent_color":"#111111"}]}"##)
                .unwrap();
        assert_eq!(
            build_process_team_snapshot(7, "Ops 😀", Some("tab\there"), "#abcdef", &solo),
            r##"{"team_template_id":7,"name":"Ops \ud83d\ude00","description":"tab\there","color":"#abcdef","roster":{"roles":[{"id":"a","name":"A","description":"","modality":"text","parent_id":null,"accent_color":"#111111"}]}}"##
        );
    }

    /// Every expectation below is the output of `str(<the same dict>)` run in
    /// this repo's Python, pasted verbatim — the point is the prompt the model
    /// reads, so a plausible-looking repr is not evidence.
    #[test]
    fn item_context_renders_as_pythons_dict_repr() {
        let rendered = py_dict(&[(
            "item",
            py_dict(&[
                ("id", py_repr(&json!(7))),
                ("title", py_repr(&json!("Ben's plan"))),
                ("description", py_repr(&Value::Null)),
                ("status", py_repr(&json!("todo"))),
                ("priority", py_repr(&json!(2))),
                ("tags", py_repr(&json!(["a", "b"]))),
                ("plan", py_repr(&json!([]))),
                ("metadata", py_repr(&json!({}))),
            ]),
        )]);
        assert_eq!(
            rendered,
            r#"{'item': {'id': 7, 'title': "Ben's plan", 'description': None, 'status': 'todo', 'priority': 2, 'tags': ['a', 'b'], 'plan': [], 'metadata': {}}}"#
        );
    }

    #[test]
    fn py_repr_quotes_the_way_python_does() {
        // Keys are alphabetical here, so serde's `BTreeMap` order happens to be
        // Python's insertion order too — the one case where the documented
        // divergence cannot hide a difference.
        assert_eq!(
            py_repr(&json!({"a": true, "b": false, "c": null, "d": 1.5, "e": [{"x": "y"}]})),
            r#"{'a': True, 'b': False, 'c': None, 'd': 1.5, 'e': [{'x': 'y'}]}"#
        );
        // A `"` alone keeps single quotes; a `'` alone flips to double; both
        // together keep single and backslash the apostrophe.
        assert_eq!(py_repr(&json!("he said \"hi\"")), r#"'he said "hi"'"#);
        assert_eq!(py_repr(&json!("Ben's plan")), r#""Ben's plan""#);
        assert_eq!(py_repr(&json!("both ' and \"")), r#"'both \' and "'"#);
        assert_eq!(py_repr(&json!("tab\there\nnl")), r#"'tab\there\nnl'"#);
        assert_eq!(py_repr(&json!("back\\slash")), r#"'back\\slash'"#);
    }
}
