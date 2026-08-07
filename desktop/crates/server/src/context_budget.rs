//! Server-side prompt sizing: token estimates, truncation, and message
//! shrinking. Port of `app/context_budget.py`.
//!
//! The UI never decides context limits; it only displays whatever the API
//! returns. Every clamp here is server-side and driven by `AGENT_PLATFORM_*`.
//!
//! **The tokenizer is real BPE**, via `tiktoken-rs`, shared with
//! [`crate::usage`]. This file carried only Python's char fallback until step 4,
//! on the reasoning that nothing asserted exact token counts — which stopped
//! being true when the chat domains moved: `context_usage` is a *response body
//! field* there, `tiktoken` is a hard requirement on the Python side so it never
//! takes its own fallback, and the same estimator drives
//! [`fit_chat_messages_for_request`], so a heuristic would have sent the model a
//! different prompt near the budget rather than merely displaying a different
//! number. The fallback survives for an encoder that will not load.
//!
//! **A long run of one repeated character is the BPE regex's worst case**, on
//! both sides: ~300ms per 8k here, ~145ms in Python, and superlinear beyond
//! that. Prose of the same length costs ~6ms. It is parity, not a port defect,
//! but it means a pathological *tool result* — a wall of dashes, base64, a
//! minified blob — can cost real time in [`shrink_messages_to_budget`], which
//! re-estimates every message each round. Worth knowing before the coder domain
//! starts feeding this function whatever a command printed.
//!
//! The four functions this file used to skip because orchestration was Python —
//! `fit_dependency_outputs_to_budget`, `dependency_context_token_budget`,
//! `subdag_parent_output_max_tokens`, `tool_result_soft_cap_tokens` — landed
//! with [`crate::executor`].

use serde_json::Value;

use crate::env_opt;
use crate::usage::estimate_messages_tokens;

/// Python's `shrink_messages_to_budget(..., min_message_tokens=48)` default. No
/// caller overrides it, so it is a constant rather than an argument.
const MIN_MESSAGE_TOKENS: usize = 48;

/// `context_budget._MESSAGE_OVERHEAD_TOKENS`: the approximate OpenAI per-message
/// cost of a role plus delimiters. [`crate::usage`] keeps its own copy for the
/// estimator; this one is only read by [`dependency_context_token_budget`].
const MESSAGE_OVERHEAD_TOKENS: usize = 4;

const TRUNCATION_SUFFIX: &str = "...[truncated]";

/// `int(os.getenv(name) or default)`, clamped to `min`. An unset **or**
/// unparsable value falls back to `default` un-clamped, exactly as Python's
/// `try: max(min, int(raw)) except ValueError: default` does.
fn env_int(name: &str, default: i64, min: i64) -> i64 {
    env_opt(name)
        .and_then(|raw| raw.parse::<i64>().ok())
        .map_or(default, |value| value.max(min))
}

pub(crate) fn context_window_tokens() -> i64 {
    env_int("AGENT_PLATFORM_CONTEXT_WINDOW_TOKENS", 32768, 1024)
}

pub fn max_output_tokens_default() -> i64 {
    env_int("AGENT_PLATFORM_MAX_OUTPUT_TOKENS", 4096, 256)
}

fn safety_margin_tokens() -> i64 {
    env_int("AGENT_PLATFORM_CONTEXT_SAFETY_MARGIN", 512, 0)
}

/// Maximum tokens allowed for the prompt side of a chat/completions request
/// (messages only, excluding the completion).
pub fn prompt_token_budget() -> usize {
    let window = context_window_tokens();
    let out = max_output_tokens_default();
    let margin = safety_margin_tokens();
    window.saturating_sub(out).saturating_sub(margin).max(512) as usize
}

/// Best-effort token count — the char heuristic, see the module docs.
pub fn estimate_tokens(text: &str) -> usize {
    crate::usage::estimate_tokens(text)
}

