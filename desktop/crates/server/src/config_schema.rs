//! Draft 2020-12 validation for the LLM proxy's `config.yaml`, ported from
//! `admin_routes.py::_validate_config_dict`.
//!
//! Python hands the parsed document to `jsonschema.Draft202012Validator` and
//! puts `ValidationError.message` straight into the 400 body, so the *sentence*
//! is the contract, not just the pass/fail. This is a small hand-rolled
//! validator rather than the `jsonschema` crate for exactly that reason: the
//! crate's wording differs on every keyword, so pulling it in would have traded
//! one divergence for another and cost a dependency. What is here covers the
//! keywords `config.schema.json` actually uses — `type`, `enum`, `minLength`,
//! `required`, `properties`, `additionalProperties`, `items`, `oneOf` — and
//! reproduces `jsonschema`'s message for each.
//!
//! **The one known divergence is *which* error you get when a document has
//! several.** Python's `validate()` raises `best_match(iter_errors(...))`,
//! which scores candidates by path depth and keyword and then descends into a
//! failed `oneOf`'s sub-errors; this returns the first failure in document
//! order and reports a failed `oneOf` as itself. Same 400, same set of
//! documents accepted and rejected — a different sentence when more than one
//! thing is wrong at once. Reproducing `by_relevance` was not worth it for a
//! message an operator reads and fixes.
//!
//! The schema itself is `include_str!`d rather than read from disk. Python
//! searched three candidate paths and *skipped validation entirely* when it
//! found none; that fallback existed because the file sat next to the Python
//! package, which is gone. `CONFIG_DIR/config.schema.json` still wins when an
//! operator drops one there, so a deployment can still pin its own.

use serde_json::Value;

use crate::llm_config::config_dir;
use crate::todos::py_repr;

/// The copy that ships in the binary — `app/llm_proxy/config.schema.json`.
const BUILTIN_SCHEMA: &str = include_str!("config.schema.json");

/// `_config_schema_path` + the `json.loads` after it. An operator-supplied file
/// that does not parse is ignored rather than fatal: Python would have raised a
/// 500 out of `json.loads`, and answering "your config is invalid" because the
/// *schema* is malformed is the worse of the two failures.
fn schema() -> Value {
    let candidate = config_dir().join("config.schema.json");
    if let Ok(text) = std::fs::read_to_string(&candidate) {
        if let Ok(parsed) = serde_json::from_str(&text) {
            return parsed;
        }
        logd!("ignoring unparseable {} — using the built-in schema", candidate.display());
    }
    serde_json::from_str(BUILTIN_SCHEMA).expect("built-in config schema is valid JSON")
}

/// `Err(message)` is what Python puts after `"Config schema: "`.
pub fn validate(data: &Value) -> Result<(), String> {
    check(&schema(), data)
}

fn check(schema: &Value, instance: &Value) -> Result<(), String> {
    let Some(schema) = schema.as_object() else {
        // `true`/`false` schemas: `false` rejects everything, `true` accepts it.
        return match schema.as_bool() {
            Some(false) => Err(format!("{} is not valid under any of the given schemas", py_repr(instance))),
            _ => Ok(()),
        };
    };

    // Keyword order follows the schema document, which is what makes "first
    // failure in document order" a stable answer rather than a hash-order one
    // (`serde_json`'s `preserve_order` feature is on crate-wide).
    for (keyword, expected) in schema {
        match keyword.as_str() {
            "type" => check_type(expected, instance)?,
            "enum" => check_enum(expected, instance)?,
            "minLength" => check_min_length(expected, instance)?,
            "required" => check_required(expected, instance)?,
            "oneOf" => check_one_of(expected, instance)?,
            _ => {}
        }
    }

    // `properties`/`additionalProperties`/`items` recurse, and only apply to the
    // matching instance type — a string instance is not an object with no
    // properties, it is simply not something `properties` says anything about.
    if let Some(members) = instance.as_object() {
        let properties = schema.get("properties").and_then(Value::as_object);
        if let Some(properties) = properties {
            for (name, subschema) in properties {
                if let Some(value) = members.get(name) {
                    check(subschema, value)?;
                }
            }
        }
        if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
            let mut extras: Vec<&str> = members
                .keys()
                .map(String::as_str)
                .filter(|k| properties.is_none_or(|p| !p.contains_key(*k)))
                .collect();
            if !extras.is_empty() {
                // `extras_msg` sorts, so the sentence does not depend on the
                // order the operator happened to write the keys in.
                extras.sort_unstable();
                let rendered: Vec<String> =
                    extras.iter().map(|k| py_repr(&Value::from(*k))).collect();
                let verb = if extras.len() == 1 { "was" } else { "were" };
                return Err(format!(
                    "Additional properties are not allowed ({} {verb} unexpected)",
                    rendered.join(", ")
                ));
            }
        }
    }

    if let (Some(items), Some(subschema)) = (instance.as_array(), schema.get("items")) {
        for item in items {
            check(subschema, item)?;
        }
    }

    Ok(())
}

