//! Token usage shapes shared by every chat domain. Port of `app/chat_usage.py`.
//!
//! **Addition, not a deletion.** `chat_usage.py` keeps four importers
//! (`action_orchestrator/engine.py`, `assistant/{routes,schemas}.py`,
//! `assistant/services/assistant_chat.py`, `coder/{routes,schemas,service}.py`),
//! so it stays in Python whatever moves here. This file exists because both
//! coder and assistant need it, and it is the only part of step 4 that touches
//! no SQL.
//!
//! **These are wire shapes.** [`ContextUsageOut`] is a response body field on
//! `GET /coder/chat/context-usage` and `GET /assistant/chat/context-usage`, and
//! on every thread/send payload from both; [`LlmUsageOut`] rides along on the
//! send payloads. Field
//! order and names are contract — the structs below are in pydantic's
//! declaration order and nothing here is `skip_serializing_if`, because pydantic
//! emits `"label": null` rather than omitting it.
//!
//! `categories` is a fixed-key struct rather than a map on purpose: Python's
//! `_empty_categories()` builds it in `CONTEXT_CATEGORY_KEYS` order and pydantic
//! preserves that, while `serde_json::Map` without the `preserve_order` feature
//! is a `BTreeMap` and would emit the eight keys alphabetically.
//!
//! **The numbers are comparable for the first time.** `estimate_tokens` is real
//! `tiktoken-rs` BPE on both sides now (see [`crate::usage`]), and Python's
//! `tiktoken>=0.7.0` is a hard requirement, so a cross-render can diff these
//! fields rather than wave at them. One hazard remains, below.
//!
//! # Known divergence: JSON key order in the `tools` / `mcp` / `subagents` counts
//!
//! Python counts `estimate_tokens(json.dumps(tools, ensure_ascii=False))`, and a
//! Python `dict` renders in insertion order. `serde_json::Map` here is a
//! `BTreeMap`, so the same tool specs render with their keys sorted — a
//! different string, and therefore a different count. Measured on the real
//! `coder/executor.py:TOOL_SPECS`: **518 tokens in Python's order, 510 sorted**.
//! That is a live cross-render diff on a response body field.
//!
//! The fix is `serde_json = { features = ["preserve_order"] }` in this crate's
//! `Cargo.toml`, which is a crate-wide change (it makes every `Map` in every
//! module insertion-ordered — almost certainly *closer* to Python everywhere,
//! but it is not this file's call to make while another session owns the tree).
//! Until then, only the three JSON-dumping categories are affected; the five
//! string-joining ones are exact.

use serde::Serialize;
use serde_json::{json, Value};

use crate::context_budget::{estimate_tokens, max_output_tokens_default, prompt_token_budget};
use crate::dag_schema::python_json;
use crate::usage::estimate_messages_tokens;

// ---------------------------------------------------------------------------
// Wire shapes
// ---------------------------------------------------------------------------

/// `CONTEXT_CATEGORY_KEYS`, as fields. Fixed keys matching the Cursor-style UI;
/// the ones no domain fills stay 0 rather than being omitted.
#[derive(Serialize, Default, Debug, Clone, PartialEq, Eq)]
pub struct ContextCategories {
    pub system_prompt: usize,
    pub tools: usize,
    pub rules: usize,
    pub skills: usize,
    pub mcp: usize,
    pub subagents: usize,
    pub conversation: usize,
    pub injected_context: usize,
}

