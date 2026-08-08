//! Model build/train operations — `app/model_ops/`, all seventeen routes.
//!
//! Ollama management (list, show, pull, copy, create, delete), the project
//! scaffold and its uploads, the registry, and the five pipeline-job routes.
//!
//! # The build pipeline, after the Python server
//!
//! `runner.py` is gone; [`spawn_pipeline_job`] replaces it. The training code
//! itself is *not* ported and never will be — LoRA fine-tuning is torch and
//! peft — so it stays Python, but as a **worker invoked by subprocess rather
//! than a server**. That was already half true: `runner.py` ran `train` and
//! `export` as a `python -c` child under `MODEL_OPS_PYTHON`, because the GPU
//! stages need their own interpreter and their own memory. Three things changed
//! to make the other half true:
//!
//! - **Every stage is a subprocess now**, not just the two GPU ones.
//!   `MODEL_OPS_GPU_SUBPROCESS` used to gate that and `prepare`/`eval` ran
//!   in-process; there is no in-process any more, so the variable is gone. The
//!   stage scripts are the ones `_stage_script` already emitted.
//! - **`eval`'s result comes back through stdout.** It used to be a function
//!   return value, which is exactly why this domain could not migrate; it is
//!   now a `@@AGP:eval@@ {json}` line the parent picks out of the log stream.
//! - **The registry write comes back the same way.** `register_model_entry`
//!   used to reach into SQLAlchemy from inside the training child, which meant
//!   the child needed `database.py`, `models.py` and the whole ORM. It now
//!   prints `@@AGP:registry@@ {json}` and [`persist_registry_entry`] here does
//!   the write — so `model_build_jobs`, `model_projects` and
//!   `model_registry_entries` have exactly one writer, this process.
//!
//! The markers are read from the same stream that is teed to the job log, so a
//! stage that emits one still has it in its log for a human to read.
//!
//! Cancellation was the other stated blocker: `runner.py` kept a module-level
//! `_running` dict, so only the process that started a job could stop one. That
//! dict is now [`AppState::model_jobs`], and it is the *same* process, because
//! there is only one.
//!
//! ponytail: written against `state.pool` like every domain but `projects`; the
//! Postgres port converts one domain at a time and this is not converted yet.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Multipart, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
use serde_json::{json, Map, Value};
use sqlx::FromRow;

use crate::auth::Principal;
use crate::error::{ApiError, PathId};
use crate::llm_config::{config_dir, ollama_api_base};
use crate::upstream_http::send_with_retry;
use crate::wire::{lax_bool, optional_str, parse_body, required_str, sql_now};
use crate::{env_opt, AppState};

pub fn routes() -> Router<Arc<AppState>> {
    const BASE: &str = "/api/v1/model-ops";
    Router::new()
        .route(&format!("{BASE}/ollama/models"), get(ollama_list_models))
        // One wildcard route for `{name:path}`, and the three static POSTs ride
        // on it. Registering `/ollama/models/pull` separately would make a
        // **GET** of that path a 405 here, where FastAPI — which declares the
        // `{name:path}` route first — answers it as "show the model called
        // pull". Dispatching inside keeps both methods on one matcher.
        .route(
            &format!("{BASE}/ollama/models/{{*name}}"),
            get(ollama_show_model).post(ollama_models_post).delete(ollama_delete),
        )
        .route(&format!("{BASE}/ollama/jobs"), post(ollama_jobs_create))
        .route(&format!("{BASE}/jobs"), post(jobs_create))
        .route(&format!("{BASE}/jobs/{{job_id}}"), get(jobs_get))
        .route(&format!("{BASE}/jobs/{{job_id}}/stream"), get(jobs_stream))
        .route(&format!("{BASE}/jobs/{{job_id}}/cancel"), post(jobs_cancel))
        .route(&format!("{BASE}/operations/build"), post(build_operation))
        .route(&format!("{BASE}/projects"), get(projects_list).post(projects_create))
        .route(&format!("{BASE}/projects/{{name}}"), get(projects_get))
        // Both take multipart, and a training set or an adapter is legitimately
        // bigger than the general body cap. Applied per route rather than to the
        // whole module, so a mistyped JSON body to `/projects` is still refused
        // at 16 MB instead of buffering half a gigabyte first.
        .route(
            &format!("{BASE}/projects/{{name}}/knowledge"),
            post(upload_knowledge).layer(DefaultBodyLimit::max(crate::upload_body_limit())),
        )
        .route(
            &format!("{BASE}/projects/{{name}}/files"),
            post(upload_files).layer(DefaultBodyLimit::max(crate::upload_body_limit())),
        )
        .route(&format!("{BASE}/registry"), get(registry_list))
        .route(&format!("{BASE}/registry/{{entry_id}}/activate"), post(registry_activate))
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// `MODEL_OPS_DATA_DIR`, else `CONFIG_DIR/model_ops`.
fn data_dir() -> PathBuf {
    env_opt("MODEL_OPS_DATA_DIR").map(PathBuf::from).unwrap_or_else(|| config_dir().join("model_ops"))
}

fn projects_dir() -> PathBuf {
    data_dir().join("projects")
}

fn template_project_dir() -> PathBuf {
    projects_dir().join("_template")
}

/// `model_ops/data`, the seed for `defaults.yaml` and the `_template` project.
///
/// It used to live under `app/`, beside the Python server package; it now ships
/// with the build worker, because the worker is the thing that reads the
/// projects it seeds. Searched in the same three places [`worker_pythonpath`]
/// looks: the explicit override, beside the executable, then the checkout.
fn bundled_data_dir() -> Option<PathBuf> {
    let candidates = [
        env_opt("MODEL_OPS_WORKER_PATH").map(PathBuf::from),
        std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join("worker"))),
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .map(|repo| repo.join("worker")),
    ];
    candidates
        .into_iter()
        .flatten()
        .map(|root| root.join("model_ops").join("data"))
        .find(|path| path.is_dir())
}

/// `ensure_data_scaffold`: the data dir, `defaults.yaml` and the `_template`
/// project, created from the bundled copies when they are missing.
fn ensure_data_scaffold() -> PathBuf {
    let root = data_dir();
    let _ = std::fs::create_dir_all(&root);
    let _ = std::fs::create_dir_all(projects_dir());

    if let Some(bundled) = bundled_data_dir() {
        let defaults = root.join("defaults.yaml");
        if !defaults.exists() && bundled.join("defaults.yaml").exists() {
            let _ = std::fs::copy(bundled.join("defaults.yaml"), &defaults);
        }
        let template = template_project_dir();
        let bundled_template = bundled.join("projects").join("_template");
        if bundled_template.is_dir() && !template.exists() {
            let _ = copy_tree(&bundled_template, &template);
        }
    }
    root
}

/// `shutil.copytree` — the destination must not already exist, which both
/// callers have checked.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn io_error(e: std::io::Error) -> ApiError {
    logd!("model-ops filesystem error: {e}");
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "An unexpected error occurred.")
}

/// `get_project_dir` + `load_project`: the manifest, plus the two underscore
/// keys the loader adds to every one it returns.
fn load_project(name: &str) -> Result<Map<String, Value>, ApiError> {
    ensure_data_scaffold();
    let dir = projects_dir().join(name);
    if !dir.is_dir() {
        return Err(ApiError::not_found(format!(
            "Project not found: {name} ({})",
            dir.display()
        )));
    }
    let manifest_path = dir.join("project.yaml");
    if !manifest_path.exists() {
        return Err(ApiError::not_found(format!(
            "Missing project.yaml: {}",
            manifest_path.display()
        )));
    }
    let raw = std::fs::read_to_string(&manifest_path).map_err(io_error)?;
    let mut data = match serde_yaml::from_str::<Value>(&raw) {
        Ok(Value::Object(map)) => map,
        // `data["_project_dir"] = …` on a non-mapping raises in Python; a 500
        // either way, and this is the same 500.
        _ => {
            return Err(ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "An unexpected error occurred.",
            ))
        }
    };
    data.insert("_project_dir".into(), json!(dir.to_string_lossy()));
    data.insert("_name".into(), json!(name));
    Ok(data)
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct ProjectRow {
    id: i64,
    name: String,
    description: Option<String>,
    manifest_json: Option<String>,
}

#[derive(FromRow)]
struct RegistryRow {
    id: i64,
    project_id: i64,
    version: String,
    ollama_tag: String,
    base_model: Option<String>,
    eval_score: Option<f64>,
    is_active: bool,
}

#[derive(FromRow)]
struct JobRow {
    id: i64,
    project_id: Option<i64>,
    job_type: String,
    stages_json: String,
    status: String,
    current_stage: Option<String>,
    log_path: Option<String>,
    result_json: Option<String>,
    register_alias: Option<String>,
    error_message: Option<String>,
    created_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
}

const JOB_COLUMNS: &str = "id, project_id, job_type, stages_json, status, current_stage, \
     log_path, result_json, register_alias, error_message, \
     CAST(created_at AS TEXT) AS created_at, CAST(started_at AS TEXT) AS started_at, \
     CAST(finished_at AS TEXT) AS finished_at";

/// `json.loads` behind `except JSONDecodeError`, with a non-mapping result read
/// as `{}` — this one *does* check the type, unlike the action registry's.
fn object_or_empty(raw: Option<&str>) -> Value {
    match raw.filter(|s| !s.is_empty()).and_then(|s| serde_json::from_str::<Value>(s).ok()) {
        Some(v @ Value::Object(_)) => v,
        _ => json!({}),
    }
}

