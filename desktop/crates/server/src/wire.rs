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

/// Nullable columns: `null` stays `null` rather than becoming an epoch.
pub fn sql_time_opt<S: serde::Serializer>(raw: &Option<String>, s: S) -> Result<S::Ok, S::Error> {
    match raw {
        Some(raw) => s.serialize_str(&iso_from_sql(raw)),
        None => s.serialize_none(),
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

/// A caller's ISO-8601 timestamp in the text SQLAlchemy would have written.
///
/// The column is a naive `DateTime`, and SQLAlchemy's SQLite bind processor
/// reads only the wall-clock fields — an offset is **dropped, never applied**,
/// so `09:00Z` and `09:00+02:00` both land as `09:00`. Converting to UTC first
/// would move every scheduled item by the caller's offset relative to what
/// Python does with the same request. Pydantic also accepts a space separator,
/// and the stored text always carries six fractional digits, like `sql_now`.
pub fn datetime_to_sql(raw: &str) -> String {
    parse_naive(raw)
        .map(sql_string)
        // Pydantic would 422 anything left over; store it as the old helper did.
        .unwrap_or_else(|| raw.trim().replacen('T', " ", 1))
}

/// Everything `datetime.fromisoformat` accepts that this codebase can send:
/// an offset, a `Z`, a space separator, a bare date, and optional fractions.
/// `None` is a value pydantic would have rejected before the handler saw it.
pub fn parse_naive(raw: &str) -> Option<NaiveDateTime> {
    // Only the date/time separator is swapped; a trailing offset keeps its own.
    let iso = raw.trim().replacen(' ', "T", 1).replace('Z', "+00:00");
    chrono::DateTime::parse_from_rfc3339(&iso)
        // `naive_local`, not `naive_utc`: the offset is dropped, never applied.
        .map(|at| at.naive_local())
        .or_else(|_| NaiveDateTime::parse_from_str(&iso, "%Y-%m-%dT%H:%M:%S%.f"))
        .or_else(|_| NaiveDateTime::parse_from_str(&iso, "%Y-%m-%dT%H:%M"))
        .or_else(|_| {
            chrono::NaiveDate::parse_from_str(&iso, "%Y-%m-%d")
                .map(|d| d.and_hms_opt(0, 0, 0).unwrap_or_default())
        })
        .ok()
}

/// The text SQLAlchemy binds for a naive `DateTime`: space separator, always six
/// fractional digits.
pub fn sql_string(at: NaiveDateTime) -> String {
    at.format("%Y-%m-%d %H:%M:%S%.6f").to_string()
}

#[cfg(test)]
mod tests {
    use super::{datetime_to_sql, iso_from_sql};

    #[test]
    fn iso_input_drops_the_offset_and_keeps_the_wall_clock() {
        assert_eq!(datetime_to_sql("2026-08-06T09:00:00Z"), "2026-08-06 09:00:00.000000");
        // Not 07:00: converting would move every scheduled item by the offset.
        assert_eq!(datetime_to_sql("2026-08-06T09:00:00+02:00"), "2026-08-06 09:00:00.000000");
        assert_eq!(datetime_to_sql("2026-08-06T09:00:00"), "2026-08-06 09:00:00.000000");
        assert_eq!(datetime_to_sql("2026-08-06T09:00:00.123456"), "2026-08-06 09:00:00.123456");
        assert_eq!(datetime_to_sql("2026-08-06 09:00:00"), "2026-08-06 09:00:00.000000");
    }

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