pub fn truncate_text_to_tokens(text: &str, max_tokens: usize) -> String {
    if max_tokens == 0 || text.is_empty() {
        return String::new();
    }
    let suffix_tokens = estimate_tokens(TRUNCATION_SUFFIX);
    if max_tokens <= suffix_tokens {
        // Python: `suffix[:max_tokens] if max_tokens < len(suffix) else suffix`.
        // The suffix is ASCII, so slicing it by bytes is slicing it by chars.
        return TRUNCATION_SUFFIX.get(..max_tokens).unwrap_or(TRUNCATION_SUFFIX).to_string();
    }

    // Python's primary path: cut at a *token* boundary, so the result is exactly
    // `max_tokens` wide. Its `except` arm is the char heuristic below.
    if let Some(bpe) = crate::usage::encoder() {
        let ids = bpe.encode_ordinary(text);
        if ids.len() <= max_tokens - suffix_tokens {
            return text.to_string();
        }
        let keep = max_tokens - suffix_tokens;
        if let Ok(head) = bpe.decode(ids[..keep].to_vec()) {
            return format!("{head}{TRUNCATION_SUFFIX}");
        }
        // A cut between the two halves of a multi-token character does not
        // decode; fall through rather than lose the text entirely.
    }

    // Heuristic fallback: ~4 chars per token, keeping Python's exact arithmetic.
    let budget_chars = (max_tokens - suffix_tokens) * 4 - 3;
    if text.len() <= budget_chars {
        return text.to_string();
    }
    // Python cuts at a character index; Rust cuts at a byte index and panics
    // mid-codepoint, so walk back to the nearest char boundary. On non-ASCII text
    // that lands a few bytes short of Python's cut — fine for an estimate, and a
    // panic in a budgeting helper is not.
    let mut cut = budget_chars.min(text.len());
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{}", &text[..cut], TRUNCATION_SUFFIX)
}

fn role_priority(role: Option<&str>) -> u8 {
    match role {
        Some("tool") => 0,
        Some("assistant") => 1,
        Some("user") => 2,
        _ => 3, // system
    }
}

fn set_content(message: &mut Value, content: String) {
    if let Some(object) = message.as_object_mut() {
        object.insert("content".into(), Value::String(content));
    }
}

/// Return a copy of `messages` with contents truncated until the estimated
/// prompt size is `<= budget_tokens`. Shrinks tool outputs first, then assistant,
/// then user, then system (oldest indices first within each role tier).
pub fn shrink_messages_to_budget(messages: &[Value], budget_tokens: usize) -> Vec<Value> {
    if budget_tokens == 0 {
        return Vec::new();
    }
    let mut msgs = messages.to_vec();
    if msgs.is_empty() || estimate_messages_tokens(&msgs) <= budget_tokens {
        return msgs;
    }

    let mut order: Vec<usize> = (0..msgs.len()).collect();
    order.sort_by_key(|&i| (role_priority(msgs[i].get("role").and_then(Value::as_str)), i));

    // First pass: one message per round, in role-priority order, each cut to
    // roughly what the overage asks for but never below the floor.
    for _ in 0..(msgs.len() * 32).max(64) {
        let total = estimate_messages_tokens(&msgs);
        if total <= budget_tokens {
            break;
        }
        let over = total - budget_tokens;
        let mut progressed = false;
        for &i in &order {
            // Owned, so the mutation below is not fighting a live borrow.
            let Some(content) = msgs[i].get("content").and_then(Value::as_str).map(str::to_owned)
            else {
                continue;
            };
            if content.is_empty() {
                continue;
            }
            let tokens = estimate_tokens(&content);
            if tokens <= MIN_MESSAGE_TOKENS {
                continue;
            }
            let target = tokens.saturating_sub(over).max(MIN_MESSAGE_TOKENS);
            let truncated = truncate_text_to_tokens(&content, target);
            if truncated != content {
                set_content(&mut msgs[i], truncated);
                progressed = true;
                break;
            }
        }
        if !progressed {
            break;
        }
    }

    // Second pass: the floor above can leave the total over budget, so keep
    // cutting the single largest message, floor be damned.
    for _ in 0..(msgs.len() * 8) {
        let total = estimate_messages_tokens(&msgs);
        if total <= budget_tokens {
            break;
        }
        let over = total - budget_tokens;
        let mut best: Option<(usize, usize)> = None;
        for (i, message) in msgs.iter().enumerate() {
            let Some(content) = message.get("content").and_then(Value::as_str) else {
                continue;
            };
            let tokens = estimate_tokens(content);
            if tokens > best.map_or(0, |(_, best_tokens)| best_tokens) {
                best = Some((i, tokens));
            }
        }
        let Some((i, tokens)) = best.filter(|&(_, tokens)| tokens > 1) else {
            break;
        };
        let content = msgs[i]["content"].as_str().unwrap_or_default().to_owned();
        let target = tokens.saturating_sub(over).max(1);
        set_content(&mut msgs[i], truncate_text_to_tokens(&content, target));
    }

    msgs
}

