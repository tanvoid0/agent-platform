//! Normalize and synthesize token usage on chat completion responses. Port of
//! `app/llm_proxy/usage_normalize.py`.
//!
//! Every caller downstream reads OpenAI's `usage` block, and not every backend
//! sends one — Ollama uses its own field names, and some return nothing at all.
//! This makes the block always present, flagging the case where it was guessed.

use serde_json::{json, Map, Value};

/// `context_budget.estimate_tokens`: real BPE, with Python's char fallback for
/// when the encoder is unavailable.
///
/// This was the char heuristic alone until step 4, on the reasoning that the
/// numbers only surfaced under `estimated: true` and nothing asserted them. That
/// stopped being true when the chat domains moved: `context_usage` is a
/// *response body field* on the coder and assistant routes, `tiktoken>=0.7.0` is
/// a hard requirement on the Python side so it never takes its own fallback, and
/// the same estimator drives `fit_chat_messages_for_request` — so a heuristic
/// here would differ on ten numbers per response *and* send the model a
/// different prompt near the context budget.
pub(crate) fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    match encoder() {
        // Python calls `encoding.encode(text)`, which refuses special tokens —
        // `encode_ordinary` is the same rule.
        Some(bpe) => bpe.encode_ordinary(text).len(),
        None => text.len().div_ceil(4),
    }
}

/// `AGENT_PLATFORM_TOKEN_ENCODING`, default `cl100k_base`, built once.
///
/// An unknown name falls back to the char heuristic rather than to a *different*
/// vocabulary: guessing an encoding would be a silent wrong count, where the
/// heuristic is a declared one. Python's `get_encoding` raises for the same
/// input, and its `except` arm is that heuristic.
pub(crate) fn encoder() -> Option<&'static tiktoken_rs::CoreBPE> {
    use std::sync::OnceLock;
    static ENCODER: OnceLock<Option<tiktoken_rs::CoreBPE>> = OnceLock::new();
    ENCODER
        .get_or_init(|| {
            let name = crate::env_opt("AGENT_PLATFORM_TOKEN_ENCODING")
                .unwrap_or_else(|| "cl100k_base".into());
            match name.as_str() {
                "cl100k_base" => tiktoken_rs::cl100k_base().ok(),
                "o200k_base" => tiktoken_rs::o200k_base().ok(),
                "p50k_base" => tiktoken_rs::p50k_base().ok(),
                "p50k_edit" => tiktoken_rs::p50k_edit().ok(),
                "r50k_base" => tiktoken_rs::r50k_base().ok(),
                other => {
                    logd!(
                        "unknown AGENT_PLATFORM_TOKEN_ENCODING {other:?}; \
                         falling back to the character heuristic"
                    );
                    None
                }
            }
        })
        .as_ref()
}

/// Approximate OpenAI-style overhead per message (role and delimiters).
const MESSAGE_OVERHEAD_TOKENS: usize = 4;

pub(crate) fn estimate_messages_tokens(messages: &[Value]) -> usize {
    let mut total = 0;
    for message in messages {
        match message.get("content") {
            Some(Value::String(text)) => total += estimate_tokens(text),
            Some(other) if !other.is_null() => total += estimate_tokens(&other.to_string()),
            _ => {}
        }
        // Tool call payloads can be large; count them roughly.
        //
        // **`str(tc)`, not JSON.** Python stringifies the *list* with `str()`,
        // so what gets tokenized is a Python repr — single quotes, `None`,
        // `True` — and it is materially shorter than the JSON of the same
        // value once the nested `arguments` string stops needing its quotes
        // escaped. Rendering JSON here under-counted by ~8% of the
        // conversation on a transcript with tool calls. Nothing caught it
        // before coder because no earlier domain's messages carry
        // `tool_calls`.
        if let Some(calls) = message.get("tool_calls").filter(|v| v.is_array()) {
            total += estimate_tokens(&crate::todos::py_repr(calls));
        }
    }
    total + MESSAGE_OVERHEAD_TOKENS * messages.len()
}

fn coerce_int(value: Option<&Value>) -> u64 {
    match value {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0).max(0) as u64,
        Some(Value::String(s)) => s.parse::<i64>().unwrap_or(0).max(0) as u64,
        _ => 0,
    }
}

/// Map a provider's usage shape onto OpenAI's field names, or `None` when there
/// is nothing to map.
pub fn normalize_usage(raw: Option<&Value>) -> Option<Map<String, Value>> {
    let raw = raw?.as_object()?;

    let mut prompt = coerce_int(raw.get("prompt_tokens"));
    let mut completion = coerce_int(raw.get("completion_tokens"));
    let mut total = coerce_int(raw.get("total_tokens"));

    if prompt == 0 && completion == 0 && total == 0 {
        // Ollama's native names.
        prompt = coerce_int(raw.get("prompt_eval_count"));
        completion = coerce_int(raw.get("eval_count"));
        if total == 0 && (prompt > 0 || completion > 0) {
            total = prompt + completion;
        }
    }
    if prompt == 0 && completion == 0 && total == 0 {
        return None;
    }
    if total == 0 {
        total = prompt + completion;
    }

    let mut out = Map::new();
    out.insert("prompt_tokens".into(), json!(prompt));
    out.insert("completion_tokens".into(), json!(completion));
    out.insert("total_tokens".into(), json!(total));
    for key in ["cost", "total_cost", "response_cost"] {
        if let Some(value) = raw.get(key) {
            out.insert(key.into(), value.clone());
        }
    }
    Some(out)
}

