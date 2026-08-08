//! The agent-platform API server (ADR 0007).
//!
//! Binds the public port and answers **everything**. It began as a reverse
//! proxy in front of a FastAPI server, moving one domain across at a time; that
//! server was deleted on 2026-08-07 and the fallback is now a 404. The only
//! subprocess this process ever starts is a model-ops build stage.
//!
//! The domains, and the modules that hold them:
//!
//! - **auth + tokens** — `auth.rs` (three tiers, `last_used_at`),
//!   `api_tokens.rs` (the workspace token CRUD)
//! - **processes** — `processes.rs` + `executor.rs` + `dag_schema.rs`: the
//!   eleven routes, the DAG executor, sub-DAG expansion, startup recovery
//! - **LLM proxy** — the whole `/v1` surface in `llm.rs` and its
//!   `llm_config`/`byok`/`model_catalog`/`model_capabilities`/`provider_catalog`/
//!   `upstream_http` satellites, plus the admin surface in `llm_admin.rs`
//!   (`config.yaml` validation in `config_schema.rs`)
//! - **assistant** — `assistant.rs` + `assistant_turn.rs` + `clarifying_form.rs`
//! - **coder** — `coder.rs` + `coder_loop.rs` + `coder_tools.rs`: all ten
//!   routes, the agent loop, both executors, the delegated tool park
//! - **chat** — `chat.rs` + `chat_usage.rs` + `chat_thread_title.rs` +
//!   `context_budget.rs`
//! - **todos** — `todos.rs`, with the agent routes and the decision engine in
//!   `action_orchestrator.rs`
//! - **workflows** — `workflows.rs` + `workflow_engine.rs` and its scheduler
//! - **projects, teams** — `projects.rs`, `teams.rs`
//! - **workspaces + documents** — `workspaces.rs`, `workspace_files.rs`,
//!   `documents.rs` (upload ingest and PDF extraction)
//! - **model ops** — `model_ops.rs`: all seventeen routes, including the build
//!   pipeline, whose stages run as subprocesses against `worker/`
//! - **status + logs** — `system.rs` over the ring in `observability.rs`
//!
//! Cross-cutting: `db.rs` (the SQLite/Postgres choke point, and the schema
//! bootstrap that replaced Alembic), `wire.rs` and `error.rs` (shared shapes
//! and the FastAPI-compatible error envelope), `request_id.rs`, `dotenv.rs`.
//!
//! `plan.md`'s "Rust server migration" section is the live source of truth for
//! what changed behaviour when the Python server went — this comment is the
//! map, not the manifest.

/// Write one diagnostic line to stderr **and** the ring `GET /system/logs`
/// serves. Takes `format!` arguments and adds the `[agent-platformd] ` prefix,
/// so it is a drop-in for the `eprintln!` calls it replaced and the console
/// output is byte-identical to what it was.
///
/// Defined here rather than in [`observability`] because `macro_rules!` is
/// textually scoped: a macro is only visible to modules declared *after* it,
/// and every module below uses this one.
#[macro_export]
macro_rules! logd {
    ($($arg:tt)*) => {
        $crate::observability::diagnostic(&format!($($arg)*))
    };
}

pub mod action_orchestrator;
pub mod api_tokens;
pub mod assistant;
pub mod coder;
pub mod coder_loop;
pub mod coder_tools;
pub mod assistant_turn;
pub mod auth;
pub mod byok;
pub mod chat;
pub mod chat_thread_title;
pub mod chat_usage;
pub mod clarifying_form;
pub mod config_schema;
pub mod context_budget;
pub mod dag_schema;
pub mod db;
pub mod documents;
pub mod dotenv;
pub mod error;
pub mod executor;
pub mod llm;
pub mod llm_admin;
pub mod llm_config;
pub mod model_capabilities;
pub mod model_catalog;
pub mod model_ops;
pub mod observability;
pub mod processes;
pub mod projects;
pub mod provider_catalog;
pub mod request_id;
pub mod system;
pub mod teams;
pub mod todos;
pub mod upstream_http;
pub mod workflow_engine;
pub mod usage;
pub mod wire;
pub mod workflows;
pub mod workspace_files;
pub mod workspaces;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::json;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Everything the server reads from the environment. The desktop shell sets
/// these when it spawns the daemon; a headless run sets them itself.
#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    /// `None` means auth is fully open — the dev convenience the Python server
    /// had when `AGENT_PLATFORM_MASTER_KEY` was unset, kept so a local run that
    /// never set the variable still works.
    pub master_key: Option<String>,
    pub db_path: PathBuf,
}