impl RegistryRow {
    fn to_out(&self, project_name: Option<&str>) -> Value {
        json!({
            "id": self.id,
            "project_id": self.project_id,
            "project_name": project_name,
            "version": self.version,
            "ollama_tag": self.ollama_tag,
            "base_model": self.base_model,
            "eval_score": self.eval_score,
            "is_active": self.is_active,
        })
    }
}

async fn registry_entries_for_project(
    state: &AppState,
    project_id: i64,
    project_name: Option<&str>,
) -> Result<Vec<Value>, ApiError> {
    let rows: Vec<RegistryRow> = sqlx::query_as(
        "SELECT id, project_id, version, ollama_tag, base_model, eval_score, is_active \
         FROM model_registry_entries WHERE project_id = ? ORDER BY created_at DESC",
    )
    .bind(project_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(rows.iter().map(|row| row.to_out(project_name)).collect())
}

async fn project_to_out(state: &AppState, row: &ProjectRow) -> Result<Value, ApiError> {
    Ok(json!({
        "id": row.id,
        "name": row.name,
        "description": row.description,
        "manifest": object_or_empty(row.manifest_json.as_deref()),
        "registry_entries": registry_entries_for_project(state, row.id, Some(&row.name)).await?,
    }))
}

/// `_sync_project_row`: read the manifest off disk and write it back to the row,
/// inserting one if this project has never been seen.
async fn sync_project_row(state: &AppState, name: &str) -> Result<ProjectRow, ApiError> {
    let manifest = load_project(name)?;
    let manifest_json = Value::Object(manifest.clone()).to_string();
    let now = sql_now();

    let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM model_projects WHERE name = ?")
        .bind(name)
        .fetch_optional(&state.pool)
        .await?;

    match existing {
        Some(id) => {
            sqlx::query("UPDATE model_projects SET manifest_json = ?, updated_at = ? WHERE id = ?")
                .bind(&manifest_json)
                .bind(&now)
                .bind(id)
                .execute(&state.pool)
                .await?;
        }
        None => {
            // A new row takes its `description` from the manifest, and only a
            // string counts — `manifest.get("description")` on a mapping or a
            // number would not survive the column either.
            let description = manifest.get("description").and_then(Value::as_str);
            sqlx::query(
                "INSERT INTO model_projects \
                 (name, description, manifest_json, workspace_id, created_at, updated_at) \
                 VALUES (?, ?, ?, NULL, ?, ?)",
            )
            .bind(name)
            .bind(description)
            .bind(&manifest_json)
            .bind(&now)
            .bind(&now)
            .execute(&state.pool)
            .await?;
        }
    }

    require_project(state, name).await
}

async fn require_project(state: &AppState, name: &str) -> Result<ProjectRow, ApiError> {
    sqlx::query_as(
        "SELECT id, name, description, manifest_json FROM model_projects WHERE name = ?",
    )
    .bind(name)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found(format!("Project not found: {name}")))
}

// ---------------------------------------------------------------------------
// Jobs (the rows; the pipeline runner is Python's)
// ---------------------------------------------------------------------------

/// Training logs grow to hundreds of MB; never read one whole just to show its
/// end.
const LOG_TAIL_BYTES: u64 = 256 * 1024;

fn read_job_log_tail(log_path: Option<&str>, lines: usize) -> String {
    let Some(path) = log_path.filter(|p| !p.is_empty()).map(Path::new) else {
        return String::new();
    };
    if !path.exists() {
        return String::new();
    }
    let Ok(mut file) = std::fs::File::open(path) else { return String::new() };
    let Ok(size) = file.metadata().map(|m| m.len()) else { return String::new() };

    use std::io::{Read, Seek, SeekFrom};
    if file.seek(SeekFrom::Start(size.saturating_sub(LOG_TAIL_BYTES))).is_err() {
        return String::new();
    }
    let mut blob = Vec::new();
    if file.read_to_end(&mut blob).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&blob);
    let mut content: Vec<&str> = text.split('\n').collect();
    // `str.splitlines()` drops a single trailing newline rather than yielding an
    // empty last element.
    if content.last() == Some(&"") {
        content.pop();
    }
    if size > LOG_TAIL_BYTES && !content.is_empty() {
        // The window almost certainly cut the first line in half.
        content.remove(0);
    }
    let tail = content.len().saturating_sub(lines);
    content[tail..].join("\n")
}

impl JobRow {
    /// `get_stages`, which falls back to `["prepare"]` rather than to empty.
    fn stages(&self) -> Value {
        match serde_json::from_str::<Value>(&self.stages_json) {
            Ok(Value::Array(items)) => {
                Value::Array(items.iter().map(|v| json!(crate::action_orchestrator::py_display(v))).collect())
            }
            Ok(_) => json!(["prepare"]),
            Err(_) => json!(["prepare"]),
        }
    }

    fn to_out(&self, project_name: Option<&str>) -> Value {
        json!({
            "id": self.id,
            "job_type": self.job_type,
            "project_id": self.project_id,
            "project_name": project_name,
            "stages": self.stages(),
            "status": self.status,
            "current_stage": self.current_stage,
            "register_alias": self.register_alias,
            "result": object_or_empty(self.result_json.as_deref()),
            "error_message": self.error_message,
            "log_tail": read_job_log_tail(self.log_path.as_deref(), 80),
            "poll_url": format!("/api/v1/model-ops/jobs/{}", self.id),
            "stream_url": format!("/api/v1/model-ops/jobs/{}/stream", self.id),
            "created_at": iso(&self.created_at),
            "started_at": self.started_at.as_deref().map(iso),
            "finished_at": self.finished_at.as_deref().map(iso),
        })
    }
}

/// `datetime.isoformat()` off a stored timestamp — the job shape stringifies
/// these itself rather than letting pydantic render a `datetime`.
fn iso(raw: &str) -> String {
    crate::wire::iso_from_sql(raw)
}

async fn job_out(state: &AppState, job_id: i64) -> Result<Value, ApiError> {
    let job: JobRow = sqlx::query_as(&format!(
        "SELECT {JOB_COLUMNS} FROM model_build_jobs WHERE id = ?"
    ))
    .bind(job_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("Job not found"))?;

    let project_name: Option<String> = match job.project_id {
        None => None,
        Some(project_id) => {
            let found: Option<String> =
                sqlx::query_scalar("SELECT name FROM model_projects WHERE id = ?")
                    .bind(project_id)
                    .fetch_optional(&state.pool)
                    .await?;
            Some(found.unwrap_or_else(|| "unknown".into()))
        }
    };
    Ok(job.to_out(project_name.as_deref()))
}

/// `create_ollama_job`: the row, then the log file named after its id.
async fn create_ollama_job(
    state: &AppState,
    job_type: &str,
    operation: Value,
) -> Result<i64, ApiError> {
    let logs_dir = ensure_data_scaffold().join("logs");
    std::fs::create_dir_all(&logs_dir).map_err(io_error)?;

    let now = sql_now();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO model_build_jobs \
         (project_id, job_type, operation_json, stages_json, status, current_stage, log_path, \
          result_json, register_alias, error_message, process_id, created_at, started_at, \
          finished_at) \
         VALUES (NULL, ?, ?, '[]', 'pending', NULL, NULL, NULL, NULL, NULL, NULL, ?, NULL, NULL) \
         RETURNING id",
    )
    .bind(job_type)
    .bind(operation.to_string())
    .bind(&now)
    .fetch_one(&state.pool)
    .await?;

    let log_path = logs_dir.join(format!("job_{id}.log"));
    std::fs::write(&log_path, "").map_err(io_error)?;
    sqlx::query("UPDATE model_build_jobs SET log_path = ? WHERE id = ?")
        .bind(log_path.to_string_lossy().as_ref())
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok(id)
}

fn append_job_log(log_path: Option<&str>, text: &str) {
    let Some(path) = log_path.filter(|p| !p.is_empty()) else { return };
    use std::io::Write;
    let Ok(mut file) = std::fs::OpenOptions::new().append(true).create(true).open(path) else {
        return;
    };
    let _ = file.write_all(text.as_bytes());
    if !text.ends_with('\n') {
        let _ = file.write_all(b"\n");
    }
}

/// `run_ollama_job` — the half of the runner that has no subprocess, so it runs
/// here. Started detached, which is what `BackgroundTasks` gives Python: the
/// enqueue response goes out first.
fn spawn_ollama_job(state: Arc<AppState>, job_id: i64, job_type: String, operation: Value) {
    tokio::spawn(async move {
        let log_path: Option<String> =
            sqlx::query_scalar("SELECT log_path FROM model_build_jobs WHERE id = ?")
                .bind(job_id)
                .fetch_optional(&state.pool)
                .await
                .ok()
                .flatten();

        let _ = sqlx::query(
            "UPDATE model_build_jobs SET status = 'running', started_at = ?, current_stage = ? \
             WHERE id = ?",
        )
        .bind(sql_now())
        .bind(&job_type)
        .bind(job_id)
        .execute(&state.pool)
        .await;

        let outcome = match job_type.as_str() {
            "ollama_pull" => {
                let name = string_field(&operation, "name");
                append_job_log(log_path.as_deref(), &format!("Pulling {name}...\n"));
                stream_ollama(&state, "pull", json!({"name": name, "stream": true}))
                    .await
                    .map(last_event)
            }
            "ollama_copy" => {
                let source = string_field(&operation, "source");
                let destination = string_field(&operation, "destination");
                append_job_log(
                    log_path.as_deref(),
                    &format!("Copying {source} -> {destination}...\n"),
                );
                stream_ollama(
                    &state,
                    "copy",
                    json!({"source": source, "destination": destination, "stream": true}),
                )
                .await
                .map(last_event)
            }
            other => Err(format!("Unknown ollama job type: {other}")),
        };

        match outcome {
            Ok(last) => {
                let _ = sqlx::query(
                    "UPDATE model_build_jobs SET status = 'succeeded', finished_at = ?, \
                     result_json = ? WHERE id = ?",
                )
                .bind(sql_now())
                .bind(json!({ "ollama": last }).to_string())
                .bind(job_id)
                .execute(&state.pool)
                .await;
            }
            Err(message) => {
                append_job_log(log_path.as_deref(), &format!("ERROR: {message}\n"));
                let truncated: String = message.chars().take(2000).collect();
                let _ = sqlx::query(
                    "UPDATE model_build_jobs SET status = 'failed', finished_at = ?, \
                     error_message = ? WHERE id = ?",
                )
                .bind(sql_now())
                .bind(truncated)
                .bind(job_id)
                .execute(&state.pool)
                .await;
            }
        }
    });
}