/// Ensure messages fit the configured prompt budget. Returns
/// `(messages, prompt_token_budget)`.
pub fn fit_chat_messages_for_request(messages: Vec<Value>) -> (Vec<Value>, usize) {
    let budget = prompt_token_budget();
    (shrink_messages_to_budget(&messages, budget), budget)
}

// ---------------------------------------------------------------------------
// Orchestration: dependency context and sub-DAG prompts
// ---------------------------------------------------------------------------

/// Max tokens for the parent task's output embedded in a sub-DAG expansion
/// prompt.
pub fn subdag_parent_output_max_tokens() -> usize {
    env_int("AGENT_PLATFORM_SUBDAG_PARENT_MAX_TOKENS", 4000, 256) as usize
}

/// Per tool result: truncate before appending to the conversation.
///
/// ponytail: nothing calls this — the tool-calling path is off by default and
/// [`crate::executor`] refuses to start a task when it is on rather than porting
/// `tool_handlers.py`. Kept because it is one of the four functions this file
/// owed `context_budget.py`, and the caller arrives with that 782-LOC module.
#[allow(dead_code)]
pub fn tool_result_soft_cap_tokens() -> usize {
    env_int("AGENT_PLATFORM_TOOL_RESULT_MAX_TOKENS", 12000, 256) as usize
}

/// Tokens available for the dependency block given the system message and the
/// user text it will be appended to.
pub fn dependency_context_token_budget(system_message: &str, instructions_and_preamble: &str) -> usize {
    let budget = prompt_token_budget();
    let header = "\n\nContext from previous steps:\n";
    let fixed = estimate_tokens(system_message)
        + estimate_tokens(instructions_and_preamble)
        + MESSAGE_OVERHEAD_TOKENS * 2
        + estimate_tokens(header);
    budget.saturating_sub(fixed).max(256)
}