impl Config {
    pub fn from_env() -> Result<Self, BoxError> {
        // SQLite-only until the `sqlx::Any` conversion finishes (see `AppState`):
        // half the domains still query `pool`, which is a `SqlitePool`, so a
        // Postgres DSN would be honoured by some queries and ignored by others.
        // Refusing beats a server answering from two databases.
        // The value is not echoed: a DSN carries the password, and this line goes
        // to stderr and into the log ring.
        if env_opt("DATABASE_URL").is_some() {
            return Err("DATABASE_URL is set, but agent-platformd supports SQLite only. \
                        Run the Python server directly, or unset DATABASE_URL."
                .into());
        }

        let host = env_opt("AGENT_PLATFORM_HOST").unwrap_or_else(|| "127.0.0.1".into());
        let master_key = env_opt("AGENT_PLATFORM_MASTER_KEY");

        // Open auth is a *loopback* convenience. Off the loopback it is an open
        // server, and nothing about the startup output said so.
        if master_key.is_none() && !is_loopback(&host) && env_opt("AGENT_PLATFORM_ALLOW_OPEN").is_none() {
            return Err(format!(
                "AGENT_PLATFORM_HOST={host} binds beyond the loopback with no \
                 AGENT_PLATFORM_MASTER_KEY set, which serves every route to anyone who can \
                 reach the port. Set a master key, or set AGENT_PLATFORM_ALLOW_OPEN=1 if that \
                 is deliberate."
            )
            .into());
        }

        Ok(Self {
            host,
            port: env_opt("AGENT_PLATFORM_PORT")
                .map(|s| s.parse::<u16>())
                .transpose()?
                .unwrap_or(18410),
            master_key,
            db_path: env_opt("AGENT_PLATFORM_DB_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("data/agent_platform.db")),
        })
    }
}

/// Whether a bind host is reachable only from this machine.
///
/// String-matched rather than parsed, because the two forms that are not
/// addresses at all (`localhost`, and the empty host some launchers pass) have
/// to pass too. Anything unrecognised is treated as exposed: guessing wrong in
/// that direction refuses a startup, guessing wrong the other way opens a server.
fn is_loopback(host: &str) -> bool {
    let h = host.trim().trim_start_matches('[').trim_end_matches(']');
    if h.is_empty() || h.eq_ignore_ascii_case("localhost") {
        return true;
    }
    h.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

pub fn env_opt(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
        _ => None,
    }
}

pub struct AppState {
    pub pool: SqlitePool,
    /// The backend-agnostic pool the domains are being moved onto, one at a
    /// time. Both are open at once *on purpose*: switching `pool`'s type breaks
    /// all 166 query sites in the same commit, and this repo has another
    /// migration in flight through the same files. A converted domain reads
    /// `any`; an unconverted one still reads `pool`; the day the last one moves,
    /// `pool` is deleted and `Config::from_env` stops refusing `DATABASE_URL`.
    ///
    /// Until then Postgres stays refused, so there is never a half-ported server
    /// answering from two databases.
    pub any: sqlx::AnyPool,
    pub backend: db::Backend,
    pub master_key: Option<String>,
    pub http: reqwest::Client,
    /// Fixed-window per-token counters, mirroring `app/api_tokens/rate_limiter.py`.
    /// ponytail: in-process like the Python one; both count every request so the
    /// effective limit is unchanged. Needs a shared store if this ever runs N-up.
    pub windows: Mutex<HashMap<i64, (u64, u32)>>,
    /// Coder turns parked on a delegated tool call, keyed by
    /// `(thread_id, call_id)` — `coder/desktop_executor.py`'s module-level
    /// `_pending` dict. **This is process memory**: `/chat/tool-result` must be
    /// served by the same process that served `/chat/stream`, which is why the
    /// coder loop routes moved in one commit. See [`coder_tools`].
    pub coder_pending: Mutex<coder_tools::PendingMap>,
    /// Live model-build stage subprocesses, keyed by job id — `runner.py`'s
    /// module-level `_running` dict. Process memory, and that is now sound
    /// rather than a blocker: this is the only process that starts one, so
    /// `POST /jobs/{id}/cancel` can always reach the child it needs to kill.
    pub model_jobs: Mutex<model_ops::JobMap>,
    /// Local model lists, refreshed in the background so `/v1/health` never waits
    /// on a backend. Empty until `serve` starts the refresh loop, which is what a
    /// test harness gets — and what Python reports for its own first 30 seconds.
    pub catalog: Arc<model_catalog::CatalogCache>,
}

impl AppState {
    /// Connects lazily, and **creates the file if it is not there**.
    ///
    /// It used to refuse to: the database was Alembic's, created by the Python
    /// child on its first start, and a Rust process that made an empty file
    /// first would have given that child a database with no schema in it and
    /// no way to notice. Nothing else creates it now — `serve` calls
    /// [`db::ensure_schema`] straight after this — so refusing would mean a
    /// fresh install has no database at all.
    pub fn new(
        db_path: &std::path::Path,
        master_key: Option<String>,
    ) -> Self {
        let opts = SqliteConnectOptions::new()
            .filename(db_path)
            .create_if_missing(true)
            // sqlx turns foreign keys ON per connection; SQLAlchemy left SQLite's
            // default OFF, so this data has never been FK-enforced in its life
            // and does not satisfy the constraints the schema declares.
            // Measured, not assumed: `PRAGMA foreign_key_check` on a real user
            // database returns 55 violations, all `eventlog.task_id` pointing at
            // tasknodes a finished DAG deleted. Turning this on is a migration
            // that has to rebuild those tables with `ON DELETE` actions and
            // clean the orphans first — see `db::connect_lazy`.
            .foreign_keys(false)
            .busy_timeout(std::time::Duration::from_secs(30));
        let url = db::url_for(db_path, None);
        let backend = db::Backend::from_url(&url);
        Self {
            pool: SqlitePoolOptions::new().connect_lazy_with(opts),
            any: db::connect_lazy(&url, backend),
            backend,
            master_key,
            http: reqwest::Client::new(),
            windows: Mutex::new(HashMap::new()),
            coder_pending: Mutex::new(HashMap::new()),
            model_jobs: Mutex::new(HashMap::new()),
            catalog: Arc::new(model_catalog::CatalogCache::default()),
        }
    }
}

/// `{"status": "ok"}` — the desktop's liveness probe and its attach-if-running
/// check.
///
/// This used to answer from the Python child's liveness, and could report
/// `down` for a daemon that was itself perfectly healthy. There is no child any
/// more — but "this handler runs" is not the same as "this server can answer",
/// because every route past `/health` needs the database. A process whose
/// SQLite file has been deleted, locked or filled the disk kept reporting `ok`
/// here and 500ing everywhere else, which is the one failure a liveness probe
/// exists to catch. So it touches the database.
///
/// The query is `SELECT 1` on the pool, not a table read: it proves a
/// connection can be opened, which is what fails, without depending on any
/// schema this check would then have to be kept in step with.
///
/// It runs on `pool` rather than `any` because `pool` is the one carrying
/// `busy_timeout(30s)`. The desktop polls this endpoint to decide whether to
/// adopt a running server or start its own, and a `SQLITE_BUSY` returned
/// immediately would read as "that server is dead" and spawn a second one
/// against the same file.
async fn health(State(state): State<Arc<AppState>>) -> Response {
    match sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(&state.pool).await {
        Ok(_) => Json(json!({"status": "ok", "service": "agent-platform"})).into_response(),
        Err(e) => {
            logd!("health check could not reach the database: {e}");
            // 503, so a container orchestrator and the desktop's
            // attach-if-running check both read it as "not ready" rather than
            // as a healthy server that happens to fail every request.
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "status": "error",
                    "service": "agent-platform",
                    "detail": "database unavailable",
                })),
            )
                .into_response()
        }
    }
}