/// `str(op.get("name", ""))` — an absent key and a null both render as Python's
/// `str()` of what was there.
fn string_field(value: &Value, field: &str) -> String {
    match value.get(field) {
        None => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => crate::action_orchestrator::py_display(other),
    }
}

/// `last: dict = {"status": "unknown"}`, replaced by each event that arrives.
fn last_event(events: Vec<Value>) -> Value {
    events.into_iter().next_back().unwrap_or_else(|| json!({"status": "unknown"}))
}

// ---------------------------------------------------------------------------
// Ollama
// ---------------------------------------------------------------------------

/// `ollama_client._base()`, which raises when the base URL is blank — a 503 on
/// every route that reaches it.
fn ollama_base() -> Result<String, String> {
    let base = ollama_api_base();
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("OLLAMA_API_BASE is not configured".into());
    }
    Ok(base.to_string())
}

/// A buffered `/api/*` call. `Err` carries the `RuntimeError` message each
/// route turns into its own status.
async fn ollama_call(
    state: &AppState,
    method: &'static str,
    path: &str,
    body: Option<Value>,
    timeout: Duration,
    context: &'static str,
) -> Result<Value, String> {
    let base = ollama_base()?;
    let url = format!("{base}{path}");
    let response = send_with_retry(context, false, || {
        let request = match method {
            "POST" => state.http.post(&url).json(body.as_ref().unwrap_or(&Value::Null)),
            _ => state.http.get(&url),
        };
        request.timeout(timeout)
    })
    .await
    .map_err(|e| e.message)?;

    if !response.is_ok() {
        let text: String = response.text().chars().take(200).collect();
        return Err(match method {
            "GET" => format!("Ollama {path} returned {}", response.status.as_u16()),
            _ => format!("Ollama {path} returned {}: {text}", response.status.as_u16()),
        });
    }
    response.json().ok_or_else(|| format!("Ollama {path} returned a non-JSON body"))
}

/// One of the three streaming `/api/*` calls, collected into its NDJSON events.
///
/// A line that is not JSON becomes `{"status": <line>}`, which is how Python
/// reports the progress text Ollama sometimes writes bare.
async fn stream_ollama(state: &AppState, op: &str, body: Value) -> Result<Vec<Value>, String> {
    let base = ollama_base()?;
    let url = format!("{base}/api/{op}");
    let response = state
        .http
        .post(&url)
        .json(&body)
        .timeout(Duration::from_secs(600))
        .send()
        .await
        .map_err(|e| format!("Ollama /api/{op} failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        let text: String = text.chars().take(500).collect();
        return Err(format!("Ollama /api/{op} returned {status}: {text}"));
    }

    let mut events = Vec::new();
    let mut pending = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Ollama /api/{op} stream failed: {e}"))?;
        pending.extend_from_slice(&chunk);
        while let Some(index) = pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = pending.drain(..=index).collect();
            push_event(&mut events, &line[..line.len() - 1]);
        }
    }
    push_event(&mut events, &pending);

    state.catalog.refresh_ollama_now(&state.http).await;
    Ok(events)
}

fn push_event(events: &mut Vec<Value>, raw: &[u8]) {
    let line = String::from_utf8_lossy(raw);
    if line.trim().is_empty() {
        return;
    }
    match serde_json::from_str::<Value>(&line) {
        Ok(value) => events.push(value),
        Err(_) => events.push(json!({ "status": line.trim() })),
    }
}

/// The last 20 events, which is all the operation shapes return.
fn tail_events(events: &[Value]) -> Vec<Value> {
    events[events.len().saturating_sub(20)..].to_vec()
}

fn operation_out(ok: bool, message: String, events: &[Value]) -> Value {
    json!({ "ok": ok, "message": message, "events": tail_events(events) })
}

