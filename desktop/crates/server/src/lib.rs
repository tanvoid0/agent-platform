//! Rust API server for agent-platform (ADR 0007).
//!
//! Binds the public port, handles the domains that have been migrated, and
//! reverse-proxies everything else to the Python server it spawns as a child.
//! No domain is migrated yet — slice 1 is the scaffold, the proxy, and auth.

pub mod auth;
pub mod error;
pub mod projects;
pub mod proxy;
pub mod upstream;

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
        let db_url = env_opt("DATABASE_URL");
        if let Some(url) = &db_url {
            return Err(format!(
                "DATABASE_URL is set ({url}), but agent-platformd supports SQLite only. \
                 Run the Python server directly, or unset DATABASE_URL."
            )
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
    pub master_key: Option<String>,
    pub upstream: Arc<upstream::Upstream>,
    pub http: reqwest::Client,
    /// Fixed-window per-token counters, mirroring `app/api_tokens/rate_limiter.py`.
    /// ponytail: in-process like the Python one; both count every request so the
    /// effective limit is unchanged. Needs a shared store if this ever runs N-up.
    pub windows: Mutex<HashMap<i64, (u64, u32)>>,
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
            .busy_timeout(std::time::Duration::from_secs(30));
        Self {
            pool: SqlitePoolOptions::new().connect_lazy_with(opts),
            master_key,
            upstream,
            http: reqwest::Client::new(),
            windows: Mutex::new(HashMap::new()),
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
        .merge(projects::routes())
        .fallback(proxy::forward)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_token,
        ))
        .with_state(state)
}

pub async fn serve(cfg: Config) -> Result<(), BoxError> {
    let up = Arc::new(upstream::start(&cfg).await?);
    let origin = up.origin.clone();
    let state = Arc::new(AppState::new(&cfg.db_path, cfg.master_key.clone(), up));

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
