//! `GET /system/status` and `GET /system/logs`, ported from
//! `app/system_routes.py` (and the `health_checks.py` it fans in).
//!
//! ADR 0007 put this domain **last** on the grounds that it is an aggregator —
//! it reports facts the other domains own, so migrating it early would have
//! meant calling back into Python for most of the body. That reasoning held:
//! every field below now reads from a Rust source that exists because the
//! domain in front of it already moved.
//!
//! **Two fields could not survive the retirement of the Python server, and are
//! renamed rather than faked.** `python` was `sys.version.split()[0]`; there is
//! no interpreter in this process to ask, and reporting the *training*
//! interpreter (`MODEL_OPS_PYTHON`, the one thing still spawned — see
//! [`crate::model_ops`]) would have meant a subprocess on a route the Status
//! screen polls every few seconds. It is now `server`: this crate's version,
//! which is what an operator reading a row labelled "Server" actually wants.
//! `platform` stays, with a coarser value — see [`platform_name`].
//!
//! The desktop's `SystemStatus` and the Status screen were updated in the same
//! commit; nothing else parses this.

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::error::ApiError;
use crate::llm_config::first_configured_provider;
use crate::{db, env_opt, AppState};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/system/status", get(system_status))
        .route("/api/v1/system/logs", get(system_logs))
}

/// `_STARTED_AT`. Process start, so uptime is the server's own and not the
/// host's — stamped by [`mark_started`] from `serve`, and lazily on first read
/// so a test that builds a router without serving reports 0 rather than panics.
fn started_at() -> Instant {
    *STARTED_AT.get_or_init(Instant::now)
}

static STARTED_AT: OnceLock<Instant> = OnceLock::new();

/// Called once from `serve`, before the listener binds.
pub fn mark_started() {
    let _ = STARTED_AT.set(Instant::now());
}

/// Runs a status page should surface as "there is work in flight".
/// `approval_required` and `task_review_required` are in here deliberately:
/// they are stalled on a human, which is the thing a monitoring screen exists
/// to make visible.
const ACTIVE_STATUSES: [&str; 6] = [
    "pending",
    "planning",
    "approval_required",
    "approved",
    "task_review_required",
    "running",
];

async fn system_status(State(state): State<Arc<AppState>>) -> Result<Response, ApiError> {
    let (app_ok, app_checks) = app_readiness(&state).await;
    let (proxy_ok, proxy_checks) = llm_proxy_readiness();
    let counts = process_counts(&state).await?;

    let active: i64 = counts
        .iter()
        .filter(|(status, _)| ACTIVE_STATUSES.contains(&status.as_str()))
        .map(|(_, n)| *n)
        .sum();
    let total: i64 = counts.iter().map(|(_, n)| *n).sum();
    let by_status: Map<String, Value> =
        counts.into_iter().map(|(status, n)| (status, Value::from(n))).collect();

    Ok(Json(json!({
        "service": "agent-platform",
        "env": env_opt("AGENT_PLATFORM_ENV")
            .unwrap_or_else(|| "development".into())
            .trim()
            .to_lowercase(),
        // Python rounded to one decimal place; `f64` division does not, so the
        // rounding is explicit or the field grows fifteen digits of noise.
        "uptime_seconds": (started_at().elapsed().as_secs_f64() * 10.0).round() / 10.0,
        "server": env!("CARGO_PKG_VERSION"),
        "platform": platform_name(),
        // The address callers use, which is not our bind address when something
        // fronts us. Kept reading the `PUBLIC_*` pair first even though this
        // process is now the outermost one: a reverse proxy in a cloud
        // deployment is exactly the case it was written for.
        "listening_on": {
            "host": env_opt("AGENT_PLATFORM_PUBLIC_HOST")
                .or_else(|| env_opt("AGENT_PLATFORM_HOST"))
                .unwrap_or_else(|| "127.0.0.1".into()),
            "port": env_opt("AGENT_PLATFORM_PUBLIC_PORT")
                .or_else(|| env_opt("AGENT_PLATFORM_PORT"))
                .and_then(|raw| raw.parse::<u16>().ok())
                .unwrap_or(18410),
        },
        "auth_required": state.master_key.is_some(),
        "readiness": {
            "ok": app_ok,
            "status": if app_ok { "ok" } else { "unready" },
            "checks": app_checks,
        },
        "llm_proxy": {
            "ok": proxy_ok,
            "status": if proxy_ok { "ok" } else { "unready" },
            "checks": proxy_checks,
        },
        "processes": { "by_status": by_status, "active": active, "total": total },
        "paths": paths(&state),
    }))
    .into_response())
}

#[derive(Deserialize)]
struct LogsQuery {
    #[serde(default)]
    after: u64,
}