fn event_status(event: Option<&Value>) -> String {
    event.map(|e| string_field(e, "status")).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Ollama routes
// ---------------------------------------------------------------------------

async fn ollama_list_models(
    State(state): State<Arc<AppState>>,
    principal: Principal,
) -> Result<Response, ApiError> {
    principal.require_scope("model:read")?;
    let payload = ollama_call(
        &state,
        "GET",
        "/api/tags",
        None,
        Duration::from_secs(15),
        "model_ops_tags",
    )
    .await
    .map_err(unavailable)?;
    let models = payload.get("models").cloned().filter(Value::is_array).unwrap_or_else(|| json!([]));
    Ok(Json(json!({ "models": models })).into_response())
}

fn unavailable(message: String) -> ApiError {
    ApiError::new(StatusCode::SERVICE_UNAVAILABLE, message)
}

async fn ollama_show_model(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(name): PathId<String>,
) -> Result<Response, ApiError> {
    principal.require_scope("model:read")?;
    let raw = ollama_call(
        &state,
        "POST",
        "/api/show",
        Some(json!({ "name": name })),
        Duration::from_secs(30),
        "model_ops_show",
    )
    .await
    .map_err(ApiError::not_found)?;

    Ok(Json(json!({
        "name": raw.get("model").cloned().filter(|v| !v.is_null()).unwrap_or_else(|| json!(name)),
        "modelfile": raw.get("modelfile"),
        "parameters": raw.get("parameters"),
        "details": raw.get("details"),
        "raw": raw,
    }))
    .into_response())
}

async fn ollama_delete(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(name): PathId<String>,
) -> Result<Response, ApiError> {
    principal.require_scope("model:write")?;
    ollama_call(
        &state,
        "POST",
        "/api/delete",
        Some(json!({ "name": name })),
        Duration::from_secs(60),
        "model_ops_delete",
    )
    .await
    .map_err(ApiError::not_found)?;
    state.catalog.refresh_ollama_now(&state.http).await;
    Ok(Json(operation_out(true, format!("Deleted {name}"), &[])).into_response())
}

/// The three `POST /ollama/models/*` routes, which share a matcher with the
/// `{name:path}` GET. Anything else is the 405 FastAPI answers for a path whose
/// only route is a GET.
async fn ollama_models_post(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(name): PathId<String>,
    raw: Bytes,
) -> Result<Response, ApiError> {
    match name.as_str() {
        "pull" => ollama_pull(state, principal, raw).await,
        "copy" => ollama_copy(state, principal, raw).await,
        "create" => ollama_create(state, principal, raw).await,
        // Starlette answers this one itself, **before** the app's exception
        // handler, so it is a bare `{"detail": …}` rather than the error
        // envelope every other failure here carries.
        _ => Ok((
            StatusCode::METHOD_NOT_ALLOWED,
            Json(json!({ "detail": "Method Not Allowed" })),
        )
            .into_response()),
    }
}

/// `Field(alias="async")` with `populate_by_name`: **both** spellings are
/// accepted, and the alias wins when a body carries both.
fn async_flag(errors: &mut Vec<Value>, body: &Value, default: bool) -> bool {
    if body.get("async").is_some() {
        return lax_bool(errors, body, "async");
    }
    if body.get("async_job").is_some() {
        return lax_bool(errors, body, "async_job");
    }
    default
}

async fn ollama_pull(
    state: Arc<AppState>,
    principal: Principal,
    raw: Bytes,
) -> Result<Response, ApiError> {
    principal.require_scope("model:write")?;
    let body = parse_body(&raw)?;
    let mut errors = Vec::new();
    let name = required_str(&mut errors, &body, "name");
    let async_job = async_flag(&mut errors, &body, false);
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    if async_job {
        let operation = json!({ "name": name });
        let job_id = create_ollama_job(&state, "ollama_pull", operation.clone()).await?;
        let out = job_out(&state, job_id).await?;
        spawn_ollama_job(state, job_id, "ollama_pull".into(), operation);
        return Ok(Json(out).into_response());
    }

    let events = stream_ollama(&state, "pull", json!({"name": name, "stream": true}))
        .await
        .map_err(unavailable)?;
    // `not events or events[-1]["status"] in (...)` — an empty stream counts as
    // success, which is what a model that was already present produces.
    let status = event_status(events.last());
    let ok = events.is_empty() || status == "success" || status == "pulling";
    Ok(Json(operation_out(ok, format!("Pulled {name}"), &events)).into_response())
}

async fn ollama_copy(
    state: Arc<AppState>,
    principal: Principal,
    raw: Bytes,
) -> Result<Response, ApiError> {
    principal.require_scope("model:write")?;
    let body = parse_body(&raw)?;
    let mut errors = Vec::new();
    let source = required_str(&mut errors, &body, "source");
    let destination = required_str(&mut errors, &body, "destination");
    // Copy defaults the other way from pull: async unless told otherwise.
    let async_job = async_flag(&mut errors, &body, true);
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    if async_job {
        let operation = json!({ "source": source, "destination": destination });
        let job_id = create_ollama_job(&state, "ollama_copy", operation.clone()).await?;
        let out = job_out(&state, job_id).await?;
        spawn_ollama_job(state, job_id, "ollama_copy".into(), operation);
        return Ok(Json(out).into_response());
    }

    let events = stream_ollama(
        &state,
        "copy",
        json!({"source": source, "destination": destination, "stream": true}),
    )
    .await
    .map_err(unavailable)?;
    let ok = events.is_empty() || event_status(events.last()) == "success";
    Ok(Json(operation_out(ok, format!("Copied {source} -> {destination}"), &events))
        .into_response())
}

async fn ollama_create(
    state: Arc<AppState>,
    principal: Principal,
    raw: Bytes,
) -> Result<Response, ApiError> {
    principal.require_scope("model:write")?;
    let body = parse_body(&raw)?;
    let mut errors = Vec::new();
    let name = required_str(&mut errors, &body, "name");
    let modelfile = optional_str(&mut errors, &body, "modelfile");
    // `from_model` carries `alias="from"`, and `populate_by_name` accepts both.
    let from_model = if body.get("from").is_some() {
        optional_str(&mut errors, &body, "from")
    } else {
        optional_str(&mut errors, &body, "from_model")
    };
    let system = optional_str(&mut errors, &body, "system");
    let quantize = optional_str(&mut errors, &body, "quantize");
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    let mut payload = json!({ "model": name, "stream": true });
    match modelfile.filter(|m| !m.is_empty()) {
        Some(modelfile) => payload["modelfile"] = json!(modelfile),
        None => {
            // Only the truthy ones are sent, which is what `if body.from_model:`
            // and its two siblings do.
            for (key, value) in
                [("from", from_model), ("system", system), ("quantize", quantize)]
            {
                if let Some(value) = value.filter(|v| !v.is_empty()) {
                    payload[key] = json!(value);
                }
            }
        }
    }

    let events = stream_ollama(&state, "create", payload).await.map_err(unavailable)?;
    let status = event_status(events.last());
    let ok = !events.is_empty() && status == "success";
    let message = if ok {
        format!("Created {name}")
    } else {
        let reported = if events.is_empty() { "unknown".to_string() } else { status };
        format!("Create finished with status {reported}")
    };
    Ok(Json(operation_out(ok, message, &events)).into_response())
}

async fn ollama_jobs_create(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    raw: Bytes,
) -> Result<Response, ApiError> {
    principal.require_scope("model:write")?;
    let body = parse_body(&raw)?;
    let mut errors = Vec::new();
    let operation = match body.get("operation") {
        None => {
            errors.push(ApiError::field_error("operation", "missing", "Field required"));
            String::new()
        }
        Some(Value::String(s)) if s == "pull" || s == "copy" => s.clone(),
        Some(_) => {
            errors.push(ApiError::field_error(
                "operation",
                "literal_error",
                "Input should be 'pull' or 'copy'",
            ));
            String::new()
        }
    };
    let name = optional_str(&mut errors, &body, "name");
    let source = optional_str(&mut errors, &body, "source");
    let destination = optional_str(&mut errors, &body, "destination");
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    let (job_type, payload) = match operation.as_str() {
        "pull" => {
            let name = name.filter(|n| !n.is_empty()).ok_or_else(|| {
                ApiError::bad_request("name is required for pull")
            })?;
            ("ollama_pull", json!({ "name": name }))
        }
        _ => {
            let source = source.filter(|s| !s.is_empty());
            let destination = destination.filter(|s| !s.is_empty());
            let (Some(source), Some(destination)) = (source, destination) else {
                return Err(ApiError::bad_request(
                    "source and destination are required for copy",
                ));
            };
            ("ollama_copy", json!({ "source": source, "destination": destination }))
        }
    };

    let job_id = create_ollama_job(&state, job_type, payload.clone()).await?;
    let out = job_out(&state, job_id).await?;
    spawn_ollama_job(state, job_id, job_type.to_string(), payload);
    Ok(Json(out).into_response())
}

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

async fn projects_list(
    State(state): State<Arc<AppState>>,
    principal: Principal,
) -> Result<Response, ApiError> {
    principal.require_scope("model:read")?;
    ensure_data_scaffold();
    let rows: Vec<ProjectRow> = sqlx::query_as(
        "SELECT id, name, description, manifest_json FROM model_projects ORDER BY name",
    )
    .fetch_all(&state.pool)
    .await?;

    let mut out = Vec::new();
    for row in &rows {
        // A row whose project has been deleted off disk is skipped, not 404'd.
        if load_project(&row.name).is_err() {
            continue;
        }
        out.push(project_to_out(&state, row).await?);
    }
    Ok(Json(json!({ "projects": out })).into_response())
}

async fn projects_create(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    raw: Bytes,
) -> Result<Response, ApiError> {
    principal.require_scope("model:write")?;
    let body = parse_body(&raw)?;
    let mut errors = Vec::new();
    let name = required_str(&mut errors, &body, "name");
    if body.get("name").is_some_and(Value::is_string) {
        let before = errors.len();
        crate::wire::check_len(&mut errors, &["name"], Some(name.as_str()), 1, 128);
        // `pattern=r"^[a-zA-Z0-9_-]+$"` — checked **only if the length passed**.
        // pydantic stops at the first failed constraint on a string, so `""` is
        // `string_too_short` alone and never also a pattern mismatch.
        if errors.len() == before
            && !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            errors.push(ApiError::field_error(
                "name",
                "string_pattern_mismatch",
                "String should match pattern '^[a-zA-Z0-9_-]+$'",
            ));
        }
    }
    let description = optional_str(&mut errors, &body, "description");
    let base_model = optional_str(&mut errors, &body, "base_model");
    let ollama_tag = optional_str(&mut errors, &body, "ollama_tag");
    if !errors.is_empty() {
        return Err(ApiError::validation(errors));
    }

    ensure_data_scaffold();
    let dest = projects_dir().join(&name);
    if dest.exists() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("Project already exists: {name}"),
        ));
    }
    let template = template_project_dir();
    if !template.is_dir() {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Template project missing; check model_ops/data/projects/_template",
        ));
    }
    copy_tree(&template, &dest).map_err(io_error)?;

    let manifest_path = dest.join("project.yaml");
    let raw_manifest = std::fs::read_to_string(&manifest_path).unwrap_or_default();
    let mut data = match serde_yaml::from_str::<Value>(&raw_manifest) {
        Ok(Value::Object(map)) => map,
        // `yaml.safe_load(f) or {}` — an empty or scalar manifest starts fresh.
        _ => Map::new(),
    };
    data.insert("name".into(), json!(name));
    for (key, value) in [
        ("description", &description),
        ("base_model", &base_model),
        ("ollama_tag", &ollama_tag),
    ] {
        if let Some(value) = value.as_ref().filter(|v| !v.is_empty()) {
            data.insert(key.into(), json!(value));
        }
    }
    let dumped = serde_yaml::to_string(&Value::Object(data.clone()))
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "An unexpected error occurred."))?;
    std::fs::write(&manifest_path, dumped).map_err(io_error)?;

    // The row's manifest is the patched file **without** the loader's
    // underscore keys — those only appear once something re-reads it.
    let now = sql_now();
    let description = description
        .filter(|d| !d.is_empty())
        .or_else(|| data.get("description").and_then(Value::as_str).map(str::to_string));
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO model_projects \
         (name, description, manifest_json, workspace_id, created_at, updated_at) \
         VALUES (?, ?, ?, NULL, ?, ?) RETURNING id",
    )
    .bind(&name)
    .bind(&description)
    .bind(Value::Object(data).to_string())
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.pool)
    .await?;

    let row = sqlx::query_as(
        "SELECT id, name, description, manifest_json FROM model_projects WHERE id = ?",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(project_to_out(&state, &row).await?).into_response())
}

async fn projects_get(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(name): PathId<String>,
) -> Result<Response, ApiError> {
    principal.require_scope("model:read")?;
    let row = sync_project_row(&state, &name).await?;
    Ok(Json(project_to_out(&state, &row).await?).into_response())
}

/// One `UploadFile`: its filename, the `path` part header if it carries one,
/// and the bytes.
struct Upload {
    filename: String,
    path_header: Option<String>,
    content: Vec<u8>,
}

/// The `files` parts of a multipart body, plus any plain text fields.
async fn read_multipart(
    mut multipart: Multipart,
) -> Result<(Vec<Upload>, Map<String, Value>), ApiError> {
    let mut uploads = Vec::new();
    let mut fields = Map::new();
    loop {
        let field = multipart.next_field().await.map_err(|e| {
            ApiError::bad_request(format!("There was an error parsing the body: {e}"))
        })?;
        let Some(field) = field else { break };
        let name = field.name().unwrap_or_default().to_string();
        let filename = field.file_name().map(str::to_string);
        let path_header =
            field.headers().get("path").and_then(|v| v.to_str().ok()).map(str::to_string);
        let bytes = field.bytes().await.map_err(|e| {
            ApiError::bad_request(format!("There was an error parsing the body: {e}"))
        })?;

        match filename {
            Some(filename) if name == "files" => uploads.push(Upload {
                filename,
                path_header,
                content: bytes.to_vec(),
            }),
            _ => {
                fields.insert(name, json!(String::from_utf8_lossy(&bytes)));
            }
        }
    }
    Ok((uploads, fields))
}

fn require_files(uploads: &[Upload]) -> Result<(), ApiError> {
    if uploads.is_empty() {
        return Err(ApiError::validation(vec![json!({
            "type": "missing", "loc": ["body", "files"], "msg": "Field required",
        })]));
    }
    Ok(())
}

