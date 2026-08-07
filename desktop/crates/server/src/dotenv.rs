//! Seed the process environment the way the Python server does, before anything
//! reads it.
//!
//! `app/database.py` calls `load_dotenv(<root>/.env)` at import time and then
//! `apply_platform_yaml_defaults()`, so by the time any Python code reads
//! `os.environ` it holds the union of the shell, the repo's `.env`, and the
//! `env:` block of `config/agent_platform.yaml`. The daemon inherited only the
//! shell, and every value it *missed* was one where the two halves disagreed:
//!
//! - `AGENT_PLATFORM_MASTER_KEY` in `.env` meant Python required a bearer token
//!   while Rust, seeing no key, left auth wide open in front of it.
//! - `DATABASE_URL` in `.env` meant Python ran on Postgres while Rust read the
//!   default SQLite file, and the guard in `Config::from_env` that exists to
//!   refuse exactly that could never see the variable that triggers it.
//! - Provider keys (`AIMLAPI_API_KEY`, `SPEECH_API_BASE`, …) meant the two
//!   servers answered `/v1/capabilities` differently for the same request.
//!
//! Precedence, highest first: the real environment, `<root>/.env`, then the YAML
//! `env:` block — the same order `load_dotenv` and `setdefault` produce, both of
//! which only fill in keys that are absent.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::env_opt;
use crate::llm_config::parse_env_text;

/// Never taken from YAML: `config/agent_platform.yaml` is committed, and these
/// belong in `.env` or the real environment only. Mirrors `_SECRET_ENV_KEYS`.
const YAML_SECRET_KEYS: [&str; 3] =
    ["AGENT_PLATFORM_MASTER_KEY", "GEMINI_API_KEY", "LM_STUDIO_API_KEY"];

/// Apply both files to the process environment.
///
/// Call this once, first thing in `main`, before the async runtime exists:
/// `set_var` is only sound while nothing else can be reading the environment.
pub fn load_env_files() {
    let root = server_root();

    if let Some(root) = &root {
        let path = root.join(".env");
        let applied = apply_missing(&parse_file(&path), &[]);
        if applied > 0 {
            eprintln!("[agent-platformd] loaded {applied} var(s) from {}", path.display());
        }
    }

    let yaml = env_opt("AGENT_PLATFORM_CONFIG_YAML")
        .map(PathBuf::from)
        .or_else(|| root.map(|r| r.join("config").join("agent_platform.yaml")));
    if let Some(path) = yaml {
        let applied = apply_missing(&yaml_env_map(&path), &YAML_SECRET_KEYS);
        if applied > 0 {
            eprintln!("[agent-platformd] loaded {applied} default(s) from {}", path.display());
        }
    }
}

/// The directory holding `app/`, `scripts/` and `.env` — the same one
/// `app/database.py` resolves relative to itself. Same search order as
/// `upstream::resolve_python`, which finds `scripts/start.py` under it.
fn server_root() -> Option<PathBuf> {
    if let Some(entry) = env_opt("AGENT_PLATFORM_PY_ENTRY") {
        let entry = PathBuf::from(entry);
        if let Some(root) = entry.parent().and_then(Path::parent) {
            return Some(root.to_path_buf());
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(root) = exe.parent().map(|p| p.join("server")) {
            if root.join("scripts").join("start.py").is_file() {
                return Some(root);
            }
        }
    }

    // crates/server -> crates -> desktop -> repo root
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?.parent()?;
    repo.join("scripts").join("start.py").is_file().then(|| repo.to_path_buf())
}

/// Set every key that is not already present. Presence, not emptiness: an
/// explicitly empty shell variable shadows the file in Python too.
fn apply_missing(values: &HashMap<String, String>, skip: &[&str]) -> usize {
    let mut applied = 0;
    for (key, value) in values {
        if key.is_empty() || skip.contains(&key.as_str()) {
            continue;
        }
        if std::env::var_os(key).is_none() {
            std::env::set_var(key, value);
            applied += 1;
        }
    }
    applied
}

/// ponytail: the platform's own `.env` parser, not python-dotenv's. It handles
/// `KEY=value`, `#` comments and one layer of quotes — no `export `, no variable
/// expansion, no multi-line values. Matches every `.env` this repo ships; widen
/// it the day one of those appears rather than porting a whole dotenv dialect.
fn parse_file(path: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(path).map(|raw| parse_env_text(&raw)).unwrap_or_default()
}

/// The `env:` block of `config/agent_platform.yaml`, stringified the way
/// `_stringify_env_value` does it.
fn yaml_env_map(path: &Path) -> HashMap<String, String> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    let Ok(doc) = serde_yaml::from_str::<Value>(&raw) else {
        return HashMap::new();
    };
    let Some(Value::Object(block)) = doc.get("env") else {
        return HashMap::new();
    };
    block
        .iter()
        .filter(|(key, _)| !key.trim().is_empty())
        .map(|(key, value)| (key.trim().to_string(), env_value(value)))
        .collect()
}

fn env_value(value: &Value) -> String {
    match value {
        Value::Bool(true) => "1".into(),
        Value::Bool(false) => "0".into(),
        Value::Null => String::new(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.trim().to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deliberately never calls `load_env_files`: it would resolve this repo and
    /// pull the real `.env` into every other test in the binary.
    #[test]
    fn yaml_block_is_stringified_and_only_fills_gaps() {
        let dir = std::env::temp_dir().join("agp-dotenv-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent_platform.yaml");
        std::fs::write(
            &path,
            "version: 1\nenv:\n  AGP_TEST_STR: \"  spaced  \"\n  AGP_TEST_INT: 600\n\
             \x20 AGP_TEST_BOOL: true\n  AGP_TEST_NULL:\n  AGENT_PLATFORM_MASTER_KEY: leaked\n",
        )
        .unwrap();

        let map = yaml_env_map(&path);
        assert_eq!(map["AGP_TEST_STR"], "spaced");
        assert_eq!(map["AGP_TEST_INT"], "600");
        assert_eq!(map["AGP_TEST_BOOL"], "1");
        assert_eq!(map["AGP_TEST_NULL"], "");

        // A key already in the environment is never overwritten, and the secret
        // list is never applied from YAML at all.
        std::env::set_var("AGP_TEST_STR", "from-shell");
        std::env::remove_var("AGENT_PLATFORM_MASTER_KEY");
        let applied = apply_missing(&map, &YAML_SECRET_KEYS);
        assert_eq!(applied, 3, "only the three absent non-secret keys");
        assert_eq!(std::env::var("AGP_TEST_STR").unwrap(), "from-shell");
        assert_eq!(std::env::var("AGP_TEST_INT").unwrap(), "600");
        assert!(std::env::var_os("AGENT_PLATFORM_MASTER_KEY").is_none());

        for key in ["AGP_TEST_STR", "AGP_TEST_INT", "AGP_TEST_BOOL", "AGP_TEST_NULL"] {
            std::env::remove_var(key);
        }
        assert!(yaml_env_map(&dir.join("missing.yaml")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
