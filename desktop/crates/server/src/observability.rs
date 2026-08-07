//! The in-process log ring `GET /system/logs` serves, ported from
//! `app/observability.py`'s `RingBufferHandler`.
//!
//! Python got this for free: every module logged through the root logger, and a
//! handler on that root formatted each record as JSON and kept the last N. This
//! crate has no logging framework — it writes diagnostics with `eprintln!` and
//! one request line with `println!` — so the ring is fed explicitly, by
//! [`logd!`] for diagnostics and by [`record`] from
//! [`crate::request_id::middleware`] for the request line.
//!
//! **stdout/stderr stay the primary destination.** [`logd!`] still writes the
//! same `[agent-platformd] …` line to stderr it wrote before, because that is
//! what the desktop shell captures from this process and what covers startup
//! and crashes — the two things a ring served over HTTP cannot, since it only
//! answers while the server is up. The ring is the addressable copy, not a
//! replacement.
//!
//! Sequence numbers are global and monotonic, so a poller passes back the
//! `next` it was given and gets only what has been written since. `dropped` is
//! non-zero once the ring has wrapped past what that poller last saw, which is
//! the difference between a gap it can see and a gap it cannot.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use serde_json::json;

/// The ring's capacity, `AGENT_PLATFORM_LOG_RING` or 2000 — Python's default.
fn capacity() -> usize {
    crate::env_opt("AGENT_PLATFORM_LOG_RING")
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(2000)
}

struct Ring {
    records: VecDeque<(u64, String)>,
    next_seq: u64,
    capacity: usize,
}

fn ring() -> &'static Mutex<Ring> {
    static RING: OnceLock<Mutex<Ring>> = OnceLock::new();
    RING.get_or_init(|| {
        let capacity = capacity();
        Mutex::new(Ring { records: VecDeque::with_capacity(capacity.min(256)), next_seq: 0, capacity })
    })
}

/// Append one already-formatted JSON line.
///
/// A poisoned lock is ignored rather than propagated: a panic in some other
/// handler must not turn every subsequent log call into a second panic, and
/// losing a diagnostic line is the smaller failure. The line has already gone
/// to the console by the time this is reached.
pub fn record(line: &str) {
    let Ok(mut ring) = ring().lock() else { return };
    if ring.records.len() == ring.capacity {
        ring.records.pop_front();
    }
    let seq = ring.next_seq;
    ring.next_seq += 1;
    ring.records.push_back((seq, line.to_string()));
}

/// `RingBufferHandler.snapshot` — `{lines, next, dropped}`.
pub fn snapshot(after: u64) -> serde_json::Value {
    let Ok(ring) = ring().lock() else {
        return json!({ "lines": [], "next": after, "dropped": 0 });
    };
    // `oldest = records[0][0] if records else after` — an empty ring reports no
    // gap rather than a gap of everything.
    let oldest = ring.records.front().map_or(after, |(seq, _)| *seq);
    let lines: Vec<&str> = ring
        .records
        .iter()
        .filter(|(seq, _)| *seq >= after)
        .map(|(_, line)| line.as_str())
        .collect();
    json!({
        "lines": lines,
        "next": ring.next_seq,
        "dropped": oldest.saturating_sub(after),
    })
}

/// The body of [`logd!`]. Public because the macro expands at the call site.
///
/// `WARNING` rather than `ERROR` for all of them: every call this replaced was
/// an `eprintln!` on a path that *degraded* — a usage counter that did not
/// increment, a sub-DAG that did not expand — and none of them fail the
/// request. Python's own equivalents log at warning too.
pub fn diagnostic(message: &str) {
    // The one `eprintln!` left in the crate: everything else routes through
    // `logd!`, which lands here. Writing it any other way is a recursion.
    eprintln!("[agent-platformd] {message}");
    let line = json!({
        "timestamp": crate::request_id::iso_now(),
        "level": "WARNING",
        "logger": "agent_platformd",
        "message": message,
        "request_id": crate::request_id::current(),
    });
    // `JsonLogFormatter` omits `request_id` when there is none rather than
    // writing a null, and the Logs screen filters on the key's presence.
    let line = match line {
        serde_json::Value::Object(mut map) => {
            if map.get("request_id").is_some_and(serde_json::Value::is_null) {
                map.remove("request_id");
            }
            serde_json::Value::Object(map)
        }
        other => other,
    };
    record(&line.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives the ring through its own struct rather than the global, so the
    /// test does not depend on what else in the suite has logged.
    fn fresh(capacity: usize) -> Ring {
        Ring { records: VecDeque::new(), next_seq: 0, capacity }
    }

    fn push(ring: &mut Ring, line: &str) {
        if ring.records.len() == ring.capacity {
            ring.records.pop_front();
        }
        let seq = ring.next_seq;
        ring.next_seq += 1;
        ring.records.push_back((seq, line.to_string()));
    }

    fn snap(ring: &Ring, after: u64) -> (Vec<String>, u64, u64) {
        let oldest = ring.records.front().map_or(after, |(seq, _)| *seq);
        let lines = ring
            .records
            .iter()
            .filter(|(seq, _)| *seq >= after)
            .map(|(_, l)| l.clone())
            .collect();
        (lines, ring.next_seq, oldest.saturating_sub(after))
    }

    #[test]
    fn a_poller_sees_each_line_once() {
        let mut ring = fresh(10);
        push(&mut ring, "a");
        push(&mut ring, "b");

        let (lines, next, dropped) = snap(&ring, 0);
        assert_eq!(lines, ["a", "b"]);
        assert_eq!((next, dropped), (2, 0));

        // Polling back with `next` returns nothing until something new lands.
        assert_eq!(snap(&ring, next).0, Vec::<String>::new());
        push(&mut ring, "c");
        assert_eq!(snap(&ring, next).0, ["c"]);
    }

    /// The point of `dropped`: a slow poller is told it missed lines instead of
    /// silently receiving a shorter list.
    #[test]
    fn wrapping_past_a_poller_is_visible_to_it() {
        let mut ring = fresh(3);
        for line in ["a", "b", "c", "d", "e"] {
            push(&mut ring, line);
        }
        let (lines, next, dropped) = snap(&ring, 0);
        assert_eq!(lines, ["c", "d", "e"]);
        assert_eq!(next, 5);
        assert_eq!(dropped, 2, "'a' and 'b' fell out of a 3-slot ring");

        // Caught up, so nothing is reported dropped even though the ring wrapped.
        assert_eq!(snap(&ring, 5).2, 0);
    }

    #[test]
    fn an_empty_ring_reports_no_gap() {
        let ring = fresh(3);
        assert_eq!(snap(&ring, 7), (vec![], 0, 0));
    }
}