/// `"1 is not of type 'string'"`. Draft 2020-12 allows a list of names.
fn check_type(expected: &Value, instance: &Value) -> Result<(), String> {
    let names: Vec<&str> = match expected {
        Value::String(name) => vec![name.as_str()],
        Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
        _ => return Ok(()),
    };
    if names.iter().any(|name| is_type(name, instance)) {
        return Ok(());
    }
    let rendered: Vec<String> = names.iter().map(|n| py_repr(&Value::from(*n))).collect();
    Err(format!("{} is not of type {}", py_repr(instance), rendered.join(", ")))
}

/// JSON Schema's type names against Python's runtime types, which is where the
/// one trap lives: `True` **is** an `int` in Python, and `jsonschema` special-
/// cases it so a boolean does not satisfy `"integer"`. `serde_json` keeps them
/// apart already, so the special case is free here — noted because reading this
/// against the Python and finding no `isinstance(x, bool)` check looks wrong.
fn is_type(name: &str, instance: &Value) -> bool {
    match name {
        "object" => instance.is_object(),
        "array" => instance.is_array(),
        "string" => instance.is_string(),
        "integer" => instance.is_i64() || instance.is_u64(),
        "number" => instance.is_number(),
        "boolean" => instance.is_boolean(),
        "null" => instance.is_null(),
        _ => true,
    }
}

/// `"'x' is not one of ['ollama', 'gemini']"`.
fn check_enum(expected: &Value, instance: &Value) -> Result<(), String> {
    let Some(options) = expected.as_array() else { return Ok(()) };
    if options.contains(instance) {
        return Ok(());
    }
    Err(format!("{} is not one of {}", py_repr(instance), py_repr(expected)))
}

/// `"'' should be non-empty"` at `minLength: 1`, `"'ab' is too short"` above it
/// — `jsonschema` splits the wording on that boundary, and every `minLength` in
/// this schema is 1.
fn check_min_length(expected: &Value, instance: &Value) -> Result<(), String> {
    let (Some(min), Some(text)) = (expected.as_u64(), instance.as_str()) else {
        return Ok(());
    };
    if text.chars().count() as u64 >= min {
        return Ok(());
    }
    let complaint = if min == 1 { "should be non-empty" } else { "is too short" };
    Err(format!("{} {complaint}", py_repr(instance)))
}

/// `"'name' is a required property"`. Only applies to objects.
fn check_required(expected: &Value, instance: &Value) -> Result<(), String> {
    let (Some(names), Some(members)) = (expected.as_array(), instance.as_object()) else {
        return Ok(());
    };
    for name in names.iter().filter_map(Value::as_str) {
        if !members.contains_key(name) {
            return Err(format!("{} is a required property", py_repr(&Value::from(name))));
        }
    }
    Ok(())
}