/// What the proxy fallback became. Shaped like every other error this server
/// emits — `ApiError`'s envelope, not axum's empty body — because a client that
/// mistypes a path gets a JSON error from every *other* status code and should
/// not have to special-case this one.
async fn not_found(req: Request) -> Response {
    error::ApiError::not_found(format!("No route for {} {}", req.method(), req.uri().path()))
        .into_response()
}

/// `GET /openapi.json` — the API reference the desktop's Settings → API screen
/// renders, and the only machine-readable description of this server.
///
/// **It is a checked-in file, not generated.** FastAPI produced it from the
/// route declarations while the server was Python; axum cannot enumerate its
/// own router, so keeping it means either annotating 141 paths with `utoipa` or
/// maintaining the document. The document won, for now — every route in it
/// answers byte-identically to what Python declared (ADR 0007 rule 5), so it
/// was accurate on the day the Python server was deleted.
///
/// ponytail: **it will drift**, and nothing detects that. Adding a route means
/// editing `openapi.json` by hand and there is no test that fails if you
/// forget — the honest fix is `utoipa` annotations, worth doing the first time
/// a stale entry actually misleads someone. Unauthenticated, as FastAPI served
/// it: it describes the surface, it does not expose data.
async fn openapi() -> Response {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        include_str!("openapi.json"),
    )
        .into_response()
}

