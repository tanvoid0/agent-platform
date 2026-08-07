//! Log-line parsing for the Logs screen.
//!
//! One stream carries three shapes: structlog JSON objects from the Python
//! server, uvicorn's `LEVEL:   text` lines, and our own `[tag] text` shell
//! notes. Splitting them into the same fields lets the screen render columns
//! instead of a wall of JSON; anything else falls through as raw text.

use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "TRACE" | "DEBUG" => Some(Level::Debug),
            "INFO" | "NOTICE" => Some(Level::Info),
            "WARN" | "WARNING" => Some(Level::Warn),
            "ERROR" | "CRITICAL" | "FATAL" => Some(Level::Error),
            _ => None,
        }
    }
}

/// A line split into what the screen shows as columns. `fields` holds whatever
/// the JSON carried beyond the four named ones, in key order.
#[derive(Debug, Default, PartialEq)]
pub struct Entry {
    pub level: Option<Level>,
    pub time: Option<String>,
    pub source: Option<String>,
    pub message: String,
    pub fields: Vec<(String, String)>,
}

pub fn parse(line: &str) -> Entry {
    let line = line.trim_end();
    json(line)
        .or_else(|| prefixed(line))
        .unwrap_or_else(|| Entry { message: line.to_string(), ..Entry::default() })
}

fn json(line: &str) -> Option<Entry> {
    let line = line.trim_start();
    if !line.starts_with('{') {
        return None;
    }
    let obj: Map<String, Value> = serde_json::from_str(line).ok()?;
    // structlog names the human sentence `message` and the machine name
    // `event`. Without either there is no message to head the row with, so the
    // object is some other JSON and is better shown raw.
    let has_message = obj.contains_key("message");
    let message = string(&obj, "message").or_else(|| string(&obj, "event"))?;
    let mut fields: Vec<(String, String)> = obj
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "timestamp" | "level" | "logger" | "message"))
        // `event` only stays a field when it is not doing duty as the message.
        .filter(|(k, _)| k.as_str() != "event" || has_message)
        .map(|(k, v)| (k.clone(), render(v)))
        .collect();
    // Sorted rather than left in the map's order: whether a `serde_json::Map`
    // is a `BTreeMap` or insertion-ordered depends on the `preserve_order`
    // feature, which the *server* crate turns on and cargo then unifies into
    // this one for a whole-workspace build. Without this the columns reorder
    // depending on how the binary was built.
    fields.sort();
    Some(Entry {
        level: string(&obj, "level").as_deref().and_then(Level::parse),
        time: string(&obj, "timestamp").as_deref().map(clock),
        source: string(&obj, "logger"),
        message,
        fields,
    })
}

fn prefixed(line: &str) -> Option<Entry> {
    // `[shell] restarting the server` — our own notes around the sidecar.
    if let Some((tag, text)) = line.strip_prefix('[').and_then(|r| r.split_once(']')) {
        return Some(Entry {
            source: Some(tag.to_string()),
            message: text.trim_start().to_string(),
            ..Entry::default()
        });
    }
    // `INFO:     Started server process [57620]` — uvicorn's own logger.
    let (head, rest) = line.split_once(':')?;
    Some(Entry {
        level: Some(Level::parse(head)?),
        message: rest.trim_start().to_string(),
        ..Entry::default()
    })
}

fn string(obj: &Map<String, Value>, key: &str) -> Option<String> {
    obj.get(key).and_then(Value::as_str).map(str::to_string)
}

fn render(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `2026-08-05T23:35:09.955547+00:00` → `23:35:09.955`. The date repeats on
/// every line and the offset never moves, so neither earns column width.
fn clock(ts: &str) -> String {
    let time = ts.split_once('T').map(|(_, t)| t).unwrap_or(ts);
    let time: String =
        time.chars().take_while(|c| c.is_ascii_digit() || *c == ':' || *c == '.').collect();
    match time.split_once('.') {
        Some((hms, frac)) => format!("{hms}.{}", &frac[..frac.len().min(3)]),
        None => time,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structlog_line_splits_into_columns() {
        let e = parse(
            r#"{"timestamp": "2026-08-05T23:35:09.966483+00:00", "level": "INFO", "logger": "agent_platform.request", "message": "request completed", "status_code": 200, "duration_ms": 2}"#,
        );
        assert_eq!(e.level, Some(Level::Info));
        assert_eq!(e.time.as_deref(), Some("23:35:09.966"));
        assert_eq!(e.source.as_deref(), Some("agent_platform.request"));
        assert_eq!(e.message, "request completed");
        assert_eq!(
            e.fields,
            vec![("duration_ms".into(), "2".into()), ("status_code".into(), "200".into())]
        );
    }

    #[test]
    fn event_heads_the_row_only_when_there_is_no_message() {
        let with = parse(r#"{"message": "request completed", "event": "request.completed"}"#);
        assert_eq!(with.message, "request completed");
        assert_eq!(with.fields, vec![("event".into(), "request.completed".into())]);

        let without = parse(r#"{"event": "request.completed"}"#);
        assert_eq!(without.message, "request.completed");
        assert!(without.fields.is_empty());
    }

    #[test]
    fn uvicorn_and_tagged_lines_keep_their_level_and_source() {
        let uv = parse(r#"INFO:     127.0.0.1:64519 - "GET /health HTTP/1.1" 200 OK"#);
        assert_eq!(uv.level, Some(Level::Info));
        assert_eq!(uv.source, None);
        assert_eq!(uv.message, r#"127.0.0.1:64519 - "GET /health HTTP/1.1" 200 OK"#);

        let tagged = parse("[shell] restarting the server");
        assert_eq!(tagged.source.as_deref(), Some("shell"));
        assert_eq!(tagged.message, "restarting the server");
        assert_eq!(tagged.level, None);
    }

    #[test]
    fn anything_unparsed_survives_whole() {
        for line in ["{ not really json", "Application startup complete.", ""] {
            let e = parse(line);
            assert_eq!(e.message, line);
            assert_eq!(e.level, None);
            assert!(e.fields.is_empty());
        }
    }
}