impl ContextCategories {
    fn total(&self) -> usize {
        self.system_prompt
            + self.tools
            + self.rules
            + self.skills
            + self.mcp
            + self.subagents
            + self.conversation
            + self.injected_context
    }
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct ContextUsageOut {
    pub context_window: i64,
    pub total_estimated: usize,
    pub percent_used: f64,
    pub prompt_budget: usize,
    pub reserved_output: i64,
    pub categories: ContextCategories,
}

#[derive(Serialize, Default, Debug, Clone, PartialEq)]
pub struct LlmStepUsageOut {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: f64,
    /// Emitted as `null` when absent — pydantic does not omit it.
    pub label: Option<String>,
}

#[derive(Serialize, Default, Debug, Clone, PartialEq)]
pub struct LlmUsageOut {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: f64,
    pub steps: Vec<LlmStepUsageOut>,
}

// ---------------------------------------------------------------------------
// Usage parsing
// ---------------------------------------------------------------------------

/// `chat_usage._coerce_int`: `max(0, int(v))`, and anything that would raise is
/// 0. Note `bool` is rejected *before* the int conversion, because `int(True)`
/// is 1 in Python and this module does not want that.
///
/// `int(3.9)` truncates toward zero, which `as i64` also does; `int(" 5 ")`
/// strips first, which `trim().parse()` also does. Python additionally accepts
/// `"1_000"` and non-ASCII digit strings — not reproduced, and not reachable
/// from any provider's usage block.
pub(crate) fn coerce_int(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(n)) => {
            n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0).max(0)
        }
        Some(Value::String(s)) => s.trim().parse::<i64>().unwrap_or(0).max(0),
        // None, null, bool, array and object all land on 0.
        _ => 0,
    }
}

/// Parse OpenAI-compatible usage from a chat completion response body.
pub fn parse_llm_usage(data: &Value, label: Option<&str>) -> LlmStepUsageOut {
    let usage = data.get("usage").filter(|v| v.is_object());
    let prompt = coerce_int(usage.and_then(|u| u.get("prompt_tokens")));
    let completion = coerce_int(usage.and_then(|u| u.get("completion_tokens")));
    let mut total = coerce_int(usage.and_then(|u| u.get("total_tokens")));
    if total == 0 && (prompt != 0 || completion != 0) {
        total = prompt + completion;
    }
    LlmStepUsageOut {
        prompt_tokens: prompt,
        completion_tokens: completion,
        total_tokens: total,
        // Reused from `crate::executor`, where step 3 ported
        // `llm_client.usage_cost_from_completion_response` — same function,
        // same five shapes, already covered by its own test.
        cost_usd: crate::executor::usage_cost_from_completion_response(data),
        label: label.map(str::to_string),
    }
}

/// Parse a raw usage dict (e.g. from a streaming final chunk).
pub fn parse_llm_usage_dict(usage: Option<&Value>, label: Option<&str>) -> LlmStepUsageOut {
    match usage.filter(|v| v.is_object()) {
        // Python's `if not isinstance(usage, dict)` arm: label only, no cost
        // lookup at all.
        None => LlmStepUsageOut { label: label.map(str::to_string), ..Default::default() },
        Some(usage) => parse_llm_usage(&json!({ "usage": usage }), label),
    }
}

pub fn merge_llm_usages(steps: Vec<LlmStepUsageOut>) -> LlmUsageOut {
    if steps.is_empty() {
        return LlmUsageOut::default();
    }
    LlmUsageOut {
        prompt_tokens: steps.iter().map(|s| s.prompt_tokens).sum(),
        completion_tokens: steps.iter().map(|s| s.completion_tokens).sum(),
        total_tokens: steps.iter().map(|s| s.total_tokens).sum(),
        // Left-to-right from 0.0, the same fold `sum()` does — so the same
        // float, including `0.1 + 0.2 == 0.30000000000000004`.
        cost_usd: steps.iter().map(|s| s.cost_usd).sum(),
        steps,
    }
}

// ---------------------------------------------------------------------------
// Context estimate
// ---------------------------------------------------------------------------

/// `AGENT_PLATFORM_CONTEXT_WINDOW_TOKENS`, default 32768, floor 1024.
///
/// `context_budget::context_window_tokens` is the same function and is private
/// to that module; it needs `pub(crate)` for this call to compile. Left as a
/// call rather than a fourth copy of `env_int` — see the report.
fn context_window() -> i64 {
    crate::context_budget::context_window_tokens()
}

/// Python's `round(x, 1)`: correctly rounded on the double's *exact* value, ties
/// to even.
///
/// Not `(x * 10.0).round() / 10.0` — that rounds 6.25 to 6.3 where Python gives
/// 6.2 (reachable: a 1024-token window with 64 tokens used is exactly 6.25%),
/// and scaling first mis-rounds 0.145 the other way. Rust's float formatter
/// rounds the same way Python's `repr` path does, so one round trip through a
/// single decimal place is the whole job.
fn round1(value: f64) -> f64 {
    format!("{value:.1}").parse().unwrap_or(0.0)
}