async fn root() -> Response {
    // `docs` pointed at FastAPI's Swagger page, which no longer exists. The
    // machine-readable document does, and is what any client actually wants.
    Json(json!({"service": "agent-platform", "api": "/api/v1", "openapi": "/openapi.json"}))
        .into_response()
}

/// `AGENT_PLATFORM_CORS_ORIGINS` — comma-separated origins allowed to call this
/// server from a browser. Unset means no CORS layer at all, which is what a
/// loopback-only install wants: same-origin and server-to-server callers never
/// send `Origin`, so the layer would only ever be overhead.
///
/// Origins are explicit on purpose — no wildcard. `Access-Control-Allow-Origin: *`
/// plus a `Bearer agp_…` token is how a token leaks to whatever page the user
/// has open. An unparseable entry is dropped with a log line rather than
/// failing startup, because a typo in one origin should not take the server down.
fn cors_layer() -> Option<tower_http::cors::CorsLayer> {
    let raw = env_opt("AGENT_PLATFORM_CORS_ORIGINS")?;
    let origins: Vec<axum::http::HeaderValue> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| match s.parse() {
            Ok(v) => Some(v),
            Err(_) => {
                logd!("[cors] ignoring unparseable origin {s:?}");
                None
            }
        })
        .collect();
    if origins.is_empty() {
        logd!("[cors] AGENT_PLATFORM_CORS_ORIGINS held no usable origin; CORS off");
        return None;
    }
    logd!("[cors] allowing {} origin(s)", origins.len());
    Some(
        tower_http::cors::CorsLayer::new()
            .allow_origin(origins)
            .allow_methods(tower_http::cors::Any)
            .allow_headers(tower_http::cors::Any),
    )
}

/// How many bytes a request body may carry before the route decides otherwise.
///
/// **axum's own default is 2 MB, and it was the only cap this server had.**
/// Starlette had none, so the port quietly introduced a ceiling nothing
/// documents: a chat turn carrying a large document context, or any upload past
/// 2 MB, gets a 413 with no hint that a limit exists. This raises the general
/// cap to something a JSON body will never reach honestly, and the upload
/// routes raise it again for themselves (see [`upload_body_limit`]).
///
/// A cap has to exist because every handler here reads the body into memory
/// before it looks at it — `Json<T>`, and `read_multipart`'s `Vec<u8>` per
/// part. Unlimited means one request can decide how much RAM this process uses.
pub fn json_body_limit() -> usize {
    megabytes("AGENT_PLATFORM_MAX_BODY_MB", 16)
}

/// The cap for the four multipart upload routes, which is the same ceiling for
/// a different reason: a LoRA adapter or a training set is legitimately large,
/// and `read_multipart` still buffers every part.
///
/// ponytail: one number for all of them, and it is a whole-body limit rather
/// than a per-file one. Split it if a route ever needs its own.
pub fn upload_body_limit() -> usize {
    megabytes("AGENT_PLATFORM_MAX_UPLOAD_MB", 512)
}

fn megabytes(var: &str, default_mb: usize) -> usize {
    env_opt(var)
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|mb| *mb > 0)
        .unwrap_or(default_mb)
        .saturating_mul(1024 * 1024)
}