async fn upload_knowledge(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(name): PathId<String>,
    multipart: Multipart,
) -> Result<Response, ApiError> {
    principal.require_scope("model:write")?;
    let (uploads, fields) = read_multipart(multipart).await?;
    require_files(&uploads)?;
    let pack_name = fields
        .get("pack_name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("uploads")
        .to_string();

    sync_project_row(&state, &name).await?;

    let dir = projects_dir().join(&name).join("knowledge").join(&pack_name);
    std::fs::create_dir_all(&dir).map_err(io_error)?;
    let mut count = 0;
    for upload in &uploads {
        let rel = if upload.filename.is_empty() { "upload.bin" } else { &upload.filename };
        // A bare filename is taken as a name; anything with a separator keeps
        // its shape, backslashes normalised. **No traversal guard** — that is
        // Python's behaviour here, and tightening it is a change, not a port.
        let safe = if rel.contains('/') || rel.contains('\\') {
            rel.replace('\\', "/")
        } else {
            Path::new(rel).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        };
        let dest = dir.join(&safe);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }
        std::fs::write(&dest, &upload.content).map_err(io_error)?;
        count += 1;
    }
    Ok(Json(json!({ "uploaded": count, "pack": pack_name })).into_response())
}

async fn upload_files(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(name): PathId<String>,
    multipart: Multipart,
) -> Result<Response, ApiError> {
    principal.require_scope("model:write")?;
    let (uploads, _fields) = read_multipart(multipart).await?;
    require_files(&uploads)?;

    sync_project_row(&state, &name).await?;

    let project_dir = projects_dir().join(&name);
    let mut count = 0;
    for upload in &uploads {
        // The `path` part header wins over the filename, which is how a client
        // uploads `datasets/train.jsonl` rather than `train.jsonl`.
        let rel = upload
            .path_header
            .clone()
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| {
                if upload.filename.is_empty() {
                    "upload.bin".to_string()
                } else {
                    upload.filename.clone()
                }
            });
        let safe = rel.replace('\\', "/").trim_start_matches('/').to_string();
        if safe.is_empty() || safe.starts_with("..") || format!("/{safe}/").contains("/../") {
            return Err(ApiError::bad_request(format!("Invalid project path: {rel}")));
        }
        let dest = project_dir.join(&safe);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }
        std::fs::write(&dest, &upload.content).map_err(io_error)?;
        count += 1;
    }
    Ok(Json(json!({ "uploaded": count })).into_response())
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

async fn registry_list(
    State(state): State<Arc<AppState>>,
    principal: Principal,
) -> Result<Response, ApiError> {
    principal.require_scope("model:read")?;
    let rows: Vec<RegistryRow> = sqlx::query_as(
        "SELECT id, project_id, version, ollama_tag, base_model, eval_score, is_active \
         FROM model_registry_entries ORDER BY created_at DESC",
    )
    .fetch_all(&state.pool)
    .await?;

    let mut entries = Vec::new();
    for row in &rows {
        let project_name: Option<String> =
            sqlx::query_scalar("SELECT name FROM model_projects WHERE id = ?")
                .bind(row.project_id)
                .fetch_optional(&state.pool)
                .await?;
        entries.push(row.to_out(project_name.as_deref()));
    }
    Ok(Json(json!({ "entries": entries })).into_response())
}

async fn registry_activate(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(entry_id): PathId<i64>,
) -> Result<Response, ApiError> {
    principal.require_scope("model:write")?;
    let row: RegistryRow = sqlx::query_as(
        "SELECT id, project_id, version, ollama_tag, base_model, eval_score, is_active \
         FROM model_registry_entries WHERE id = ?",
    )
    .bind(entry_id)
    .fetch_optional(&state.pool)
    .await?
    // `ValueError` there, and the route turns it into a 404.
    .ok_or_else(|| ApiError::not_found("Registry entry not found"))?;

    sqlx::query("UPDATE model_registry_entries SET is_active = 0 WHERE project_id = ?")
        .bind(row.project_id)
        .execute(&state.pool)
        .await?;
    sqlx::query("UPDATE model_registry_entries SET is_active = 1 WHERE id = ?")
        .bind(entry_id)
        .execute(&state.pool)
        .await?;

    let project_name: Option<String> =
        sqlx::query_scalar("SELECT name FROM model_projects WHERE id = ?")
            .bind(row.project_id)
            .fetch_optional(&state.pool)
            .await?;
    let mut out = row.to_out(project_name.as_deref());
    out["is_active"] = json!(true);
    Ok(Json(out).into_response())
}

// ---------------------------------------------------------------------------
// Pipeline jobs — `routes.py`'s five, and `runner.py` under them
// ---------------------------------------------------------------------------

/// `PipelineStage`, in the order `_stage_script` knows how to build.
const PIPELINE_STAGES: [&str; 4] = ["prepare", "train", "export", "eval"];

/// `POST /jobs` — create the row, answer, and run it detached, which is what
/// `BackgroundTasks` gave Python.
async fn jobs_create(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    raw: Bytes,
) -> Result<Response, ApiError> {
    principal.require_scope("model:write")?;
    let body = parse_body(&raw)?;
    let request = BuildRequest::parse(&body)?;
    let out = start_pipeline_job(state, &request).await?;
    Ok(Json(out).into_response())
}

/// `POST /operations/build` — the same thing under the reusable operation
/// contract, so an orchestrator step can start a build.
async fn build_operation(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    raw: Bytes,
) -> Result<Response, ApiError> {
    principal.require_scope("model:write")?;
    let body = parse_body(&raw)?;

    // `operation: str = Field(default="model.build")` — absent is the default,
    // present-but-wrong is a 400.
    match body.get("operation") {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) if s == "model.build" => {}
        Some(_) => {
            return Err(ApiError::bad_request("Unsupported operation; use model.build"));
        }
    }
    let input = body.get("input").cloned().ok_or_else(|| {
        ApiError::validation(vec![ApiError::field_error("input", "missing", "Field required")])
    })?;

    let request = BuildRequest::parse(&input).map_err(nest_under_input)?;
    let out = start_pipeline_job(state, &request).await?;
    Ok(Json(json!({
        "operation": "model.build",
        "job_id": out["id"],
        "poll_url": out["poll_url"],
        "stream_url": out["stream_url"],
    }))
    .into_response())
}

/// A validation failure on the nested body reports `["body", "input", …]`, the
/// way pydantic locates a field inside a sub-model.
fn nest_under_input(e: ApiError) -> ApiError {
    if e.code != "validation_error" {
        return e;
    }
    let Some(errors) = e.extra.as_ref().and_then(|x| x.get("errors")).and_then(Value::as_array)
    else {
        return e;
    };
    let nested: Vec<Value> = errors
        .iter()
        .cloned()
        .map(|mut entry| {
            if let Some(loc) = entry.get_mut("loc").and_then(Value::as_array_mut) {
                loc.insert(1, Value::from("input"));
            }
            entry
        })
        .collect();
    ApiError::validation(nested)
}

struct BuildRequest {
    project: String,
    stages: Vec<String>,
    register_alias: Option<String>,
    offline_eval: bool,
    process_id: Option<i64>,
}

