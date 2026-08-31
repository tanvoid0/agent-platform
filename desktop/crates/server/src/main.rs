//! `agent-platformd` — the platform API server (ADR 0007).
//!
//! Desktop-first: the iced app spawns this. Headless: run it directly — it is
//! self-contained, and the cloud artifact ADR 0007 aimed at. It spawned a
//! Python child until every domain had migrated; now the only subprocess it
//! ever starts is a model-ops build stage.

use agent_platform_server::{dotenv, logd, serve, Config};

/// Not `#[tokio::main]`: the environment is seeded from `.env` and the platform
/// YAML before the runtime exists, because `set_var` is only sound while no
/// other thread can be reading the environment.
fn main() {
    // Before the read: a `.env` that PowerShell's `>>` left as UTF-16 parses
    // either way, but it stays broken until something rewrites it.
    dotenv::repair_env_encoding();
    dotenv::load_env_files();

    let cfg = match Config::from_env() {
        Ok(cfg) => cfg,
        Err(e) => {
            logd!("{e}");
            std::process::exit(2);
        }
    };

    let args: Vec<String> = std::env::args().collect();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            logd!("could not start the async runtime: {e}");
            std::process::exit(1);
        }
    };

    if matches!(
        args.get(1).map(String::as_str),
        Some("grant-comp" | "set-entitlement" | "revoke-sessions" | "migrate")
    ) {
        if let Err(e) = runtime.block_on(agent_platform_server::cli::run(&args, &cfg)) {
            logd!("{e}");
            std::process::exit(1);
        }
        return;
    }

    if let Err(e) = runtime.block_on(serve(cfg)) {
        logd!("{e}");
        std::process::exit(1);
    }
}
