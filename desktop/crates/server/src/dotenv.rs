//! Seed the process environment from the repo's `.env` and platform YAML,
//! before anything reads it.
//!
//! `app/database.py` used to do this at import time — `load_dotenv(<root>/.env)`
//! then `apply_platform_yaml_defaults()` — so `os.environ` held the union of
//! the shell, the `.env`, and the `env:` block of
//! `config/agent_platform.yaml`. The daemon inherited only the shell, and every
//! value it *missed* was one where the two halves disagreed:
//!
//! - `AGENT_PLATFORM_MASTER_KEY` in `.env` meant Python required a bearer token
//!   while Rust, seeing no key, left auth wide open in front of it.
//! - `DATABASE_URL` in `.env` meant Python ran on Postgres while Rust read the
//!   default SQLite file, and the guard in `Config::from_env` that exists to
//!   refuse exactly that could never see the variable that triggers it.
//! - Provider keys (`AIMLAPI_API_KEY`, `SPEECH_API_BASE`, …) meant the two
//!   servers answered `/v1/capabilities` differently for the same request.
//!
//! Those three failures are gone with the Python half, but the loading is not
//! a compatibility shim — it is now the only thing that reads either file, and
//! an operator who puts a master key in `.env` still expects it to be read.
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
/// belong in `.env` or the real environment only.
///
/// This is `llm_admin`'s masking list, not a copy of it. A key that `GET /env`
/// hides is a key that must not arrive from a committed file, and the two lists
/// spelled separately had already drifted — the admin surface masked
/// `AIMLAPI_API_KEY` and `ANTHROPIC_API_KEY` while the copy here still let the
/// YAML supply them.
use crate::llm_admin::SENSITIVE_ENV_KEYS as YAML_SECRET_KEYS;

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
            logd!("loaded {applied} var(s) from {}", path.display());
        }
    }

    let yaml = env_opt("AGENT_PLATFORM_CONFIG_YAML")
        .map(PathBuf::from)
        .or_else(|| root.map(|r| r.join("config").join("agent_platform.yaml")));
    if let Some(path) = yaml {
        let applied = apply_missing(&yaml_env_map(&path), &YAML_SECRET_KEYS);
        if applied > 0 {
            logd!("loaded {applied} default(s) from {}", path.display());
        }
    }
}

/// The directory holding `.env` and `config/`.
///
/// Keyed on `config/agent_platform.yaml`, which is committed and is one of the
/// two files this module reads. It used to look for `scripts/start.py` — the
/// Python entry point — and check an installed `<exe dir>/server` before the
/// checkout; neither exists now, so both are gone and the marker is a file that
/// is still there.
fn server_root() -> Option<PathBuf> {
    if let Some(explicit) = env_opt("AGENT_PLATFORM_ROOT") {
        return Some(PathBuf::from(explicit));
    }

    // Beside the installed executable first, then the checkout this was built
    // from — a dev run has no files next to `target/debug/`.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if dir.join("config").join("agent_platform.yaml").is_file() {
                return Some(dir.to_path_buf());
            }
        }
    }

    // crates/server -> crates -> desktop -> repo root
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?.parent()?;
    repo.join("config").join("agent_platform.yaml").is_file().then(|| repo.to_path_buf())
}