/// Draft 2020-12's `oneOf` is "exactly one", and `jsonschema` has two sentences
/// for the two ways to miss.
fn check_one_of(expected: &Value, instance: &Value) -> Result<(), String> {
    let Some(subschemas) = expected.as_array() else { return Ok(()) };
    let matched: Vec<usize> = subschemas
        .iter()
        .enumerate()
        .filter(|(_, subschema)| check(subschema, instance).is_ok())
        .map(|(index, _)| index)
        .collect();
    match matched.len() {
        1 => Ok(()),
        0 => Err(format!("{} is not valid under any of the given schemas", py_repr(instance))),
        _ => Err(format!(
            "{} is valid under each of {}",
            py_repr(instance),
            matched
                .iter()
                .map(|i| format!("{}", subschemas[*i]))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The built-in schema has to parse, or every POST 500s on the `expect`.
    #[test]
    fn builtin_schema_parses_and_accepts_a_real_config() {
        let config = json!({
            "defaults": {"provider": "ollama", "model": "qwen3:8b"},
            "providers": [{
                "name": "ollama",
                "models": ["qwen3:8b", {"model_name": "fast", "model": "qwen3:1.7b"}],
            }],
        });
        assert_eq!(validate(&config), Ok(()));
    }

    /// One assertion per keyword, each on the sentence rather than the fact of
    /// failing — the sentence is what the 400 body carries.
    #[test]
    fn messages_match_jsonschema_wording() {
        let enum_schema = json!({"enum": ["ollama", "gemini"]});
        assert_eq!(
            check(&enum_schema, &json!("groq")),
            Err("'groq' is not one of ['ollama', 'gemini']".into())
        );

        let type_schema = json!({"type": "string"});
        assert_eq!(check(&type_schema, &json!(1)), Err("1 is not of type 'string'".into()));

        let min_schema = json!({"type": "string", "minLength": 1});
        assert_eq!(check(&min_schema, &json!("")), Err("'' should be non-empty".into()));

        let required_schema = json!({"required": ["name", "models"]});
        assert_eq!(
            check(&required_schema, &json!({"name": "ollama"})),
            Err("'models' is a required property".into())
        );

        let closed = json!({"additionalProperties": false, "properties": {"model": {}}});
        assert_eq!(
            check(&closed, &json!({"model": "x", "z": 1, "a": 2})),
            Err("Additional properties are not allowed ('a', 'z' were unexpected)".into())
        );
        assert_eq!(
            check(&closed, &json!({"z": 1})),
            Err("Additional properties are not allowed ('z' was unexpected)".into())
        );

        let one_of = json!({"oneOf": [{"type": "string"}, {"type": "object"}]});
        assert_eq!(
            check(&one_of, &json!(7)),
            Err("7 is not valid under any of the given schemas".into())
        );
    }

    /// The nested cases the real schema produces, which is where a validator
    /// that only checked the root would still pass the test above.
    #[test]
    fn errors_surface_from_inside_providers() {
        let bad_provider = json!({"providers": [{"name": "groq", "models": ["x"]}]});
        assert_eq!(
            validate(&bad_provider),
            Err("'groq' is not one of ['ollama', 'gemini', 'lm_studio', 'aimlapi']".into())
        );

        let missing_models = json!({"providers": [{"name": "ollama"}]});
        assert_eq!(validate(&missing_models), Err("'models' is a required property".into()));

        // The `oneOf` under `providers[].models`: neither branch takes a number.
        let bad_entry = json!({"providers": [{"name": "ollama", "models": [7]}]});
        assert_eq!(
            validate(&bad_entry),
            Err("7 is not valid under any of the given schemas".into())
        );

        // `defaults` is closed, so a typo'd key is caught rather than ignored.
        let typo = json!({"defaults": {"provider": "ollama", "modle": "x"}});
        assert_eq!(
            validate(&typo),
            Err("Additional properties are not allowed ('modle' was unexpected)".into())
        );

        // The root is `additionalProperties: true` — an unknown top-level key
        // is deliberately allowed, and a test that did not say so would let a
        // future tightening pass unnoticed.
        assert_eq!(validate(&json!({"anything": {"at": "all"}})), Ok(()));
    }
}
