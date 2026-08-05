//! `agent-platformd` — the platform API server (ADR 0007).
//!
//! Desktop-first: the iced app spawns this and it spawns Python. Headless: run it
//! directly, with `AGENT_PLATFORM_UPSTREAM` pointing at a Python server or
//! `AGENT_PLATFORM_PYTHON`/`AGENT_PLATFORM_PY_ENTRY` set so it starts its own.

use agent_platform_server::{serve, Config};

#[tokio::main]
async fn main() {
    let cfg = match Config::from_env() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("[agent-platformd] {e}");
            std::process::exit(2);
        }
    };

    if let Err(e) = serve(cfg).await {
        eprintln!("[agent-platformd] {e}");
        std::process::exit(1);
    }
}