/// Recent server log lines, one JSON object per line, newest last.
///
/// Poll with the `next` from the previous response to get only what has been
/// written since. The desktop shell has its own copy of this taken from the
/// process's stdout — that one covers startup and crashes, which this cannot,
/// because this only answers while the server is running.
async fn system_logs(Query(q): Query<LogsQuery>) -> Response {
    Json(crate::observability::snapshot(q.after)).into_response()
}

// ---------------------------------------------------------------------------
// The fan-in — `health_checks.py`
// ---------------------------------------------------------------------------

/// `app_readiness_payload`: the database answers, and the workspace root is
/// usable. Returns `(ok, checks)` rather than Python's `(status_code, payload)`
/// — the 503 half of that tuple was for `/ready`, a route that is `llm.rs`'s.
async fn app_readiness(state: &AppState) -> (bool, Vec<Value>) {
    let database = match sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(&state.any).await {
        Ok(_) => check("database", true, "database reachable"),
        Err(e) => check("database", false, &format!("database check failed: {e}")),
    };

    // `workspace_root()` creates the directory and swallows the error; the
    // check is whether it is there afterwards, which is the same question
    // Python's `try/except` around `mkdir` answered.
    let root = crate::workspace_files::workspace_root();
    let workspace = if root.is_dir() {
        check("workspace_root", true, &format!("workspace root ready at {}", root.display()))
    } else {
        check(
            "workspace_root",
            false,
            &format!("workspace root unavailable: {} is not a directory", root.display()),
        )
    };

    let ok = database["ok"] == json!(true) && workspace["ok"] == json!(true);
    (ok, vec![database, workspace])
}

/// `llm_proxy_readiness_payload`.
///
/// **Always ok**, and that is Python's behaviour too rather than a shortcut
/// taken here: `first_configured_provider` ends in an unconditional
/// `return "lm_studio"`, so the `if provider:` guard it is tested against can
/// never be false and the 503 branch below it is unreachable. `llm.rs`'s
/// `/v1/health/readiness` carries the same note.
fn llm_proxy_readiness() -> (bool, Vec<Value>) {
    let provider = first_configured_provider();
    (true, vec![check("provider_config", true, &format!("default provider can resolve to {provider}"))])
}

fn check(name: &str, ok: bool, detail: &str) -> Value {
    json!({ "name": name, "ok": ok, "detail": detail })
}

/// `_process_counts` — `GROUP BY status`, so a status with no rows is absent
/// rather than zero.
async fn process_counts(state: &AppState) -> Result<Vec<(String, i64)>, ApiError> {
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT status, COUNT(*) FROM process GROUP BY status")
            .fetch_all(&state.any)
            .await?;
    Ok(rows)
}

/// `_paths`.
fn paths(state: &AppState) -> Value {
    let db_path =
        env_opt("AGENT_PLATFORM_DB_PATH").unwrap_or_else(|| "data/agent_platform.db".into());
    json!({
        "database": db_path,
        "database_backend": match state.backend {
            db::Backend::Sqlite => "sqlite",
            db::Backend::Postgres => "postgresql",
        },
        "workspaces": crate::workspace_files::workspace_root().display().to_string(),
        "llm_config_dir": env_opt("CONFIG_DIR"),
        "model_ops_data": env_opt("MODEL_OPS_DATA_DIR"),
    })
}

/// `platform.platform()`'s replacement.
///
/// ponytail: OS and architecture only — `windows-x86_64` where Python said
/// `Windows-11-10.0.26200-SP0`. The build number needs a per-OS call
/// (`RtlGetVersion`, `uname`, `sw_vers`) or the `os_info` crate; this row is a
/// diagnostic label on the Status screen, and neither the extra dependency nor
/// three blocks of `unsafe` earn their keep for it. Swap in `os_info` if a
/// support conversation ever turns on the build number.
fn platform_name() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The counting is the only logic in `/status` that can be wrong quietly:
    /// a status missing from `ACTIVE_STATUSES` makes a stalled process look
    /// idle on the one screen that exists to show it is not.
    #[test]
    fn active_counts_the_human_gates() {
        let counts = vec![
            ("running".to_string(), 2_i64),
            ("approval_required".to_string(), 1),
            ("task_review_required".to_string(), 3),
            ("completed".to_string(), 10),
            ("failed".to_string(), 4),
        ];
        let active: i64 = counts
            .iter()
            .filter(|(s, _)| ACTIVE_STATUSES.contains(&s.as_str()))
            .map(|(_, n)| *n)
            .sum();
        let total: i64 = counts.iter().map(|(_, n)| *n).sum();
        assert_eq!(active, 6, "the two gated statuses count as in flight");
        assert_eq!(total, 20);
    }

    #[test]
    fn platform_is_not_empty() {
        let name = platform_name();
        assert!(name.contains('-'), "{name}");
        assert!(!name.starts_with('-') && !name.ends_with('-'), "{name}");
    }
}
