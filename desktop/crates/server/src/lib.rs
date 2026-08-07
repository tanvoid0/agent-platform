//! Rust API server for agent-platform (ADR 0007).
//!
//! Binds the public port, handles the domains that have been migrated, and
//! reverse-proxies everything else to the Python server it spawns as a child.
//! Whole domains: auth, `/health`, `/`, projects, teams, todos (`todos.rs` +
//! `action_orchestrator.rs`), workflows (`workflows.rs` + `workflow_engine.rs`),
//! processes (`processes.rs` + `executor.rs` + `dag_schema.rs`), and the
//! embedded LLM proxy's whole `/v1` surface (`llm.rs` and its
//! `llm_config`/`byok`/`model_catalog`/`model_capabilities`/`provider_catalog`/
//! `upstream_http` satellites). Partial: assistant (`assistant.rs` +
//! `assistant_turn.rs` + `clarifying_form.rs` — reads, the profile write, and
//! chat's context-usage/thread/send; the rest still proxied) and `chat.rs`
//! (`POST /api/v1/chat` alone). Untouched: coder, playground, `system_routes`,
//! the workspace/document stack. `plan.md`'s "Rust server migration" section
//! is the live source of truth for exactly which routes, kept current there
//! rather than duplicated here — this comment is the map, not the manifest.

pub mod action_orchestrator;
pub mod assistant;
pub mod coder;
pub mod assistant_turn;
pub mod auth;
pub mod byok;
pub mod chat;
pub mod chat_thread_title;
pub mod chat_usage;
pub mod clarifying_form;
pub mod context_budget;
pub mod dag_schema;
pub mod db;
pub mod dotenv;
pub mod error;
pub mod executor;
pub mod llm;
pub mod llm_config;
pub mod model_capabilities;
pub mod model_catalog;
pub mod processes;
pub mod projects;
pub mod provider_catalog;
pub mod proxy;
pub mod request_id;
pub mod teams;
pub mod todos;
pub mod upstream;
pub mod upstream_http;
pub mod workflow_engine;
pub mod usage;
pub mod wire;
pub mod workflows;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::json;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Everything the server reads from the environment. The desktop shell already
/// sets these for the Python child, so the daemon reads the same names and
/// passes them straight through to it.
#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    /// `None` means auth is fully open — the same dev convenience the Python
    /// server has when `AGENT_PLATFORM_MASTER_KEY` is unset.
    pub master_key: Option<String>,
    pub db_path: PathBuf,
    /// Set to talk to a Python server someone else is running; leave unset to
    /// have the daemon spawn and own one.
    pub upstream: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self, BoxError> {
        // The Rust side is SQLite-only until the cloud deployment needs Postgres
        // (ADR 0007). Starting anyway would point this process at a different
        // database than the Python child it proxies to, which is worse than
        // refusing.
        // The value is not echoed: a DSN carries the password, and this line goes
        // to stderr and into the desktop's log ring.
        if env_opt("DATABASE_URL").is_some() {
            return Err("DATABASE_URL is set, but agent-platformd supports SQLite only. \
                        Run the Python server directly, or unset DATABASE_URL."
                .into());
        }

        Ok(Self {
            host: env_opt("AGENT_PLATFORM_HOST").unwrap_or_else(|| "127.0.0.1".into()),
            port: env_opt("AGENT_PLATFORM_PORT")
                .map(|s| s.parse::<u16>())
                .transpose()?
                .unwrap_or(18410),
            master_key: env_opt("AGENT_PLATFORM_MASTER_KEY"),
            db_path: env_opt("AGENT_PLATFORM_DB_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("data/agent_platform.db")),
            upstream: env_opt("AGENT_PLATFORM_UPSTREAM"),
        })
    }
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
    pub upstream: Arc<upstream::Upstream>,
    pub http: reqwest::Client,
    /// Fixed-window per-token counters, mirroring `app/api_tokens/rate_limiter.py`.
    /// ponytail: in-process like the Python one; both count every request so the
    /// effective limit is unchanged. Needs a shared store if this ever runs N-up.
    pub windows: Mutex<HashMap<i64, (u64, u32)>>,
    /// Local model lists, refreshed in the background so `/v1/health` never waits
    /// on a backend. Empty until `serve` starts the refresh loop, which is what a
    /// test harness gets — and what Python reports for its own first 30 seconds.
    pub catalog: Arc<model_catalog::CatalogCache>,
}

