//! Todo boards, ported from `app/todos/routes.py` + `services/board_service.py`.
//!
//! **This domain is split, not moved.** Rust serves the CRUD; Python still
//! serves `items/{id}/agent/*`, `planning-form/submit` and `spawn-process`,
//! because those go through the LLM proxy and the orchestrator, neither of
//! which has migrated. Those Python routes write `todo_items` and
//! `todo_item_events` — so unlike projects and teams, this table has two
//! writers, which is what ADR 0007's rule 1 exists to prevent. It was a
//! deliberate call to take visible progress over waiting for `llm_proxy`.
//!
//! What keeps it survivable: Rust only ever writes the columns a user edits
//! (`UPDATE … SET title = ?`), never a whole row, so a concurrent agent write
//! to `plan_json` or `metadata_json` cannot be clobbered by a rename. The
//! reverse is not true — SQLAlchemy flushes whole rows, so an agent step that
//! loaded an item before a rename will write the old title back. The window is
//! one request wide and both writers are the same user, but it is real, and it
//! closes when `llm_proxy` moves.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::FromRow;

use crate::auth::Principal;
use crate::error::ApiError;
use crate::teams::{random_palette_color, stable_palette_color};
use crate::wire::{check_len, sql_now, sql_time, sql_time_opt};
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
        .route("/api/v1/todos/planner-profiles", get(planner_profiles))
    // Not here on purpose: items/{id}/agent/*, planning-form/submit and
    // spawn-process fall through to Python. See the module docs.
}

const TODO_STATUSES: [&str; 5] = ["plan", "backlog", "in_progress", "review", "done"];
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
        sqlx::query_as("SELECT project_id FROM todo_boards WHERE id = ?")
            .bind(board_id)
            .fetch_optional(&state.pool)
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

async fn assert_item_access(
    state: &AppState,
    principal: &Principal,
    item_id: i64,
) -> Result<(), ApiError> {
    let board_id: Option<i64> = sqlx::query_scalar("SELECT board_id FROM todo_items WHERE id = ?")
        .bind(item_id)
        .fetch_optional(&state.pool)
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

const BOARD_COLUMNS: &str =
    "id, project_id, name, description, default_model, created_at, updated_at, \
     (SELECT COUNT(*) FROM todo_categories c WHERE c.board_id = b.id) AS category_count, \
     (SELECT COUNT(*) FROM todo_items i WHERE i.board_id = b.id) AS item_count";

#[derive(FromRow)]
struct CategoryRow {
    id: i64,
    board_id: i64,
    name: String,
    color: Option<String>,
    sort_order: i64,
    planner_profile_id: Option<i64>,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize)]