impl BuildRequest {
    /// `ModelBuildJobCreateRequest`. `stages` defaults to all four, and an
    /// unknown stage is a 422 rather than a runtime `ValueError` — the enum is
    /// on the request model there, so it fails at the boundary.
    fn parse(body: &Value) -> Result<Self, ApiError> {
        let mut errors = Vec::new();
        let project = required_str(&mut errors, body, "project");

        let stages = match body.get("stages") {
            None | Some(Value::Null) => PIPELINE_STAGES.iter().map(|s| s.to_string()).collect(),
            Some(Value::Array(items)) => {
                let mut out = Vec::with_capacity(items.len());
                for (index, item) in items.iter().enumerate() {
                    match item.as_str() {
                        Some(name) if PIPELINE_STAGES.contains(&name) => out.push(name.to_string()),
                        _ => errors.push(ApiError::field_error_at(
                            &["stages", &index.to_string()],
                            "enum",
                            &format!(
                                "Input should be {}",
                                PIPELINE_STAGES
                                    .iter()
                                    .map(|s| format!("'{s}'"))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        )),
                    }
                }
                out
            }
            Some(_) => {
                errors.push(ApiError::field_error("stages", "list_type", "Input should be a valid list"));
                Vec::new()
            }
        };

        let register_alias = optional_str(&mut errors, body, "register_alias");
        let offline_eval = lax_bool(&mut errors, body, "offline_eval");
        let process_id = crate::wire::lax_int(&mut errors, body, "process_id");

        if !errors.is_empty() {
            return Err(ApiError::validation(errors));
        }
        Ok(Self {
            project,
            stages,
            register_alias: register_alias.filter(|a| !a.is_empty()),
            offline_eval,
            process_id,
        })
    }
}

/// The shared half of `model_jobs_create`: sync the project, create the row,
/// render the response, *then* spawn — so a client that polls the `poll_url` in
/// the response can never beat the row into existence.
async fn start_pipeline_job(state: Arc<AppState>, request: &BuildRequest) -> Result<Value, ApiError> {
    // `get_project_by_name` raising `FileNotFoundError` → 404. `load_project`
    // reads the manifest off disk and reports the same thing.
    let project = sync_project_row(&state, &request.project).await?;

    let logs_dir = ensure_data_scaffold().join("logs");
    std::fs::create_dir_all(&logs_dir).map_err(io_error)?;

    let now = sql_now();
    let job_id: i64 = sqlx::query_scalar(
        "INSERT INTO model_build_jobs \
         (project_id, job_type, operation_json, stages_json, status, current_stage, log_path, \
          result_json, register_alias, error_message, process_id, created_at, started_at, \
          finished_at) \
         VALUES (?, 'pipeline', NULL, ?, 'pending', NULL, NULL, NULL, ?, NULL, ?, ?, NULL, NULL) \
         RETURNING id",
    )
    .bind(project.id)
    .bind(Value::from(request.stages.clone()).to_string())
    .bind(request.register_alias.as_deref())
    .bind(request.process_id)
    .bind(&now)
    .fetch_one(&state.pool)
    .await?;

    let log_path = logs_dir.join(format!("job_{job_id}.log"));
    std::fs::write(&log_path, "").map_err(io_error)?;
    sqlx::query("UPDATE model_build_jobs SET log_path = ? WHERE id = ?")
        .bind(log_path.to_string_lossy().as_ref())
        .bind(job_id)
        .execute(&state.pool)
        .await?;

    // `link_process_to_job`: a job started by an orchestration step points back
    // at it. A missing process is silently skipped, as there.
    if let Some(process_id) = request.process_id {
        let _ = sqlx::query("UPDATE process SET model_build_job_id = ? WHERE id = ?")
            .bind(job_id)
            .bind(process_id)
            .execute(&state.pool)
            .await;
    }

    let out = job_out(&state, job_id).await?;
    spawn_pipeline_job(state, job_id, request.project.clone(), request.offline_eval);
    Ok(out)
}

async fn jobs_get(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(job_id): PathId<i64>,
) -> Result<Response, ApiError> {
    principal.require_scope("model:read")?;
    Ok(Json(job_out(&state, job_id).await?).into_response())
}

/// `POST /jobs/{id}/cancel`. Kills the stage subprocess if one is running, then
/// marks the row — in that order, so a stage that dies of the kill cannot
/// overwrite `cancelled` with `failed` afterwards (the runner checks).
async fn jobs_cancel(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(job_id): PathId<i64>,
) -> Result<Response, ApiError> {
    principal.require_scope("model:write")?;
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM model_build_jobs WHERE id = ?")
            .bind(job_id)
            .fetch_optional(&state.pool)
            .await?;
    let status = status.ok_or_else(|| ApiError::not_found("Job not found"))?;
    if status != "pending" && status != "running" {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("Job is {status}")));
    }

    // `_running.pop` + `proc.terminate()`. Taking it out of the map first is
    // what tells the runner's own arm that this was a cancellation.
    let handle = state.model_jobs.lock().ok().and_then(|mut map| map.remove(&job_id));
    if let Some(handle) = handle {
        handle.cancel();
    }

    sqlx::query("UPDATE model_build_jobs SET status = 'cancelled', finished_at = ? WHERE id = ?")
        .bind(sql_now())
        .bind(job_id)
        .execute(&state.pool)
        .await?;
    Ok(Json(job_out(&state, job_id).await?).into_response())
}

/// `GET /jobs/{id}/stream` — SSE over the job's log file.
///
/// The first frame carries the last 200 lines as context, and every frame after
/// it only what has been appended since, tracked by **byte** offset. A `done`
/// event closes the stream on a terminal status.
async fn jobs_stream(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    PathId(job_id): PathId<i64>,
) -> Result<Response, ApiError> {
    principal.require_scope("model:read")?;
    let job: JobRow = sqlx::query_as(&format!(
        "SELECT {JOB_COLUMNS} FROM model_build_jobs WHERE id = ?"
    ))
    .bind(job_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("Job not found"))?;

    let log_path = job.log_path.clone();
    let mut offset = log_path
        .as_deref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .unwrap_or(0);
    let mut first = read_job_log_tail(log_path.as_deref(), 200);

    let stream = async_stream::stream! {
        loop {
            let row: Option<JobRow> = sqlx::query_as(&format!(
                "SELECT {JOB_COLUMNS} FROM model_build_jobs WHERE id = ?"
            ))
            .bind(job_id)
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten();
            let Some(row) = row else { break };

            let chunk = if first.is_empty() {
                let (chunk, next) = read_job_log_since(log_path.as_deref(), offset);
                offset = next;
                chunk
            } else {
                std::mem::take(&mut first)
            };

            if !chunk.is_empty() {
                let payload = json!({
                    "log": chunk,
                    "status": row.status,
                    "stage": row.current_stage,
                });
                yield Ok::<_, std::convert::Infallible>(format!("event: log\ndata: {payload}\n\n"));
            }

            if matches!(row.status.as_str(), "succeeded" | "failed" | "cancelled") {
                let payload = json!({
                    "status": row.status,
                    "result": object_or_empty(row.result_json.as_deref()),
                });
                yield Ok(format!("event: done\ndata: {payload}\n\n"));
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    };

    Ok(Response::builder()
        .header("content-type", "text/event-stream")
        .body(axum::body::Body::from_stream(stream))
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "stream setup failed"))?)
}

/// `read_job_log_since` — bytes appended since `offset`, and the new offset.
///
/// Byte offsets, not string lengths: a tail window slides as the file grows, so
/// slicing one by a previous length re-sends or skips lines. Restarts from 0 if
/// the file shrank.
fn read_job_log_since(log_path: Option<&str>, offset: u64) -> (String, u64) {
    use std::io::{Read, Seek, SeekFrom};
    let Some(path) = log_path.filter(|p| !p.is_empty()) else { return (String::new(), offset) };
    let Ok(mut file) = std::fs::File::open(path) else { return (String::new(), offset) };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = if size < offset { 0 } else { offset };
    if file.seek(SeekFrom::Start(start)).is_err() {
        return (String::new(), offset);
    }
    let mut buffer = Vec::new();
    if file.read_to_end(&mut buffer).is_err() {
        return (String::new(), offset);
    }
    (String::from_utf8_lossy(&buffer).into_owned(), start + buffer.len() as u64)
}

// ---------------------------------------------------------------------------
// The runner
// ---------------------------------------------------------------------------

/// A live stage subprocess, so `/cancel` can reach it. `runner.py`'s `_running`
/// dict held the `asyncio` process object; a kill sender is all this needs.
pub struct JobHandle {
    cancel: tokio::sync::watch::Sender<bool>,
}

impl JobHandle {
    pub fn cancel(&self) {
        let _ = self.cancel.send(true);
    }
}

pub type JobMap = std::collections::HashMap<i64, Arc<JobHandle>>;

/// `run_job` for `job_type == "pipeline"`.
fn spawn_pipeline_job(state: Arc<AppState>, job_id: i64, project: String, offline_eval: bool) {
    let (tx, rx) = tokio::sync::watch::channel(false);
    if let Ok(mut map) = state.model_jobs.lock() {
        map.insert(job_id, Arc::new(JobHandle { cancel: tx }));
    }

    tokio::spawn(async move {
        let outcome = run_pipeline_job(&state, job_id, &project, offline_eval, rx).await;
        // Whether it finished or died, it is no longer cancellable.
        if let Ok(mut map) = state.model_jobs.lock() {
            map.remove(&job_id);
        }

        // A job the operator cancelled mid-stage is already `cancelled`; the
        // stage's non-zero exit must not overwrite that with `failed`.
        let current: Option<String> =
            sqlx::query_scalar("SELECT status FROM model_build_jobs WHERE id = ?")
                .bind(job_id)
                .fetch_optional(&state.pool)
                .await
                .ok()
                .flatten();
        if current.as_deref() == Some("cancelled") {
            return;
        }

        let log_path: Option<String> =
            sqlx::query_scalar("SELECT log_path FROM model_build_jobs WHERE id = ?")
                .bind(job_id)
                .fetch_optional(&state.pool)
                .await
                .ok()
                .flatten();

        match outcome {
            Ok(()) => {
                let _ = sqlx::query(
                    "UPDATE model_build_jobs SET status = 'succeeded', finished_at = ? WHERE id = ?",
                )
                .bind(sql_now())
                .bind(job_id)
                .execute(&state.pool)
                .await;
            }
            Err(message) => {
                append_job_log(log_path.as_deref(), &format!("ERROR: {message}\n"));
                let truncated: String = message.chars().take(2000).collect();
                let _ = sqlx::query(
                    "UPDATE model_build_jobs SET status = 'failed', finished_at = ?, \
                     error_message = ? WHERE id = ?",
                )
                .bind(sql_now())
                .bind(truncated)
                .bind(job_id)
                .execute(&state.pool)
                .await;
            }
        }
    });
}

