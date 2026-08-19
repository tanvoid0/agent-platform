//! Parallel smart-title generation for chat threads. Port of the non-SSE half of
//! `app/chat_thread_title.py`.
//!
//! **Addition, not a deletion.** `chat_thread_title.py` keeps two importers
//! (`assistant/services/assistant_chat.py` and `coder/service.py`), so it stays
//! in Python whatever moves here. This file landed before either half of step 4
//! because both of those domains need it and none of it touches SQL.
//!
//! # What is deliberately not here
//!
//! **`merge_title_sse_events` (`chat_thread_title.py:137-199`) is not ported,
//! and the coder commit found it never needs to be.** Its queue and two workers
//! exist to interleave one late frame into a stream; a
//! `tokio::sync::mpsc::UnboundedSender` cloned to the title task *is* that
//! merge, and "keep waiting for the title after the source closes" falls out of
//! the channel closing when the last sender drops. See
//! `coder::spawn_title_worker`. Its non-SSE dependencies are all in
//! this file:
//! [`await_smart_title`] is the same resolve-or-fall-back it does inline, and
//! `format_sse_event` is one `format!`.
//!
//! **Persistence is the caller's.** Python's `await_smart_title` also runs
//! `persist_thread_title`, which writes whichever of `assistant_chat_threads`
//! or `coder_chat_threads` the caller owns — different tables, so there is
//! nothing shared to port. [`await_smart_title`]
//! returns the resolved title and the caller keeps Python's guard: write the row
//! only when `thread.title.unwrap_or("") != final`, touching `updated_at` with
//! it.
//!
//! # The model call
//!
//! Python reaches the model through `llm_client.call_llm(..., temperature=0.2,
//! max_output_tokens=24)`, which posts to `/v1/chat/completions` over loopback.
//! Here it is [`crate::llm::complete_internal`] — the same resolution, coercion,
//! capability guard, retry policy and usage normalisation, minus a socket. The
//! request body is assembled the way `call_llm` assembles it, message fitting
//! included, so the prompt that reaches the upstream is identical.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use tokio::task::JoinHandle;

use crate::context_budget::fit_chat_messages_for_request;
use crate::dag_schema::sanitize_llm_model_alias;
use crate::AppState;

/// `DEFAULT_PLACEHOLDERS`. The coder domain overrides it with `["New session"]`
/// alone, so this is a default argument rather than the only allowed set.
pub const DEFAULT_PLACEHOLDERS: [&str; 2] = ["New chat", "New session"];

const TITLE_SYSTEM: &str = "You generate short conversation titles. Reply with only the title text: \
max 6 words, no quotes, no trailing punctuation.";

/// `CHAT_SMART_TITLES`, default on. Unset, empty and whitespace-only all mean
/// on, which is what Python's `(os.getenv(...) or "1").strip()` does and what
/// [`crate::env_opt`] already does by trimming and rejecting blanks.
pub fn chat_smart_titles_enabled() -> bool {
    smart_titles_enabled_from(&crate::env_opt("CHAT_SMART_TITLES").unwrap_or_else(|| "1".into()))
}

/// Split out so the word list is testable without writing to the process
/// environment, which every other test in this crate shares.
fn smart_titles_enabled_from(raw: &str) -> bool {
    !matches!(raw.to_lowercase().as_str(), "0" | "false" | "no" | "off")
}

/// The title used when smart titling is off, fails, or has not answered yet:
/// the message itself, whitespace-collapsed and cut to 48 characters.
pub fn fallback_title_from_message(message: &str, default: &str) -> String {
    let text = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return default.to_string();
    }
    // Python slices by character, so counting bytes here would cut CJK titles
    // three times too early.
    if text.chars().count() <= 48 {
        return text;
    }
    format!("{}...", text.chars().take(45).collect::<String>())
}

/// Is this thread still wearing the title the row was created with? Blank and
/// whitespace-only both count.
pub fn is_placeholder_title(title: Option<&str>, placeholders: &[&str]) -> bool {
    match title.map(str::trim).filter(|t| !t.is_empty()) {
        None => true,
        Some(trimmed) => placeholders.contains(&trimmed),
    }
}

