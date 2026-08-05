//! Shapes shared by every migrated domain: how a timestamp and a field-length
//! failure look on the wire. Both are contract, not formatting preference.

use chrono::NaiveDateTime;
use serde_json::Value;

use crate::error::ApiError;

/// Python hands timestamps to the JSON encoder as `datetime`, which renders
/// `datetime.isoformat()` — microseconds only when non-zero.
pub fn iso8601<S: serde::Serializer>(at: &NaiveDateTime, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&iso_string(*at))
}

pub fn iso_string(at: NaiveDateTime) -> String {
    if at.and_utc().timestamp_subsec_micros() == 0 {
        at.format("%Y-%m-%dT%H:%M:%S").to_string()
    } else {
        at.format("%Y-%m-%dT%H:%M:%S%.6f").to_string()
    }
}

pub fn iso_value(at: NaiveDateTime) -> Value {
    Value::String(iso_string(at))
}

/// Renders a timestamp column the way Python does, offset and all.
///
/// The same column holds both shapes: `utc_now_naive` writes
/// `2026-08-05 22:11:22.076026`, while older seed code wrote an aware value and
/// left `+00:00` on it. SQLAlchemy parses each back into a naive or aware
/// `datetime`, and pydantic then renders the aware one with a trailing `Z` —
/// so decoding to `NaiveDateTime` here would silently drop that suffix and
/// change what a seeded row looks like on the wire.
pub fn sql_time<S: serde::Serializer>(raw: &String, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&iso_from_sql(raw))
}

pub fn iso_from_sql(raw: &str) -> String {
    let raw = raw.trim();
    // An offset can only start past the date, whose own dashes would match first.
    let (body, offset) = match raw.rfind(['+', '-']) {
        Some(i) if i > 10 => (&raw[..i], &raw[i..]),
        _ => (raw, ""),
    };
    let mut out = body.replacen(' ', "T", 1);
    if let Some(dot) = out.find('.') {
        // SQLAlchemy's SQLite parser reads six fractional digits and ignores the
        // rest, and `isoformat()` omits them entirely when they are zero.
        if out[dot + 1..].chars().all(|c| c == '0') {
            out.truncate(dot);
        } else if out.len() > dot + 7 {
            out.truncate(dot + 7);
        }
    }
    out.push_str(if offset == "+00:00" { "Z" } else { offset });
    out
}

/// A timestamp in exactly the text SQLAlchemy writes.
///
/// Binding a `NaiveDateTime` instead stores nanoseconds — Windows' clock has
/// 100 ns ticks — and the same row then renders `…036520900` here and
/// `…036520` from Python, which is a diff in every response that carries a
/// timestamp we wrote.
pub fn sql_now() -> String {
    chrono::Utc::now().naive_utc().format("%Y-%m-%d %H:%M:%S%.6f").to_string()
}

#[cfg(test)]
mod tests {
    use super::iso_from_sql;

    #[test]
    fn renders_both_stored_shapes() {
        assert_eq!(iso_from_sql("2026-08-05 22:11:22.076026"), "2026-08-05T22:11:22.076026");
        assert_eq!(iso_from_sql("2026-08-05 22:11:22.076026+00:00"), "2026-08-05T22:11:22.076026Z");
        assert_eq!(iso_from_sql("2026-08-05 22:11:22.000000"), "2026-08-05T22:11:22");
        assert_eq!(iso_from_sql("2026-08-05 22:11:22.000000+00:00"), "2026-08-05T22:11:22Z");
        assert_eq!(iso_from_sql("2026-08-05 22:11:22-05:00"), "2026-08-05T22:11:22-05:00");
    }
}

/// Appends the pydantic-style entries for one optional string field.
pub fn check_len(errors: &mut Vec<Value>, loc: &[&str], value: Option<&str>, min: usize, max: usize) {
    let Some(value) = value else { return };
    let len = value.chars().count();
    if len < min {
        errors.push(ApiError::field_error_at(
            loc,
            "string_too_short",
            &format!("String should have at least {min} character{}", plural(min)),
        ));
    } else if len > max {
        errors.push(ApiError::field_error_at(
            loc,
            "string_too_long",
            &format!("String should have at most {max} character{}", plural(max)),
        ));
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}