async fn run_pipeline_job(
    state: &AppState,
    job_id: i64,
    project: &str,
    offline_eval: bool,
    cancelled: tokio::sync::watch::Receiver<bool>,
) -> Result<(), String> {
    let job: JobRow = sqlx::query_as(&format!(
        "SELECT {JOB_COLUMNS} FROM model_build_jobs WHERE id = ?"
    ))
    .bind(job_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| "Job vanished".to_string())?;
    let log_path = job.log_path.clone();

    sqlx::query("UPDATE model_build_jobs SET status = 'running', started_at = ? WHERE id = ?")
        .bind(sql_now())
        .bind(job_id)
        .execute(&state.pool)
        .await
        .map_err(|e| e.to_string())?;

    let stages: Vec<String> = job
        .stages()
        .as_array()
        .map(|items| items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    for stage in &stages {
        let stage = stage.as_str();
        let _ = sqlx::query("UPDATE model_build_jobs SET current_stage = ? WHERE id = ?")
            .bind(stage)
            .bind(job_id)
            .execute(&state.pool)
            .await;
        append_job_log(log_path.as_deref(), &format!("=== stage: {stage} ===\n"));

        let markers =
            run_stage(state, stage, project, offline_eval, log_path.as_deref(), cancelled.clone())
                .await?;

        // `eval` is the only stage that contributes to `result`, and it used to
        // do so by returning a dict. Now it prints one.
        if let Some(eval) = markers.eval {
            let mut result = object_or_empty(
                sqlx::query_scalar::<_, Option<String>>(
                    "SELECT result_json FROM model_build_jobs WHERE id = ?",
                )
                .bind(job_id)
                .fetch_optional(&state.pool)
                .await
                .ok()
                .flatten()
                .flatten()
                .as_deref(),
            );
            if let Some(map) = result.as_object_mut() {
                map.insert("eval".into(), eval);
            }
            let _ = sqlx::query("UPDATE model_build_jobs SET result_json = ? WHERE id = ?")
                .bind(result.to_string())
                .bind(job_id)
                .execute(&state.pool)
                .await;
        }
    }

    // `register_ollama_alias` after every stage succeeded, from the project's
    // manifest `ollama_tag` (its name when it has none).
    if let Some(alias) = job.register_alias.as_deref().filter(|a| !a.is_empty()) {
        let manifest: Option<String> =
            sqlx::query_scalar("SELECT manifest_json FROM model_projects WHERE id = ?")
                .bind(job.project_id)
                .fetch_optional(&state.pool)
                .await
                .ok()
                .flatten();
        let manifest = object_or_empty(manifest.as_deref());
        let tag = manifest
            .get("ollama_tag")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| project.to_string());
        if let Err(e) = register_ollama_alias(alias, &tag) {
            append_job_log(log_path.as_deref(), &format!("WARNING: alias not registered: {e}\n"));
        }
    }

    Ok(())
}

/// What a stage printed on its marker lines.
#[derive(Default)]
struct StageMarkers {
    eval: Option<Value>,
}

/// The `@@AGP:` prefix the Python worker prints its structured results on.
/// Chosen to be something no training library emits and no human would type.
const MARKER_PREFIX: &str = "@@AGP:";

/// Run one stage as a `MODEL_OPS_PYTHON -c …` child, teeing its combined output
/// to the job log and picking the marker lines out as they go past.
async fn run_stage(
    state: &AppState,
    stage: &str,
    project: &str,
    offline_eval: bool,
    log_path: Option<&str>,
    mut cancelled: tokio::sync::watch::Receiver<bool>,
) -> Result<StageMarkers, String> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let script = stage_script(stage, project, offline_eval)?;
    let mut command = tokio::process::Command::new(train_python());
    command
        .arg("-c")
        .arg(&script)
        .env("PYTHONPATH", worker_pythonpath())
        // Line-buffered, or a training run's output arrives in 8 KB blocks and
        // the log stream stalls for minutes at a time.
        .env("PYTHONUNBUFFERED", "1")
        // Passed explicitly rather than left to inheritance. Each of these has
        // a *resolved* value here — `CONFIG_DIR` may be unset in the
        // environment and still resolve to the repo's `data/llm`, and
        // `OLLAMA_API_BASE` may have come from the `.env` file or from startup
        // discovery. The worker's own fallbacks would land somewhere else, and
        // a training run that writes its adapters into the wrong directory
        // fails an hour later rather than immediately.
        .env("CONFIG_DIR", crate::llm_config::config_dir())
        .env("MODEL_OPS_DATA_DIR", data_dir())
        .env("OLLAMA_API_BASE", crate::llm_config::ollama_api_base())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = command.spawn().map_err(|e| {
        format!("Stage {stage} could not start ({}): {e}", train_python())
    })?;
    let stdout = child.stdout.take().ok_or_else(|| format!("Stage {stage} had no stdout"))?;
    let stderr = child.stderr.take();

    // stderr is merged into the log the way `stderr=STDOUT` merged it before,
    // but on its own task so a stage that writes a lot to one and nothing to
    // the other cannot deadlock on a full pipe.
    if let Some(stderr) = stderr {
        let log_path = log_path.map(str::to_string);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                append_job_log(log_path.as_deref(), &format!("{line}\n"));
            }
        });
    }

    let mut markers = StageMarkers::default();
    let mut lines = BufReader::new(stdout).lines();
    loop {
        tokio::select! {
            // Biased so a cancel that arrives while output is flowing is still
            // seen, rather than losing every poll to the ready stdout branch.
            biased;
            _ = cancelled.changed() => {
                if *cancelled.borrow() {
                    let _ = child.kill().await;
                    return Err(format!("Stage {stage} cancelled"));
                }
            }
            line = lines.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        append_job_log(log_path, &format!("{line}\n"));
                        handle_marker(state, &line, &mut markers).await;
                    }
                    _ => break,
                }
            }
        }
    }

    let status = child.wait().await.map_err(|e| format!("Stage {stage} failed to run: {e}"))?;
    if !status.success() {
        // `RuntimeError(f"Stage {stage} exited with code {code}")`. A signalled
        // process has no code on Unix, which Python rendered as a negative
        // number; `code()` is `None` there, so it is spelled out instead.
        return Err(match status.code() {
            Some(code) => format!("Stage {stage} exited with code {code}"),
            None => format!("Stage {stage} was terminated by a signal"),
        });
    }
    Ok(markers)
}

/// `@@AGP:eval@@ {json}` and `@@AGP:registry@@ {json}`. Anything else on a
/// marker line is ignored rather than fatal — a worker from a newer build must
/// not fail a job on a parent that does not know its marker yet.
async fn handle_marker(state: &AppState, line: &str, markers: &mut StageMarkers) {
    let Some(rest) = line.trim().strip_prefix(MARKER_PREFIX) else { return };
    let Some((kind, payload)) = rest.split_once("@@") else { return };
    let Ok(payload) = serde_json::from_str::<Value>(payload.trim()) else {
        logd!("model-ops: unparseable {kind} marker from the build worker");
        return;
    };
    match kind {
        "eval" => markers.eval = Some(payload),
        "registry" => {
            // `set_active` rides in the envelope; `register_model_entry`'s
            // second argument defaulted to True.
            let set_active = payload
                .get("set_active")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let entry = payload.get("entry").cloned().unwrap_or(payload.clone());
            if let Err(e) = persist_registry_entry(state, &entry, set_active).await {
                logd!("model-ops: registry entry not stored: {e:?}");
            }
        }
        _ => {}
    }
}

/// `persist_registry_entry`. Upserts on `(project_id, version)`.
async fn persist_registry_entry(
    state: &AppState,
    entry: &Value,
    set_active: bool,
) -> Result<(), ApiError> {
    // `entry.get("project") or entry.get("ollama_tag")` — no name, no write.
    let project_name = entry
        .get("project")
        .and_then(Value::as_str)
        .or_else(|| entry.get("ollama_tag").and_then(Value::as_str))
        .map(str::to_string);
    let Some(project_name) = project_name.filter(|n| !n.is_empty()) else { return Ok(()) };
    let project = sync_project_row(state, &project_name).await?;

    let version = entry
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("v1")
        .to_string();
    let tag = entry
        .get("ollama_tag")
        .and_then(Value::as_str)
        .unwrap_or(&project_name)
        .to_string();

    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM model_registry_entries WHERE project_id = ? AND version = ?",
    )
    .bind(project.id)
    .bind(&version)
    .fetch_optional(&state.pool)
    .await?;

    let entry_id = match existing {
        Some(id) => id,
        None => sqlx::query_scalar(
            "INSERT INTO model_registry_entries (project_id, version, ollama_tag, is_active) \
             VALUES (?, ?, ?, 0) RETURNING id",
        )
        .bind(project.id)
        .bind(&version)
        .bind(&tag)
        .fetch_one(&state.pool)
        .await?,
    };

    // `eval_score` is only written when present: `if entry.get("eval_score") is
    // not None`, so a later stage cannot blank an earlier one's number.
    sqlx::query(
        "UPDATE model_registry_entries \
         SET base_model = ?, adapter_path = ?, gguf_path = ?, metadata_json = ?, \
             eval_score = COALESCE(?, eval_score) \
         WHERE id = ?",
    )
    .bind(entry.get("base_model").and_then(Value::as_str))
    .bind(entry.get("adapter").and_then(Value::as_str))
    .bind(entry.get("gguf").and_then(Value::as_str))
    .bind(entry.to_string())
    .bind(entry.get("eval_score").and_then(Value::as_f64))
    .bind(entry_id)
    .execute(&state.pool)
    .await?;

    if set_active {
        sqlx::query("UPDATE model_registry_entries SET is_active = 0 WHERE project_id = ?")
            .bind(project.id)
            .execute(&state.pool)
            .await?;
        sqlx::query("UPDATE model_registry_entries SET is_active = 1, ollama_tag = ? WHERE id = ?")
            .bind(&tag)
            .bind(entry_id)
            .execute(&state.pool)
            .await?;
    }
    Ok(())
}

