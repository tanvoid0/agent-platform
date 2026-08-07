//! `agent-platformd` — the platform API server (ADR 0007).
//!
//! Desktop-first: the iced app spawns this and it spawns Python. Headless: run it
//! directly, with `AGENT_PLATFORM_UPSTREAM` pointing at a Python server or
//! `AGENT_PLATFORM_PYTHON`/`AGENT_PLATFORM_PY_ENTRY` set so it starts its own.

use agent_platform_server::{dotenv, serve, Config};

/// Not `#[tokio::main]`: the environment is seeded from `.env` and the platform
/// YAML before the runtime exists, because `set_var` is only sound while no
/// other thread can be reading the environment.
fn main() {
    dotenv::load_env_files();

    let cfg = match Config::from_env() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("[agent-platformd] {e}");
            std::process::exit(2);
        }
    };

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("[agent-platformd] could not start the async runtime: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = runtime.block_on(serve(cfg)) {
        eprintln!("[agent-platformd] {e}");
        std::process::exit(1);
    }
}