/// Truncate dependency outputs so the combined estimate — separators included —
/// fits `max_tokens`, scaling each chunk proportionally when over budget.
pub fn fit_dependency_outputs_to_budget(chunks: &[String], max_tokens: usize) -> Vec<String> {
    if chunks.is_empty() {
        return Vec::new();
    }
    if max_tokens == 0 {
        // Python's `max_tokens <= 0` branch: every chunk truncated to nothing.
        return chunks.iter().map(|c| truncate_text_to_tokens(c, 0)).collect();
    }
    let separator_tokens = estimate_tokens("\n---\n") * (chunks.len() - 1);
    let body_budget = max_tokens.saturating_sub(separator_tokens).max(64);
    let estimates: Vec<usize> = chunks.iter().map(|c| estimate_tokens(c).max(1)).collect();
    let total: usize = estimates.iter().sum();
    if total <= body_budget {
        return chunks.to_vec();
    }
    let scale = body_budget as f64 / total as f64;
    chunks
        .iter()
        .zip(&estimates)
        .map(|(chunk, &tokens)| {
            // `int(te * scale)` truncates toward zero, which `as usize` also does.
            let allow = ((tokens as f64 * scale) as usize).max(48);
            truncate_text_to_tokens(chunk, allow)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truncation_marks_itself() {
        // The contract is a *token* count, not a character count: this asserted
        // `(100 - 4) * 4 - 3` characters while only the char heuristic was
        // ported, and real BPE keeps 774 characters of `"xxx…"` inside the same
        // 100 tokens. Asserting the width Python actually promises is the point.
        let out = truncate_text_to_tokens(&"x".repeat(1000), 100);
        assert!(out.ends_with(TRUNCATION_SUFFIX));
        // Both numbers are what `app/context_budget.py` returns for this input,
        // read off a Python run: 774 characters, exactly 100 tokens.
        assert_eq!(estimate_tokens(&out), 100, "cut to exactly the budget");
        assert_eq!(out.len(), 774, "same cut Python makes");
        // Under budget is returned whole, and a budget under the suffix's own
        // cost degrades to a slice of the suffix rather than panicking.
        assert_eq!(truncate_text_to_tokens("short", 100), "short");
        assert_eq!(truncate_text_to_tokens("whatever", 3), "...");
        // Multi-byte text survives, whichever branch runs.
        assert!(truncate_text_to_tokens(&"é".repeat(1000), 100).ends_with(TRUNCATION_SUFFIX));
    }

    #[test]
    #[ignore = "timing probe, not a contract"]
    fn bench_estimator() {
        for (label, text) in [
            ("8000 repeated y", "y".repeat(8000)),
            ("8000 chars prose", "the quick brown fox ".repeat(400)),
        ] {
            let t = std::time::Instant::now();
            let n = estimate_tokens(&text);
            println!("{label:20} tokens={n:6} {:?}", t.elapsed());
        }
    }

    #[test]
    fn the_encoder_is_the_real_one() {
        // Guards the decision itself: if the encoder ever silently stops loading,
        // every count falls back to `len/4` and both halves of the platform drift
        // apart on numbers a user can see. These are cl100k_base's real answers.
        assert_eq!(estimate_tokens("hello world"), 2);
        assert_eq!(estimate_tokens("The quick brown fox jumps over the lazy dog"), 9);
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn a_list_that_already_fits_is_passed_through_untouched() {
        let msgs = vec![
            json!({"role": "system", "content": "be brief"}),
            json!({"role": "user", "content": "hello"}),
        ];
        assert_eq!(shrink_messages_to_budget(&msgs, 1000), msgs);
    }

    #[test]
    fn an_oversized_list_comes_back_under_budget() {
        let msgs: Vec<Value> = (0..3)
            .map(|_| json!({"role": "user", "content": "y".repeat(8000)}))
            .collect();
        let out = shrink_messages_to_budget(&msgs, 500);
        assert_eq!(out.len(), 3, "messages are shrunk, never dropped");
        assert!(estimate_messages_tokens(&out) <= 500);
    }

    #[test]
    fn dependency_chunks_that_fit_are_returned_whole() {
        let chunks = vec!["alpha".to_string(), "beta".to_string()];
        assert_eq!(fit_dependency_outputs_to_budget(&chunks, 1000), chunks);
        assert!(fit_dependency_outputs_to_budget(&[], 1000).is_empty());
        // A zero budget empties every chunk rather than dropping any.
        assert_eq!(fit_dependency_outputs_to_budget(&chunks, 0), vec!["".to_string(); 2]);
    }

    #[test]
    fn oversized_dependency_chunks_scale_proportionally() {
        let chunks = vec!["a".repeat(8000), "b".repeat(2000)];
        let out = fit_dependency_outputs_to_budget(&chunks, 500);
        assert_eq!(out.len(), 2, "chunks are truncated, never dropped");
        assert!(out.iter().all(|c| c.ends_with(TRUNCATION_SUFFIX)));
        // The bigger input keeps the bigger share.
        assert!(out[0].len() > out[1].len());
        // Every chunk keeps at least the 48-token floor's worth of text.
        assert!(out.iter().all(|c| estimate_tokens(c) >= 48));
    }

    #[test]
    fn the_dependency_budget_is_what_is_left_after_the_fixed_text() {
        let full = dependency_context_token_budget("", "");
        let squeezed = dependency_context_token_budget(&"s".repeat(4000), &"u".repeat(4000));
        assert!(squeezed < full);
        // Never negative and never below the floor, however large the prompt.
        //
        // Deliberately prose rather than one character repeated 400_000 times,
        // which is what this asserted while tokenising was `len / 4` and free.
        // A long run of an identical character is the worst case for the BPE
        // regex — 300ms per 8k here and 145ms in Python — so that input turned
        // one assertion into a minute of CPU. See the module docs.
        let huge = "the quick brown fox ".repeat(9000);
        assert_eq!(dependency_context_token_budget(&huge, ""), 256);
    }

    #[test]
    fn tool_output_is_cut_before_system_prompt() {
        let msgs = vec![
            json!({"role": "system", "content": "a".repeat(400)}),
            json!({"role": "tool", "content": "b".repeat(4000)}),
        ];
        let out = shrink_messages_to_budget(&msgs, 600);
        assert_eq!(out[0], msgs[0], "system survives whole");
        assert!(out[1]["content"].as_str().unwrap().ends_with(TRUNCATION_SUFFIX));
        assert!(estimate_messages_tokens(&out) <= 600);
    }
}