/// Python's `splitlines()` boundaries. `str::lines` only knows `\n` and `\r\n`,
/// and a local model that answers with a `\u{2028}` in it should lose the tail
/// the same way on both servers.
fn is_line_boundary(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r'
            | '\u{0b}'
            | '\u{0c}'
            | '\u{1c}'
            | '\u{1d}'
            | '\u{1e}'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

/// `_clean_smart_title`: unquote, first line only, no trailing sentence
/// punctuation, 128 characters hard.
fn clean_smart_title(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches(|c| c == '"' || c == '\'' || c == '`').trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let line = trimmed.split(is_line_boundary).next().unwrap_or("").trim();
    let line = line.trim_end_matches(|c| c == '.' || c == '!' || c == '?');
    if line.chars().count() > 128 {
        return format!("{}...", line.chars().take(125).collect::<String>());
    }
    line.to_string()
}

/// One buffered completion asking for a title. `None` on any failure — the
/// caller always has a fallback, and a title is never worth failing a turn for.
///
/// Python's `generate_smart_title` only wraps the call itself in `try`, so a
/// `null` `content` raises out of the task instead of returning `None`; its
/// caller's own `except Exception: pass` then produces the same fallback. The
/// `?` below collapses both paths into the one observable outcome.
async fn generate_smart_title(
    state: Arc<AppState>,
    message: String,
    model: Option<String>,
) -> Option<String> {
    if !chat_smart_titles_enabled() {
        return None;
    }
    let text = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return None;
    }
    // `text[:800]` — characters, not bytes.
    let user: String = text.chars().take(800).collect();

    // `llm_client.call_llm`'s model resolution: an explicit model wins, else
    // `SUBAGENT_MODEL` then `PLANNER_MODEL`, and the winner is alias-sanitized.
    let raw_model = model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .or_else(crate::executor::default_subagent_model);
    let resolved = raw_model.as_deref().and_then(sanitize_llm_model_alias);

    let (fitted, _) = fit_chat_messages_for_request(vec![
        json!({"role": "system", "content": TITLE_SYSTEM}),
        json!({"role": "user", "content": user}),
    ]);

    let mut body = Map::new();
    body.insert("messages".into(), Value::Array(fitted));
    body.insert("temperature".into(), json!(0.2));
    if let Some(model) = resolved {
        body.insert("model".into(), json!(model));
    }
    body.insert("max_tokens".into(), json!(24));

    let data = crate::llm::complete_internal(&state, body, crate::resources::Priority::Background).await.ok()?;
    let content = data
        .get("choices")?
        .as_array()?
        .first()?
        .get("message")?
        .get("content")?
        .as_str()?;

    let cleaned = clean_smart_title(content);
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Start the title running alongside the caller's own LLM turn, or `None` when
/// smart titles are off.
///
/// `asyncio.create_task` in Python; `tokio::spawn` here, which is eager rather
/// than deferred to the next await point. That only makes the title *more*
/// likely to be ready by the time [`await_smart_title`] asks for it.
pub fn start_smart_title_task(
    state: Arc<AppState>,
    message: &str,
    model: Option<&str>,
) -> Option<JoinHandle<Option<String>>> {
    if !chat_smart_titles_enabled() {
        return None;
    }
    let (message, model) = (message.to_string(), model.map(str::to_string));
    Some(tokio::spawn(generate_smart_title(state, message, model)))
}

/// Resolve the title task, falling back on absence, failure or a panic — the
/// three arms of Python's `if title_task is not None` / `except Exception`.
///
/// See the module docs: the caller persists, this only decides.
pub async fn await_smart_title(task: Option<JoinHandle<Option<String>>>, fallback: &str) -> String {
    match task {
        Some(task) => match task.await {
            Ok(Some(smart)) => smart,
            _ => fallback.to_string(),
        },
        None => fallback.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expected values read off a `python -c` run against
    /// `app/chat_thread_title.py`, not reasoned about.
    #[test]
    fn titles_are_cleaned_the_way_python_cleans_them() {
        assert_eq!(clean_smart_title("  \"Weekly meal plan\"  "), "Weekly meal plan");
        assert_eq!(clean_smart_title("```Plan```"), "Plan");
        assert_eq!(clean_smart_title("Title line one\nline two"), "Title line one");
        assert_eq!(clean_smart_title("Ends in punctuation!?."), "Ends in punctuation");
        assert_eq!(clean_smart_title("   "), "");
        assert_eq!(clean_smart_title("\"\""), "");
        assert_eq!(clean_smart_title("Résumé review"), "Résumé review");

        // 200 characters in, 125 plus the ellipsis out — 128 total, which is the
        // number Python reports for the same input.
        let long = clean_smart_title(&"x".repeat(200));
        assert_eq!(long.chars().count(), 128);
        assert!(long.ends_with("..."));
    }

    #[test]
    fn the_fallback_collapses_whitespace_and_cuts_at_48() {
        assert_eq!(fallback_title_from_message("  Say   hi  ", "New chat"), "Say hi");
        assert_eq!(fallback_title_from_message("   ", "New session"), "New session");
        // Exactly 48 is returned whole; 60 comes back as 45 plus the ellipsis.
        assert_eq!(fallback_title_from_message(&"a".repeat(48), "New chat"), "a".repeat(48));
        assert_eq!(
            fallback_title_from_message(&"x".repeat(60), "New chat"),
            format!("{}...", "x".repeat(45))
        );
    }

    #[test]
    fn placeholders_are_per_domain() {
        assert!(is_placeholder_title(None, &DEFAULT_PLACEHOLDERS));
        assert!(is_placeholder_title(Some(""), &DEFAULT_PLACEHOLDERS));
        assert!(is_placeholder_title(Some("  "), &DEFAULT_PLACEHOLDERS));
        assert!(is_placeholder_title(Some("New chat"), &DEFAULT_PLACEHOLDERS));
        assert!(is_placeholder_title(Some("New session"), &DEFAULT_PLACEHOLDERS));
        assert!(!is_placeholder_title(Some("Meal planning"), &DEFAULT_PLACEHOLDERS));
        // Coder's narrower set: its own placeholder still counts, the
        // assistant's does not.
        assert!(is_placeholder_title(Some("New session"), &["New session"]));
        assert!(!is_placeholder_title(Some("New chat"), &["New session"]));
    }

    #[test]
    fn the_env_switch_is_off_only_for_the_four_falsy_words() {
        for word in ["0", "false", "no", "off", "OFF", "False"] {
            assert!(!smart_titles_enabled_from(word), "{word} should disable smart titles");
        }
        // Everything else is on, including the empty string — `env_opt` never
        // hands one over, but Python's `or "1"` says the same thing.
        for word in ["1", "true", "yes", "on", "", "disabled"] {
            assert!(smart_titles_enabled_from(word), "{word} should leave smart titles on");
        }
    }
}