/// The whole surface. There is no fallback any more — an unknown path is a 404
/// from [`not_found`], where it used to be forwarded to Python.
///
/// Auth stays a layer rather than per-route: it applies to exactly the prefix
/// `app/main.py` guarded with `_api_deps`, and putting it here keeps that one
/// decision in one place rather than on forty route declarations.
pub fn router(state: Arc<AppState>) -> Router {
    let router = Router::new()
        .route("/health", axum::routing::get(health))
        .route("/", axum::routing::get(root))
        .route("/openapi.json", axum::routing::get(openapi))
        .merge(action_orchestrator::routes())
        .merge(api_tokens::routes())
        .merge(assistant::routes())
        .merge(coder::routes())
        .merge(chat::routes())
        .merge(llm::routes())
        .merge(llm_admin::routes())
        .merge(model_ops::routes())
        .merge(processes::routes())
        .merge(system::routes())
        .merge(projects::routes())
        .merge(teams::routes())
        .merge(todos::routes())
        .merge(workflows::routes())
        .merge(workspace_files::routes())
        .merge(workspaces::routes())
        .fallback(not_found)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_token,
        ))
        // The general cap. A route that needs more sets its own *closer to the
        // handler*, which is what makes it win: both layers write the same
        // request extension, and the inner one writes last.
        .layer(axum::extract::DefaultBodyLimit::max(json_body_limit()))
        // A body that arrives one byte at a time holds a connection and a task
        // open for as long as the sender likes, and the body-size cap does not
        // help — the bytes never add up. Request body only: a response timeout
        // here would cut every SSE stream this server serves at the same mark.
        .layer(tower_http::timeout::RequestBodyTimeoutLayer::new(
            std::time::Duration::from_secs(60),
        ))
        // Outermost, so an auth rejection carries a correlation id too.
        .layer(axum::middleware::from_fn(request_id::middleware))
        .with_state(state);

    // Outside both: a preflight `OPTIONS` carries no `Authorization` header, so
    // it has to be answered before `require_token` sees it and 401s.
    match cors_layer() {
        Some(cors) => router.layer(cors),
        None => router,
    }
}

pub async fn serve(cfg: Config) -> Result<(), BoxError> {
    // Before anything slow, so `uptime_seconds` measures the server and not the
    // time it spent waiting for a backend to answer.
    system::mark_started();
    let state = Arc::new(AppState::new(&cfg.db_path, cfg.master_key.clone()));
    // Before anything queries: Alembic used to do this from the Python child,
    // and there is no child. See `db::ensure_schema` for what it can and cannot
    // do (it creates; it does not migrate).
    if let Some(parent) = cfg.db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    db::ensure_schema(&state.any).await?;
    state.catalog.clone().spawn_refresh(state.http.clone());
    workflow_engine::spawn_scheduler(state.clone());
    // Executors are in-process tasks and do not survive a restart, so whatever
    // the last shutdown interrupted is requeued here. `AGENT_PLATFORM_RESUME_ON_STARTUP=0`
    // still switches it off — it was there so the Python child would not recover
    // the same rows this process was recovering, and it stays as the operator
    // control for "start without replaying anything".
    executor::spawn_startup_recovery(state.clone());

    let addr: SocketAddr = format!("{}:{}", cfg.host, cfg.port).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    logd!("listening on http://{addr}");

    // After the bind, on its own task: a `VACUUM INTO` of a large database
    // should not be the reason the port is late, and nothing about it has to
    // finish before the first request.
    {
        let state = state.clone();
        let db_path = cfg.db_path.clone();
        tokio::spawn(async move { db::backup(&state.any, &db_path).await });
    }

    axum::serve(listener, router(state.clone()))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // Past the graceful drain, so nothing is writing. The `-wal` sidecar is
    // never truncated on its own and only grows; this is the one moment it can
    // be folded back in for free.
    db::checkpoint(&state.any).await;

    Ok(())
}

/// Resolves on the first shutdown request the platform can send.
///
/// **SIGTERM is the one that matters and was missing.** `docker stop`,
/// `systemctl stop` and a Kubernetes pod eviction all send SIGTERM and then
/// SIGKILL after a grace period; a server listening only for Ctrl-C ignored the
/// polite one and was killed by the second, dropping every in-flight SSE
/// stream, DAG executor step and model-build stage mid-write. Ctrl-C stays
/// because that is how a developer stops it in a terminal.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        // A failure to install the handler must not become a server that
        // cannot be stopped: fall back to Ctrl-C alone.
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                logd!("could not listen for SIGTERM, Ctrl-C only: {e}");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    // Windows: `ctrl_c` covers Ctrl-C, Ctrl-Break and the console close button.
    // A `taskkill /F` is not catchable by anything, here or in Python.
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;

    logd!("shutdown signal received; draining in-flight requests");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard in [`Config::from_env`] turns on this predicate, and getting it
    /// wrong in the permissive direction publishes an unauthenticated server.
    #[test]
    fn only_loopback_hosts_read_as_loopback() {
        for host in ["127.0.0.1", "127.9.9.9", "localhost", "LOCALHOST", "::1", "[::1]", " 127.0.0.1 ", ""] {
            assert!(is_loopback(host), "{host:?} is loopback");
        }
        for host in ["0.0.0.0", "192.168.1.10", "::", "[::]", "example.com", "10.0.0.1"] {
            assert!(!is_loopback(host), "{host:?} is not loopback");
        }
    }
}