struct CategoryOut {
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

const CATEGORY_COLUMNS: &str =
    "id, board_id, name, color, sort_order, planner_profile_id, created_at, updated_at";

#[derive(FromRow)]
struct ItemRow {
    id: i64,
    board_id: i64,
    category_id: Option<i64>,
    title: String,
    description: String,
    status: String,
    priority: i64,
    tags_json: Option<String>,
    plan_json: Option<String>,
    metadata_json: Option<String>,
    assigned_profile_id: Option<i64>,
    linked_process_id: Option<i64>,
    parent_item_id: Option<i64>,
    due_at: Option<String>,
    scheduled_at: Option<String>,
    time_horizon: Option<String>,
    item_kind: Option<String>,
    recurrence_json: Option<String>,
    completion_json: Option<String>,
    created_at: String,
    updated_at: String,
}

const ITEM_COLUMNS: &str = "id, board_id, category_id, title, description, status, priority, \
     tags_json, plan_json, metadata_json, assigned_profile_id, linked_process_id, parent_item_id, \
     due_at, scheduled_at, time_horizon, item_kind, recurrence_json, completion_json, \
     created_at, updated_at";

#[derive(Serialize)]
struct ItemOut {
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
    sqlx::query_as(&format!("SELECT {BOARD_COLUMNS} FROM todo_boards b WHERE id = ?"))
        .bind(board_id)
        .fetch_optional(&state.pool)
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
        Some(project_id) => sqlx::query_as(&format!(
            "SELECT {BOARD_COLUMNS} FROM todo_boards b WHERE project_id = ? ORDER BY id ASC"
        ))
        .bind(project_id)
        .fetch_all(&state.pool)
        .await?,
        None => {
            sqlx::query_as(&format!("SELECT {BOARD_COLUMNS} FROM todo_boards b ORDER BY id ASC"))
                .fetch_all(&state.pool)
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

fn default_board_model() -> Option<String> {
    Some("gemma4:31b-cloud".into())
}

async fn create_board(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Query(q): Query<ProjectQuery>,
    body: Option<Json<BoardCreate>>,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:write")?;
    if principal.workspace_id.is_some() {
        let project_id = q.project_id.ok_or_else(|| {
            ApiError::bad_request("project_id is required for a workspace-scoped token.")
        })?;
        crate::projects::assert_access(&state, &principal, project_id).await?;
    }

    let Json(req) = body.ok_or_else(|| {
        ApiError::validation(vec![ApiError::field_error("name", "missing", "Field required")])
    })?;
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
    let board_id: i64 = sqlx::query_scalar(
        "INSERT INTO todo_boards (project_id, name, description, default_model, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(q.project_id)
    .bind(req.name.unwrap_or_default().trim())
    .bind(req.description.map(|d| d.trim().to_string()).filter(|d| !d.is_empty()))
    .bind(req.default_model)
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.pool)
    .await?;

    if let Some(slug) = req.template_slug.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        // Deliberately after the insert, like Python: an unknown slug 400s and
        // leaves the (already committed) empty board behind.
        apply_board_template(&state, board_id, slug).await?;
    }

    Ok((StatusCode::CREATED, Json(load_board(&state, board_id).await?)).into_response())
}

async fn apply_board_template(state: &AppState, board_id: i64, slug: &str) -> Result<(), ApiError> {
    let Some((_, _, _, categories)) = BOARD_TEMPLATES.iter().find(|(s, ..)| *s == slug) else {
        return Err(ApiError::bad_request(format!("Unknown board template: {slug}")));
    };
    let now = sql_now();
    for (i, (name, color, profile_slug)) in categories.iter().enumerate() {
        let profile_id: Option<i64> =
            sqlx::query_scalar("SELECT id FROM planner_agent_profiles WHERE slug = ?")
                .bind(profile_slug)
                .fetch_optional(&state.pool)
                .await?;
        sqlx::query(
            "INSERT INTO todo_categories \
             (board_id, name, color, sort_order, planner_profile_id, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(board_id)
        .bind(name)
        .bind(color)
        .bind(i as i64)
        .bind(profile_id)
        .bind(&now)
        .bind(&now)
        .execute(&state.pool)
        .await?;
    }
    Ok(())
}

async fn get_board(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Path(board_id): Path<i64>,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:read")?;
    assert_board_access(&state, &principal, board_id).await?;
    let board = load_board(&state, board_id).await?;

    // `record_board_visit`: opening a board is what makes "Continue planning"
    // point at it, so the read has a write in it.
    if let Some(project_id) = board.project_id {
        sqlx::query("UPDATE project SET last_todo_board_id = ? WHERE id = ?")
            .bind(board_id)
            .bind(project_id)
            .execute(&state.pool)
            .await?;
    }

    let categories: Vec<CategoryOut> = sqlx::query_as::<_, CategoryRow>(&format!(
        "SELECT {CATEGORY_COLUMNS} FROM todo_categories WHERE board_id = ? \
         ORDER BY sort_order ASC, id ASC"
    ))
    .bind(board_id)
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(CategoryOut::from)
    .collect();

    let items: Vec<ItemOut> = sqlx::query_as::<_, ItemRow>(&format!(
        "SELECT {ITEM_COLUMNS} FROM todo_items WHERE board_id = ? ORDER BY updated_at DESC"
    ))
    .bind(board_id)
    .fetch_all(&state.pool)
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
    Path(board_id): Path<i64>,
    body: Option<Json<BoardUpdate>>,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:write")?;
    assert_board_access(&state, &principal, board_id).await?;
    load_board(&state, board_id).await?;

    let Json(req) = body.ok_or_else(|| ApiError::validation(vec![]))?;
    let mut errors = Vec::new();
    check_len(&mut errors, &["name"], req.name.as_deref(), 1, 256);
    check_len(&mut errors, &["default_model"], req.default_model.as_deref(), 0, 128);
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    if let Some(name) = req.name {
        sqlx::query("UPDATE todo_boards SET name = ? WHERE id = ?")
            .bind(name.trim())
            .bind(board_id)
            .execute(&state.pool)
            .await?;
    }
    if let Some(description) = req.description {
        sqlx::query("UPDATE todo_boards SET description = ? WHERE id = ?")
            .bind(Some(description.trim().to_string()).filter(|d| !d.is_empty()))
            .bind(board_id)
            .execute(&state.pool)
            .await?;
    }
    if let Some(model) = req.default_model {
        sqlx::query("UPDATE todo_boards SET default_model = ? WHERE id = ?")
            .bind(Some(model.trim().to_string()).filter(|m| !m.is_empty()))
            .bind(board_id)
            .execute(&state.pool)
            .await?;
    }
    sqlx::query("UPDATE todo_boards SET updated_at = ? WHERE id = ?")
        .bind(sql_now())
        .bind(board_id)
        .execute(&state.pool)
        .await?;

    Ok(Json(load_board(&state, board_id).await?).into_response())
}

async fn delete_board(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Path(board_id): Path<i64>,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:write")?;
    assert_board_access(&state, &principal, board_id).await?;
    load_board(&state, board_id).await?;
    // No cascade, matching `session.delete(board)` with SQLite foreign keys off:
    // categories and items outlive the board. Changing that here would be a
    // behaviour change hiding inside a port.
    sqlx::query("DELETE FROM todo_boards WHERE id = ?")
        .bind(board_id)
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---------------------------------------------------------------------------
// Categories
// ---------------------------------------------------------------------------

async fn list_categories(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Path(board_id): Path<i64>,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:read")?;
    assert_board_access(&state, &principal, board_id).await?;
    load_board(&state, board_id).await?;
    let categories: Vec<CategoryOut> = sqlx::query_as::<_, CategoryRow>(&format!(
        "SELECT {CATEGORY_COLUMNS} FROM todo_categories WHERE board_id = ? \
         ORDER BY sort_order ASC, id ASC"
    ))
    .bind(board_id)
    .fetch_all(&state.pool)
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
    Path(board_id): Path<i64>,
    body: Option<Json<CategoryCreate>>,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:write")?;
    assert_board_access(&state, &principal, board_id).await?;
    load_board(&state, board_id).await?;

    let Json(req) = body.ok_or_else(|| {
        ApiError::validation(vec![ApiError::field_error("name", "missing", "Field required")])
    })?;
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
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO todo_categories \
         (board_id, name, color, sort_order, planner_profile_id, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(board_id)
    .bind(req.name.unwrap_or_default().trim())
    .bind(&color)
    .bind(req.sort_order)
    .bind(req.planner_profile_id)
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.pool)
    .await?;

    Ok((StatusCode::CREATED, Json(load_category(&state, board_id, id).await?)).into_response())
}

async fn load_category(
    state: &AppState,
    board_id: i64,
    category_id: i64,
) -> Result<CategoryOut, ApiError> {
    let row: Option<CategoryRow> =
        sqlx::query_as(&format!("SELECT {CATEGORY_COLUMNS} FROM todo_categories WHERE id = ?"))
            .bind(category_id)
            .fetch_optional(&state.pool)
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
    Path((board_id, category_id)): Path<(i64, i64)>,
    body: Option<Json<CategoryUpdate>>,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:write")?;
    assert_board_access(&state, &principal, board_id).await?;
    load_category(&state, board_id, category_id).await?;

    let Json(req) = body.ok_or_else(|| ApiError::validation(vec![]))?;
    let mut errors = Vec::new();
    check_len(&mut errors, &["name"], req.name.as_deref(), 1, 128);
    check_len(&mut errors, &["color"], req.color.as_deref(), 0, 32);
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    if let Some(name) = req.name {
        sqlx::query("UPDATE todo_categories SET name = ? WHERE id = ?")
            .bind(name.trim())
            .bind(category_id)
            .execute(&state.pool)
            .await?;
    }
    if let Some(color) = req.color {
        sqlx::query("UPDATE todo_categories SET color = ? WHERE id = ?")
            .bind(color)
            .bind(category_id)
            .execute(&state.pool)
            .await?;
    }
    if let Some(sort_order) = req.sort_order {
        sqlx::query("UPDATE todo_categories SET sort_order = ? WHERE id = ?")
            .bind(sort_order)
            .bind(category_id)
            .execute(&state.pool)
            .await?;
    }
    if let Some(profile_id) = req.planner_profile_id {
        sqlx::query("UPDATE todo_categories SET planner_profile_id = ? WHERE id = ?")
            .bind(profile_id)
            .bind(category_id)
            .execute(&state.pool)
            .await?;
    }
    sqlx::query("UPDATE todo_categories SET updated_at = ? WHERE id = ?")
        .bind(sql_now())
        .bind(category_id)
        .execute(&state.pool)
        .await?;

    Ok(Json(load_category(&state, board_id, category_id).await?).into_response())
}

// ---------------------------------------------------------------------------
// Items
// ---------------------------------------------------------------------------

async fn load_item(state: &AppState, item_id: i64) -> Result<ItemOut, ApiError> {
    let row: Option<ItemRow> =
        sqlx::query_as(&format!("SELECT {ITEM_COLUMNS} FROM todo_items WHERE id = ?"))
            .bind(item_id)
            .fetch_optional(&state.pool)
            .await?;
    row.map(ItemOut::from).ok_or_else(|| ApiError::not_found("Item not found"))
}

async fn list_items(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Path(board_id): Path<i64>,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:read")?;
    assert_board_access(&state, &principal, board_id).await?;
    load_board(&state, board_id).await?;
    let items: Vec<ItemOut> = sqlx::query_as::<_, ItemRow>(&format!(
        "SELECT {ITEM_COLUMNS} FROM todo_items WHERE board_id = ? ORDER BY updated_at DESC"
    ))
    .bind(board_id)
    .fetch_all(&state.pool)
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
    Path(board_id): Path<i64>,
    body: Option<Json<ItemCreate>>,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:write")?;
    assert_board_access(&state, &principal, board_id).await?;
    load_board(&state, board_id).await?;

    let Json(req) = body.ok_or_else(|| {
        ApiError::validation(vec![ApiError::field_error("title", "missing", "Field required")])
    })?;
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
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO todo_items (board_id, category_id, title, description, status, priority, \
         tags_json, assigned_profile_id, parent_item_id, due_at, scheduled_at, time_horizon, \
         item_kind, recurrence_json, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
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
    .fetch_one(&state.pool)
    .await?;

    Ok((StatusCode::CREATED, Json(load_item(&state, id).await?)).into_response())
}

/// Callers send ISO-8601; the column holds SQLAlchemy's space-separated form.
fn datetime_to_sql(raw: &str) -> String {
    raw.replacen('T', " ", 1).trim_end_matches('Z').to_string()
}

async fn get_item(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Path(item_id): Path<i64>,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:read")?;
    assert_item_access(&state, &principal, item_id).await?;
    Ok(Json(load_item(&state, item_id).await?).into_response())
}

async fn update_item(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Path(item_id): Path<i64>,
    body: Option<Json<Value>>,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:write")?;
    assert_item_access(&state, &principal, item_id).await?;
    let item = load_item(&state, item_id).await?;

    let patch: Map<String, Value> = match body {
        Some(Json(Value::Object(map))) => map,
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

    sqlx::query("UPDATE todo_items SET updated_at = ? WHERE id = ?")
        .bind(sql_now())
        .bind(item_id)
        .execute(&state.pool)
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
    T: for<'q> sqlx::Encode<'q, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite> + Send,
{
    // The column name is from a fixed list in this module, never from input.
    let sql = format!("UPDATE todo_items SET {column} = ? WHERE id = ?");
    sqlx::query(&sql).bind(value).bind(item_id).execute(&state.pool).await?;
    Ok(())
}

async fn delete_item(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Path(item_id): Path<i64>,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:write")?;
    assert_item_access(&state, &principal, item_id).await?;
    load_item(&state, item_id).await?;
    sqlx::query("DELETE FROM todo_items WHERE id = ?")
        .bind(item_id)
        .execute(&state.pool)
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

async fn item_events(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Path(item_id): Path<i64>,
    Query(q): Query<EventQuery>,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:read")?;
    assert_item_access(&state, &principal, item_id).await?;
    load_item(&state, item_id).await?;

    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT id, item_id, event_type, content_json, created_at FROM todo_item_events \
         WHERE item_id = ? AND id > ? ORDER BY id ASC LIMIT ?",
    )
    .bind(item_id)
    .bind(q.after_id)
    .bind(q.limit)
    .fetch_all(&state.pool)
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

#[derive(FromRow)]
struct ProfileRow {
    id: i64,
    slug: String,
    name: String,
    requirement_type: String,
    system_prompt: String,
    default_model: Option<String>,
    action_set_id: Option<i64>,
    skill_paths_json: Option<String>,
}

async fn planner_profiles(
    State(state): State<Arc<AppState>>,
    principal: Principal,
) -> Result<Response, ApiError> {
    require_scope(&principal, "todos:read")?;
    let rows: Vec<ProfileRow> = sqlx::query_as(
        "SELECT id, slug, name, requirement_type, system_prompt, default_model, action_set_id, \
         skill_paths_json FROM planner_agent_profiles ORDER BY id ASC",
    )
    .fetch_all(&state.pool)
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

    #[test]
    fn iso_input_becomes_a_sqlalchemy_timestamp() {
        assert_eq!(datetime_to_sql("2026-08-05T22:11:22"), "2026-08-05 22:11:22");
        assert_eq!(datetime_to_sql("2026-08-05T22:11:22Z"), "2026-08-05 22:11:22");
        assert_eq!(datetime_to_sql("2026-08-05 22:11:22"), "2026-08-05 22:11:22");
    }
}