/// `config_bridge.register_ollama_alias`: add or update an alias in the ollama
/// provider block of `config.yaml`.
fn register_ollama_alias(alias: &str, ollama_model: &str) -> Result<(), String> {
    let path = crate::llm_config::config_yaml_path();
    let mut data: Value = match std::fs::read_to_string(&path) {
        Ok(text) => serde_yaml::from_str(&text).map_err(|e| e.to_string())?,
        Err(_) => json!({}),
    };
    if !data.is_object() {
        data = json!({});
    }
    let root = data.as_object_mut().expect("just forced to an object");

    let providers = root
        .entry("providers")
        .or_insert_with(|| json!([]));
    if !providers.is_array() {
        *providers = json!([]);
    }
    let providers = providers.as_array_mut().expect("just forced to an array");

    let index = providers
        .iter()
        .position(|p| p.get("name").and_then(Value::as_str) == Some("ollama"))
        .unwrap_or_else(|| {
            providers.push(json!({"name": "ollama", "models": []}));
            providers.len() - 1
        });
    let block = providers[index].as_object_mut().ok_or("ollama block is not a mapping")?;
    let models = block.entry("models").or_insert_with(|| json!([]));
    if !models.is_array() {
        *models = json!([]);
    }
    let models = models.as_array_mut().expect("just forced to an array");

    match models
        .iter_mut()
        .find(|m| m.get("model_name").and_then(Value::as_str) == Some(alias))
    {
        Some(found) => {
            found["model"] = Value::from(ollama_model);
        }
        None => models.push(json!({"model_name": alias, "model": ollama_model})),
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let rendered = serde_yaml::to_string(&data).map_err(|e| e.to_string())?;
    std::fs::write(&path, rendered).map_err(|e| e.to_string())
}

/// `_train_python`: `MODEL_OPS_PYTHON`, else whatever `python` is on PATH.
///
/// Python's fallback was `sys.executable` — the interpreter running the server,
/// which no longer exists. There is no better default left, so an install that
/// wants the build pipeline sets the variable; [`run_stage`] names it in the
/// error when the spawn fails, which is the only way an operator finds out.
fn train_python() -> String {
    env_opt("MODEL_OPS_PYTHON").unwrap_or_else(|| "python".to_string())
}

/// `_app_pythonpath`: the directory the `model_ops` package sits in.
///
/// `MODEL_OPS_WORKER_PATH` is the override; otherwise `worker/` beside the
/// executable (where the installer puts it), then `worker/` in the checkout
/// this was built from.
///
/// The checkout branch is not a nicety: a dev build runs out of
/// `target/debug/`, which has no `worker/` beside it, and without it every
/// build stage dies with `ModuleNotFoundError: No module named 'model_ops'` —
/// found by running one, not by reading. [`bundled_data_dir`] searches the same
/// three places for the same reason.
fn worker_pythonpath() -> String {
    let candidates = [
        env_opt("MODEL_OPS_WORKER_PATH").map(PathBuf::from),
        std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join("worker"))),
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .map(|repo| repo.join("worker")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|path| path.join("model_ops").is_dir())
        .map(|path| path.to_string_lossy().into_owned())
        // Nothing found: hand the relative name over anyway so the stage fails
        // with Python's own `ModuleNotFoundError` naming the package, which is
        // a better error than anything invented here.
        .unwrap_or_else(|| "worker".to_string())
}

/// `_stage_script` — the `python -c` body for one stage.
///
/// The project name is interpolated through a JSON string literal, which is a
/// valid Python string literal for every character that can appear here and
/// closes the injection the `{project!r}` in Python left open in principle.
fn stage_script(stage: &str, project: &str, offline_eval: bool) -> Result<String, String> {
    let name = Value::from(project).to_string();
    let body = match stage {
        "prepare" => format!(
            "from model_ops.pipeline.merge_knowledge import merge_packs\n\
             from model_ops.pipeline.build_dataset import build_dataset\n\
             merge_packs({name})\n\
             build_dataset({name})\n"
        ),
        "train" => format!("from model_ops.pipeline.train_lora import train\ntrain({name})\n"),
        "export" => format!(
            "from model_ops.pipeline.export_ollama import merge_and_export_gguf\n\
             merge_and_export_gguf({name})\n"
        ),
        // The one stage whose result travels: it printed nothing before, and
        // the parent read its return value.
        "eval" => format!(
            "import json\n\
             from model_ops.pipeline.eval import run_eval\n\
             _r = run_eval({name}, offline={})\n\
             print('{MARKER_PREFIX}eval@@ ' + json.dumps(_r if _r is not None else {{}}))\n",
            if offline_eval { "True" } else { "False" }
        ),
        other => return Err(format!("Unknown stage: {other}")),
    };
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stage scripts are strings compiled by another interpreter, so a typo
    /// here is a runtime failure minutes into a build. These assert the import
    /// lines and the marker the parent then parses.
    #[test]
    fn stage_scripts_name_the_pipeline_entry_points() {
        let prepare = stage_script("prepare", "my-app", false).unwrap();
        assert!(prepare.contains("merge_packs(\"my-app\")"), "{prepare}");
        assert!(prepare.contains("build_dataset(\"my-app\")"), "{prepare}");

        assert!(stage_script("train", "x", false).unwrap().contains("train(\"x\")"));
        assert!(stage_script("export", "x", false)
            .unwrap()
            .contains("merge_and_export_gguf(\"x\")"));

        let eval = stage_script("eval", "x", true).unwrap();
        assert!(eval.contains("run_eval(\"x\", offline=True)"), "{eval}");
        assert!(eval.contains("@@AGP:eval@@"), "{eval}");
        assert!(stage_script("eval", "x", false).unwrap().contains("offline=False"));

        assert_eq!(stage_script("nope", "x", false), Err("Unknown stage: nope".into()));
    }

    /// A project name with a quote in it must not break out of the literal.
    #[test]
    fn a_hostile_project_name_stays_inside_its_string() {
        let script = stage_script("train", "x\"); import os; os.system(\"rm -rf /", false).unwrap();
        assert!(script.contains(r#"train("x\"); import os; os.system(\"rm -rf /")"#), "{script}");
        // One `train(` call, so nothing was appended as a second statement.
        assert_eq!(script.matches("train(").count(), 1);
    }

    /// The parser has to ignore anything that is not a marker, because every
    /// line of a training run's output goes through it.
    #[test]
    fn only_marker_lines_are_parsed() {
        assert_eq!(split_marker("Epoch 1/3: loss=0.42"), None);
        assert_eq!(split_marker("@@AGP:eval@@ {\"score\": 1}"), Some(("eval", "{\"score\": 1}")));
        // Leading whitespace is tolerated; a marker with no `@@` terminator is not.
        assert_eq!(split_marker("   @@AGP:registry@@ {}"), Some(("registry", "{}")));
        assert_eq!(split_marker("@@AGP:broken {}"), None);
    }

    /// The half of `handle_marker` that has no database in it, so it can be
    /// tested without one.
    fn split_marker(line: &str) -> Option<(&str, &str)> {
        let rest = line.trim().strip_prefix(MARKER_PREFIX)?;
        let (kind, payload) = rest.split_once("@@")?;
        Some((kind, payload.trim()))
    }

    /// `read_job_log_since` is what makes the SSE stream not re-send what the
    /// client already has.
    #[test]
    fn the_log_stream_only_sends_what_is_new() {
        let dir = std::env::temp_dir().join("agp-model-ops-since");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("job.log");
        let as_str = path.to_string_lossy().into_owned();

        std::fs::write(&path, "one\ntwo\n").unwrap();
        let (chunk, offset) = read_job_log_since(Some(&as_str), 0);
        assert_eq!(chunk, "one\ntwo\n");
        assert_eq!(offset, 8);

        // Nothing appended, nothing sent, offset unchanged.
        assert_eq!(read_job_log_since(Some(&as_str), offset), (String::new(), offset));

        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        let (chunk, offset) = read_job_log_since(Some(&as_str), offset);
        assert_eq!(chunk, "three\n");
        assert_eq!(offset, 14);

        // A truncated file restarts from the beginning rather than reading past
        // its end forever.
        std::fs::write(&path, "new\n").unwrap();
        assert_eq!(read_job_log_since(Some(&as_str), offset), ("new\n".to_string(), 4));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn an_empty_pull_stream_still_counts_as_success() {
        // `not events or events[-1]["status"] in (...)` — a model that is
        // already present streams nothing at all.
        assert_eq!(event_status(None), "");
        assert_eq!(event_status(Some(&json!({"status": "success"}))), "success");
        assert_eq!(event_status(Some(&json!({}))), "");
    }

    #[test]
    fn only_the_last_twenty_events_come_back() {
        let events: Vec<Value> = (0..25).map(|i| json!({ "status": i })).collect();
        let tail = tail_events(&events);
        assert_eq!(tail.len(), 20);
        assert_eq!(tail[0]["status"], json!(5));
        assert_eq!(tail_events(&events[..3]).len(), 3);
    }

    #[test]
    fn a_bare_progress_line_becomes_a_status_event() {
        let mut events = Vec::new();
        push_event(&mut events, b"{\"status\":\"pulling\"}");
        push_event(&mut events, b"  ");
        push_event(&mut events, b"downloading 40%");
        assert_eq!(events, vec![json!({"status": "pulling"}), json!({"status": "downloading 40%"})]);
    }

    #[test]
    fn the_log_tail_is_read_from_the_end() {
        let dir = std::env::temp_dir().join("agp-model-ops-tail");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("job.log");
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        let tail = read_job_log_tail(Some(path.to_string_lossy().as_ref()), 2);
        assert_eq!(tail, "two\nthree");
        // A missing file is empty, never an error.
        assert_eq!(read_job_log_tail(Some("no-such-file.log"), 5), "");
        assert_eq!(read_job_log_tail(None, 5), "");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn the_async_alias_and_the_field_name_both_work() {
        let mut errors = Vec::new();
        assert!(async_flag(&mut errors, &json!({"async": true}), false));
        assert!(async_flag(&mut errors, &json!({"async_job": true}), false));
        // The alias wins when both are present, and the default stands when
        // neither is.
        assert!(!async_flag(&mut errors, &json!({"async": false, "async_job": true}), true));
        assert!(async_flag(&mut errors, &json!({}), true));
        assert!(errors.is_empty());
    }
}