/// The keyword-only arguments of `estimate_context_usage`. A struct because Rust
/// has no keyword defaults and every caller fills a different three or four of
/// the eight: coder passes system/tools/conversation, assistant adds
/// `injected_context`. The other four
/// have no caller in either language today but are part of the wire shape.
#[derive(Default)]
pub struct ContextInputs<'a> {
    pub system_prompt: Option<&'a str>,
    pub tools: Option<&'a [Value]>,
    pub rules: Option<&'a [String]>,
    pub skills: Option<&'a [String]>,
    pub mcp_tools: Option<&'a [Value]>,
    pub subagent_defs: Option<&'a [Value]>,
    pub conversation_messages: Option<&'a [Value]>,
    pub injected_context: Option<&'a str>,
}

/// `if x:` in Python — an empty string, list or `None` all skip the category,
/// leaving it 0.
fn non_empty_str(value: Option<&str>) -> Option<&str> {
    value.filter(|s| !s.is_empty())
}

fn non_empty<T>(value: Option<&[T]>) -> Option<&[T]> {
    value.filter(|s| !s.is_empty())
}

/// Estimate the input context breakdown. Real BPE, so it may still diverge from
/// what the *provider* bills — that is Python's caveat too, not a port defect.
pub fn estimate_context_usage(inputs: &ContextInputs<'_>) -> ContextUsageOut {
    let mut categories = ContextCategories::default();

    if let Some(text) = non_empty_str(inputs.system_prompt) {
        categories.system_prompt = estimate_tokens(text);
    }
    if let Some(tools) = non_empty(inputs.tools) {
        categories.tools = estimate_tokens(&python_json(&tools, false));
    }
    if let Some(rules) = non_empty(inputs.rules) {
        categories.rules = estimate_tokens(&rules.join("\n"));
    }
    if let Some(skills) = non_empty(inputs.skills) {
        categories.skills = estimate_tokens(&skills.join("\n"));
    }
    if let Some(mcp) = non_empty(inputs.mcp_tools) {
        categories.mcp = estimate_tokens(&python_json(&mcp, false));
    }
    if let Some(subagents) = non_empty(inputs.subagent_defs) {
        categories.subagents = estimate_tokens(&python_json(&subagents, false));
    }
    if let Some(messages) = non_empty(inputs.conversation_messages) {
        categories.conversation = estimate_messages_tokens(messages);
    }
    if let Some(text) = non_empty_str(inputs.injected_context) {
        categories.injected_context = estimate_tokens(text);
    }

    let total = categories.total();
    let window = context_window();
    let percent =
        if window > 0 { round1(total as f64 / window as f64 * 100.0) } else { 0.0 };

    ContextUsageOut {
        context_window: window,
        total_estimated: total,
        percent_used: percent,
        prompt_budget: prompt_token_budget(),
        reserved_output: max_output_tokens_default(),
        categories,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every number below was read off a `python -c` run against
    /// `app/chat_usage.py` with no `AGENT_PLATFORM_*` overrides set, not
    /// reasoned about. The repo `.env` and `config/agent_platform.yaml` set none
    /// of the four env vars this file reads, so the defaults are what both
    /// servers actually run with.
    #[test]
    fn the_category_breakdown_matches_pythons() {
        let tools = [json!({
            "type": "function",
            "function": {"name": "read_file", "description": "Read a file"},
        })];
        let mcp = [json!({"name": "fetch", "args": {"url": "string"}})];
        let subagents = [json!({"slug": "researcher", "model": "gpt-4o-mini"})];
        let conversation = [
            json!({"role": "user", "content": "hello world"}),
            json!({"role": "assistant", "content": "Hi! How can I help?"}),
        ];
        let rules = ["be brief".to_string(), "no emoji".to_string()];
        let skills = ["planner".to_string(), "coder".to_string()];

        let out = estimate_context_usage(&ContextInputs {
            system_prompt: Some("You are a helpful assistant."),
            tools: Some(&tools),
            rules: Some(&rules),
            skills: Some(&skills),
            mcp_tools: Some(&mcp),
            subagent_defs: Some(&subagents),
            conversation_messages: Some(&conversation),
            // Non-ASCII, so `ensure_ascii=False` is exercised rather than assumed.
            injected_context: Some("Résumé: nothing yet"),
        });

        assert_eq!(out.context_window, 32768);
        assert_eq!(out.prompt_budget, 28160);
        assert_eq!(out.reserved_output, 4096);
        assert_eq!(
            out.categories,
            ContextCategories {
                system_prompt: 6,
                // These three are the key-order hazard in the module docs.
                // Checked both ways in Python: these fixtures tokenize to the
                // same counts sorted as in insertion order (26 / 17 / 20), so
                // they pin the arithmetic and not the ordering. The real
                // `coder/executor.py:TOOL_SPECS` does *not* — 518 against 510.
                tools: 26,
                rules: 5,
                skills: 4,
                mcp: 17,
                subagents: 20,
                // 2 + 7 content tokens plus 4 overhead per message.
                conversation: 17,
                injected_context: 6,
            }
        );
        assert_eq!(out.total_estimated, 101);
        assert_eq!(out.percent_used, 0.3);
    }

    #[test]
    fn an_empty_estimate_is_all_zeroes_and_still_reports_the_window() {
        let out = estimate_context_usage(&ContextInputs::default());
        assert_eq!(out.categories, ContextCategories::default());
        assert_eq!(out.total_estimated, 0);
        assert_eq!(out.percent_used, 0.0);
        assert_eq!(out.context_window, 32768);
        // The whole body, in pydantic's field order.
        assert_eq!(
            serde_json::to_string(&out).unwrap(),
            r#"{"context_window":32768,"total_estimated":0,"percent_used":0.0,"prompt_budget":28160,"reserved_output":4096,"categories":{"system_prompt":0,"tools":0,"rules":0,"skills":0,"mcp":0,"subagents":0,"conversation":0,"injected_context":0}}"#
        );

        // Empty is falsy in Python, so an empty list is not an empty JSON array
        // in the count — it is skipped entirely.
        let empty: [Value; 0] = [];
        let out = estimate_context_usage(&ContextInputs {
            system_prompt: Some(""),
            tools: Some(&empty),
            ..Default::default()
        });
        assert_eq!(out.categories, ContextCategories::default());
    }

    #[test]
    fn percent_rounds_the_way_python_rounds() {
        // `round(6.25, 1) == 6.2` — ties to even. A naive `(x*10).round()/10`
        // gives 6.3 here and this assertion is the only thing that would catch
        // it drifting.
        assert_eq!(round1(6.25), 6.2);
        assert_eq!(round1(0.145), 0.1);
        assert_eq!(round1(12.35), 12.3);
        assert_eq!(round1(0.05), 0.1);
        assert_eq!(round1(0.0), 0.0);
    }

    /// The shapes `python -c` was run over, in the same order. `label` rides
    /// through untouched on every one of them.
    #[test]
    fn usage_dicts_parse_the_way_python_parses_them() {
        let step = |usage: Option<Value>| parse_llm_usage_dict(usage.as_ref(), Some("step"));
        let tokens = |s: &LlmStepUsageOut| (s.prompt_tokens, s.completion_tokens, s.total_tokens);

        // Absent, or present but not an object at all.
        assert_eq!(tokens(&step(None)), (0, 0, 0));
        assert_eq!(tokens(&step(Some(json!(["nope"])))), (0, 0, 0));
        assert_eq!(tokens(&step(Some(json!({})))), (0, 0, 0));
        assert_eq!(step(None).label.as_deref(), Some("step"));

        // The ordinary OpenAI block, and one missing `total_tokens`.
        let openai = json!({"prompt_tokens": 12, "completion_tokens": 5, "total_tokens": 17});
        assert_eq!(tokens(&step(Some(openai))), (12, 5, 17));
        assert_eq!(tokens(&step(Some(json!({"prompt_tokens": 12, "completion_tokens": 5})))), (12, 5, 17));
        // All three zero stays zero — the sum is only synthesized when a part is set.
        let zeroes = json!({"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0});
        assert_eq!(tokens(&step(Some(zeroes))), (0, 0, 0));

        // `_coerce_int`'s whole surface: strings (stripped), an unparsable
        // string, a float (truncated), a negative (floored at 0), null, and
        // bools — which are 0 here even though `int(True)` is 1 in Python.
        let strings = json!({"prompt_tokens": "12", "completion_tokens": " 5 ", "total_tokens": "oops"});
        assert_eq!(tokens(&step(Some(strings))), (12, 5, 17));
        let floats = json!({"prompt_tokens": 3.9, "completion_tokens": -4, "total_tokens": null});
        assert_eq!(tokens(&step(Some(floats))), (3, 0, 3));
        assert_eq!(tokens(&step(Some(json!({"prompt_tokens": true, "completion_tokens": 2})))), (0, 2, 2));
        // A nested value in a count field is 0, but a good `total_tokens`
        // survives its siblings being junk.
        let junk = json!({"prompt_tokens": [1], "completion_tokens": {"a": 1}, "total_tokens": 9});
        assert_eq!(tokens(&step(Some(junk))), (0, 0, 9));

        // Cost is read out of the same dict this shape gets wrapped into.
        let with_cost = json!({"prompt_tokens": 1, "completion_tokens": 2, "cost": 0.25});
        let parsed = step(Some(with_cost));
        assert_eq!((tokens(&parsed), parsed.cost_usd), ((1, 2, 3), 0.25));
        assert!(parse_llm_usage_dict(Some(&json!({"prompt_tokens": 1})), None).label.is_none());

        // Whole-body parsing, where cost hides in `_hidden_params`.
        let body = json!({
            "usage": {"prompt_tokens": 4, "completion_tokens": 6},
            "_hidden_params": {"response_cost": 3.0},
        });
        let parsed = parse_llm_usage(&body, Some("body"));
        assert_eq!((tokens(&parsed), parsed.cost_usd), ((4, 6, 10), 3.0));
        assert_eq!(tokens(&parse_llm_usage(&json!({"choices": []}), None)), (0, 0, 0));
    }

    #[test]
    fn merging_sums_the_steps_and_keeps_them() {
        assert_eq!(merge_llm_usages(Vec::new()), LlmUsageOut::default());

        let merged = merge_llm_usages(vec![
            LlmStepUsageOut {
                prompt_tokens: 3,
                completion_tokens: 2,
                total_tokens: 5,
                cost_usd: 0.1,
                label: Some("a".into()),
            },
            LlmStepUsageOut {
                prompt_tokens: 7,
                completion_tokens: 1,
                total_tokens: 8,
                cost_usd: 0.2,
                label: Some("b".into()),
            },
        ]);
        assert_eq!((merged.prompt_tokens, merged.completion_tokens, merged.total_tokens), (10, 3, 13));
        // Python's `sum()` folds left from 0, and so does this — including the
        // float error, which lands in a response body verbatim.
        assert_eq!(merged.cost_usd, 0.30000000000000004);
        assert_eq!(merged.steps.len(), 2);
        assert_eq!(
            serde_json::to_string(&merged).unwrap(),
            r#"{"prompt_tokens":10,"completion_tokens":3,"total_tokens":13,"cost_usd":0.30000000000000004,"steps":[{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5,"cost_usd":0.1,"label":"a"},{"prompt_tokens":7,"completion_tokens":1,"total_tokens":8,"cost_usd":0.2,"label":"b"}]}"#
        );
    }

    /// `serde_json`'s `preserve_order` feature is load-bearing, not a nicety: a
    /// Python dict renders in insertion order, a `BTreeMap` renders sorted, and
    /// this module *counts the tokens of the rendered JSON* into a response body
    /// field — the real `TOOL_SPECS` measured 518 tokens Python's way against
    /// 510 sorted. The feature is one line in `Cargo.toml` and its loss would be
    /// silent everywhere except a cross-render, so it gets an assertion.
    #[test]
    fn json_objects_keep_insertion_order() {
        let rendered = serde_json::json!({"zebra": 1, "apple": 2}).to_string();
        assert_eq!(
            rendered, r#"{"zebra":1,"apple":2}"#,
            "serde_json lost `preserve_order`; every rendered object is now sorted \
             and token counts in `context_usage` will drift from Python's"
        );
    }
}