/// Set every key that is not already present. Presence, not emptiness: an
/// explicitly empty shell variable shadows the file in Python too.
fn apply_missing(values: &HashMap<String, String>, skip: &[&str]) -> usize {
    let mut applied = 0;
    for (key, value) in values {
        if key.is_empty() || skip.contains(&key.as_str()) {
            continue;
        }
        // `set_var` panics on a NUL in either half, which turns a mis-encoded
        // file into a dead server. This is the only site that calls it, so the
        // guard belongs here rather than in each parser.
        if key.contains('\0') || value.contains('\0') {
            logd!("skipping env var with NUL bytes in its name or value");
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
    let Ok(bytes) = std::fs::read(path) else {
        return HashMap::new();
    };
    // Lossy, not `read_to_string`: a UTF-16 file with any non-ASCII character
    // in it is not valid UTF-8, and returning an empty map there would lose the
    // operator's whole configuration silently. Read-only on purpose - the
    // repair is `repair_env_encoding`, which only `main` calls.
    parse_env_text(&String::from_utf8_lossy(&bytes))
}

/// Rewrite a mis-encoded `.env` as UTF-8, once, before anything reads it.
///
/// PowerShell's `>>` appends UTF-16LE, so a `.env` extended that way is UTF-8
/// down to the last redirect and NUL-interleaved after it. Warning about that
/// on every boot fixed nothing - the file stayed broken and the message became
/// scenery - so the daemon now repairs the file. The original is kept beside it
/// the first time, because this is the operator's credentials and a bad guess
/// must not be the only copy.
///
/// **Called from `main` only, never from a read path.** It writes to the
/// operator's `.env`, and `tests/postgres_schema.rs` calls `load_env_files` to
/// find `DATABASE_URL` - a test run must not edit that file.
pub fn repair_env_encoding() {
    let Some(root) = server_root() else { return };
    repair_file(&root.join(".env"));
}

/// ponytail: repair is "drop the NULs and BOMs", not a real UTF-16 decode. It
/// is exactly right for the ASCII these files hold and for the mixed-encoding
/// shape `>>` produces, which a whole-file UTF-16 decode would mangle. A `.env`
/// with non-ASCII in it needs `encoding_rs`; nothing here has one.
fn repair_file(path: &Path) {
    let Ok(bytes) = std::fs::read(path) else { return };
    let raw = String::from_utf8_lossy(&bytes);
    if !raw.contains('\0') && !raw.contains('\u{feff}') {
        return;
    }
    let backup = path.with_extension("utf16.bak");
    if !backup.exists() {
        if let Err(e) = std::fs::write(&backup, &bytes) {
            // No backup, no rewrite. The NULs are stripped at parse time
            // anyway, which is the old behaviour and is not worth risking a
            // lost `.env` to improve on.
            logd!("{} is mis-encoded but could not be backed up ({e}); leaving it alone", path.display());
            return;
        }
    }
    match std::fs::write(path, raw.replace(['\0', '\u{feff}'], "").as_bytes()) {
        Ok(()) => logd!(
            "{} was UTF-16 (PowerShell `>>` does this); rewrote it as UTF-8, original at {}",
            path.display(),
            backup.display()
        ),
        Err(e) => logd!("{} is mis-encoded and could not be rewritten ({e})", path.display()),
    }
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

    /// A `.env` that PowerShell's `>>` appended to: UTF-8 head, UTF-16LE tail,
    /// which `read_to_string` hands back as NUL-interleaved text. It has to
    /// parse, and a NUL must never reach `set_var` — that panics, which used to
    /// take the whole server down at startup.
    #[test]
    fn nul_bearing_env_file_parses_and_never_reaches_set_var() {
        let dir = std::env::temp_dir().join("agp-dotenv-nul-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".env");
        let utf16: String =
            "AGP_TEST_NUL=utf16\n".chars().flat_map(|c| [c, '\0']).collect();
        std::fs::write(&path, format!("AGP_TEST_PLAIN=utf8\n{utf16}")).unwrap();

        let map = parse_file(&path);
        assert_eq!(map["AGP_TEST_PLAIN"], "utf8");
        assert_eq!(map["AGP_TEST_NUL"], "utf16");

        // Repair is a separate, explicit step: reading must never write.
        assert!(std::fs::read(&path).unwrap().contains(&0), "parse_file rewrote the file");

        repair_file(&path);
        let on_disk = std::fs::read(&path).unwrap();
        assert!(!on_disk.contains(&0), "the file itself was not fixed");
        assert_eq!(parse_file(&path), map, "the repaired file parses the same");
        assert!(
            std::fs::read(path.with_extension("utf16.bak")).unwrap().contains(&0),
            "the mis-encoded original was not kept"
        );
        // Second boot: nothing left to do, and the backup is not overwritten
        // with the already-repaired copy.
        repair_file(&path);
        assert!(
            std::fs::read(path.with_extension("utf16.bak")).unwrap().contains(&0),
            "the backup was clobbered on the second pass"
        );

        let mut raw = HashMap::new();
        raw.insert("AGP_TEST_RAW\0NUL".to_string(), "x".to_string());
        raw.insert("AGP_TEST_RAW_VALUE".to_string(), "y\0z".to_string());
        raw.insert("AGP_TEST_RAW_OK".to_string(), "y".to_string());
        std::env::remove_var("AGP_TEST_RAW_OK");
        assert_eq!(apply_missing(&raw, &[]), 1, "only the clean pair is applied");
        std::env::remove_var("AGP_TEST_RAW_OK");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