pub fn synthesize_usage(request_messages: Option<&Value>, response_content: &str) -> Map<String, Value> {
    let messages = request_messages.and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    let prompt = estimate_messages_tokens(messages);
    let completion = estimate_tokens(response_content);
    let mut out = Map::new();
    out.insert("prompt_tokens".into(), json!(prompt));
    out.insert("completion_tokens".into(), json!(completion));
    out.insert("total_tokens".into(), json!(prompt + completion));
    out.insert("estimated".into(), json!(true));
    out
}

/// Ensure a completion body carries a normalized `usage` block. A body that does
/// not parse is passed through untouched — this is a best-effort enrichment, not
/// a validator.
pub fn normalize_completion_body(body: &[u8], request_messages: Option<&Value>) -> Vec<u8> {
    if body.is_empty() {
        return body.to_vec();
    }
    let Ok(Value::Object(mut data)) = serde_json::from_slice::<Value>(body) else {
        return body.to_vec();
    };

    let usage = normalize_usage(data.get("usage")).unwrap_or_else(|| {
        let content = data
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
            .unwrap_or("");
        synthesize_usage(request_messages, content)
    });

    data.insert("usage".into(), Value::Object(usage));
    serde_json::to_vec(&Value::Object(data)).unwrap_or_else(|_| body.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `tool_calls` are tokenized as Python's `str(list)`, not as JSON. The
    /// numbers here are pinned against `python -c` over the same message, and
    /// the point is the *difference*: rendering JSON gives a lower count, and
    /// that lands in a `context_usage` body as a wrong number rather than as
    /// an error. Coder is the first domain whose transcripts carry tool calls.
    #[test]
    fn tool_calls_are_counted_as_pythons_repr_not_as_json() {
        let messages = [json!({
            "role": "assistant",
            "content": "Looking.",
            "tool_calls": [{
                "id": "c1",
                "type": "function",
                "function": {"name": "read_file", "arguments": "{\"path\":\"app/main.py\"}"},
            }],
        })];
        let counted = estimate_messages_tokens(&messages);

        let repr = crate::todos::py_repr(messages[0].get("tool_calls").unwrap());
        assert!(repr.starts_with("[{'id': 'c1'"), "expected a python repr, got {repr}");
        let expected =
            estimate_tokens("Looking.") + estimate_tokens(&repr) + MESSAGE_OVERHEAD_TOKENS;
        assert_eq!(counted, expected);

        // The JSON rendering this used to use is a different, smaller number.
        let as_json = messages[0].get("tool_calls").unwrap().to_string();
        assert_ne!(estimate_tokens(&as_json), estimate_tokens(&repr));
    }

    #[test]
    fn ollamas_field_names_map_onto_openais() {
        let usage = normalize_usage(Some(&json!({"prompt_eval_count": 100, "eval_count": 25})))
            .expect("mapped");
        assert_eq!(usage["prompt_tokens"], json!(100));
        assert_eq!(usage["completion_tokens"], json!(25));
        assert_eq!(usage["total_tokens"], json!(125));

        // Cost fields ride along when the backend sends them.
        let usage = normalize_usage(Some(&json!({"total_tokens": 5, "cost": 0.01}))).unwrap();
        assert_eq!(usage["cost"], json!(0.01));

        assert!(normalize_usage(Some(&json!({}))).is_none());
        assert!(normalize_usage(None).is_none());
    }

    #[test]
    fn a_body_with_no_usage_gets_an_estimate_that_says_so() {
        let out = normalize_completion_body(
            br#"{"choices":[{"message":{"content":"hi"}}]}"#,
            Some(&json!([{"role": "user", "content": "hello"}])),
        );
        let data: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(data["usage"]["estimated"], json!(true));
        assert!(data["usage"]["total_tokens"].as_u64().unwrap() > 0);
        assert_eq!(
            data["usage"]["total_tokens"].as_u64().unwrap(),
            data["usage"]["prompt_tokens"].as_u64().unwrap()
                + data["usage"]["completion_tokens"].as_u64().unwrap()
        );

        // A real usage block is kept, not replaced by a guess.
        let out = normalize_completion_body(
            br#"{"usage":{"prompt_tokens":7,"completion_tokens":3},"choices":[]}"#,
            None,
        );
        let data: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(data["usage"]["total_tokens"], json!(10));
        assert!(data["usage"].get("estimated").is_none());

        // Not JSON: passed through rather than mangled.
        assert_eq!(normalize_completion_body(b"not json", None), b"not json");
    }
}