impl AppState {
    /// Connects lazily: the schema is Alembic's, created by the Python child on
    /// its first start, which may not have happened yet when we build state.
    pub fn new(
        db_path: &std::path::Path,
        master_key: Option<String>,
        upstream: Arc<upstream::Upstream>,
    ) -> Self {
        let opts = SqliteConnectOptions::new()
            .filename(db_path)
            .create_if_missing(false)
            // sqlx turns foreign keys ON per connection; SQLAlchemy leaves SQLite's
            // default OFF. With them on, deleting a board that still has items is
            // a 500 here and a 204 there — the schema has FKs the data does not
            // honour. Matching Python is the contract; tightening this is a
            // migration, not a port.
            .foreign_keys(false)
            .busy_timeout(std::time::Duration::from_secs(30));
        let url = db::url_for(db_path, None);
        let backend = db::Backend::from_url(&url);
        Self {
            pool: SqlitePoolOptions::new().connect_lazy_with(opts),
            any: db::connect_lazy(&url, backend),
            backend,
            master_key,
            upstream,
            http: reqwest::Client::new(),
            windows: Mutex::new(HashMap::new()),
            catalog: Arc::new(model_catalog::CatalogCache::default()),
        }
    }
}

/// `{"status": "ok"}` only while the server behind us is actually alive.
///
/// This is the desktop's liveness probe and its attach-if-running check, so
/// answering for a dead child would make the Status screen claim a server that
/// is not there. When we do not own the child we cannot tell, so we ask it.
async fn health(State(state): State<Arc<AppState>>, req: Request) -> Response {
    match state.upstream.child_alive() {
        Some(true) => Json(json!({"status": "ok", "service": "agent-platform"})).into_response(),
        Some(false) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status": "down", "service": "agent-platform"})),
        )
            .into_response(),
        None => proxy::forward(State(state), req).await,
    }
}

async fn root() -> Response {
    Json(json!({"service": "agent-platform", "api": "/api/v1", "docs": "/docs"})).into_response()
}

/// The whole surface: migrated routes plus the proxy fallback.
///
/// Auth runs as a layer rather than per-route because it has to cover the
/// fallback too, and it applies to exactly the prefix `app/main.py` guards with
/// `_api_deps`.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", axum::routing::get(health))
        .route("/", axum::routing::get(root))
        .merge(assistant::routes())
        .merge(coder::routes())
        .merge(chat::routes())
        .merge(llm::routes())
        .merge(processes::routes())
        .merge(projects::routes())
        .merge(teams::routes())
        .merge(todos::routes())
        .merge(workflows::routes())
        .fallback(proxy::forward)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_token,
        ))
        // Outermost, so an auth rejection is stamped too and the id reaches the
        // proxied request rather than being added after it has already gone.
        .layer(axum::middleware::from_fn(request_id::middleware))
        .with_state(state)
}

pub async fn serve(cfg: Config) -> Result<(), BoxError> {
    let up = Arc::new(upstream::start(&cfg).await?);
    let origin = up.origin.clone();
    let state = Arc::new(AppState::new(&cfg.db_path, cfg.master_key.clone(), up));
    state.catalog.clone().spawn_refresh(state.http.clone());
    workflow_engine::spawn_scheduler(state.clone());
    // Executors are in-process tasks and do not survive a restart, so whatever the
    // last shutdown interrupted is requeued here. The Python child is started with
    // `AGENT_PLATFORM_RESUME_ON_STARTUP=0` for the same reason the scheduler is
    // switched off there: two servers recovering means every stranded process is
    // planned twice.
    executor::spawn_startup_recovery(state.clone());

    let addr: SocketAddr = format!("{}:{}", cfg.host, cfg.port).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("[agent-platformd] listening on http://{addr} → {origin}");

    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;

    // `serve` returning drops the router, the state, and with it the Upstream,
    // whose Drop kills the Python child.
    Ok(())
}
