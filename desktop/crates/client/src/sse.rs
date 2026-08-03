//! SSE stream for `GET /api/v1/processes/{id}/stream`.
//!
//! Server contract (see `app/process_routes.py`): data-only frames — no `event:`
//! field — carrying either a log row `{task_id, type, content, timestamp}` or a
//! sentinel `{"type": "terminal"|"error", "content": ...}`. `:` comment lines are
//! keep-alives. The stream self-terminates on terminal/approval states.
//!
//! Consumers may treat any frame as a "refetch now" trigger (the web UI does);
//! the payload is still parsed so terminal/error can end subscriptions.

use crate::client::Client;
use futures::stream::Stream;
use futures::StreamExt;
use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
pub struct StreamEvent {
    #[serde(default)]
    pub task_id: Option<i64>,
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(default)]
    pub content: serde_json::Value,
    #[serde(default)]
    pub timestamp: Option<String>,
}

impl StreamEvent {
    /// Sentinel frames (`{"type":"terminal"|"error"}`) carry no `task_id`/`timestamp`;
    /// ordinary event-log rows always do — including rows whose `event_type` is
    /// `"error"` (e.g. a planning failure). Don't conflate the two.
    fn is_sentinel(&self) -> bool {
        self.task_id.is_none() && self.timestamp.is_none()
    }
    pub fn is_terminal(&self) -> bool {
        self.event_type == "terminal" && self.is_sentinel()
    }
    pub fn is_error(&self) -> bool {
        self.event_type == "error" && self.is_sentinel()
    }
}

#[derive(Debug, Clone)]
pub enum SseItem {
    Event(StreamEvent),
    /// A data frame that did not parse as a `StreamEvent`; raw payload preserved.
    Raw(String),
    /// Stream ended without a terminal frame; reconnect scheduled after the delay.
    Reconnecting { attempt: u32, delay: Duration },
}

/// Reconnect backoff mirroring `web/src/hooks/useProcessEventStream.ts`:
/// `min(30s, 500ms * 2^min(attempt-1, 6))`, attempt counter reset on any frame.
pub fn backoff(attempt: u32) -> Duration {
    let exp = attempt.saturating_sub(1).min(6);
    Duration::from_millis((500u64 << exp).min(30_000))
}

/// Extract complete `data:` payloads from an SSE byte buffer. Returns the
/// payloads of every complete (blank-line-terminated) frame; leftover bytes
/// stay in `buf`. Keep-alive comment lines are dropped.
pub fn drain_frames(buf: &mut String) -> Vec<String> {
    let mut out = Vec::new();
    // Normalize CRLF so frame splitting only deals with \n\n.
    while let Some(pos) = buf.find("\r\n") {
        buf.replace_range(pos..pos + 2, "\n");
    }
    while let Some(pos) = buf.find("\n\n") {
        let frame: String = buf[..pos].to_string();
        buf.drain(..pos + 2);
        for line in frame.lines() {
            if let Some(data) = line.strip_prefix("data:") {
                out.push(data.strip_prefix(' ').unwrap_or(data).to_string());
            }
            // lines starting with ':' are keep-alives; anything else is ignored
        }
    }
    out
}

/// Open the SSE stream for a process, reconnecting with backoff until a
/// terminal/error sentinel arrives (then the stream ends).
///
/// Caveat (matches server behavior in `app/process_routes.py`): a process that is
/// already terminal with a backlog of rows replays them and closes with NO
/// sentinel — the stream then yields `Reconnecting` forever. Consumers must gate
/// the subscription on polled process status, exactly as the web UI did.
pub fn process_stream(client: Client, process_id: i64) -> impl Stream<Item = SseItem> {
    async_stream(client, process_id)
}

fn async_stream(client: Client, process_id: i64) -> impl Stream<Item = SseItem> {
    futures::stream::unfold(StreamState::new(client, process_id), |mut st| async move {
        let item = st.next().await?;
        Some((item, st))
    })
}

struct StreamState {
    client: Client,
    process_id: i64,
    attempt: u32,
    done: bool,
    current: Option<CurrentConn>,
    pending: std::collections::VecDeque<SseItem>,
}

struct CurrentConn {
    stream: std::pin::Pin<
        Box<dyn Stream<Item = std::result::Result<bytes::Bytes, reqwest::Error>> + Send>,
    >,
    buf: String,
}

impl StreamState {
    fn new(client: Client, process_id: i64) -> Self {
        Self {
            client,
            process_id,
            attempt: 0,
            done: false,
            current: None,
            pending: Default::default(),
        }
    }

    async fn next(&mut self) -> Option<SseItem> {
        loop {
            if let Some(item) = self.pending.pop_front() {
                return Some(item);
            }
            if self.done {
                return None;
            }

            if self.current.is_none() {
                if self.attempt > 0 {
                    let delay = backoff(self.attempt);
                    tokio::time::sleep(delay).await;
                }
                self.attempt += 1;
                let url = self
                    .client
                    .url(&format!("/api/v1/processes/{}/stream", self.process_id));
                let resp = self
                    .client
                    .authed(
                        self.client
                            .http()
                            .get(url)
                            .header("Accept", "text/event-stream"),
                    )
                    .send()
                    .await;
                match resp {
                    Ok(r) if r.status().is_success() => {
                        self.current = Some(CurrentConn {
                            stream: Box::pin(r.bytes_stream()),
                            buf: String::new(),
                        });
                    }
                    _ => {
                        return Some(SseItem::Reconnecting {
                            attempt: self.attempt,
                            delay: backoff(self.attempt + 1),
                        });
                    }
                }
            }

            let conn = self.current.as_mut().unwrap();
            match conn.stream.next().await {
                Some(Ok(chunk)) => {
                    conn.buf.push_str(&String::from_utf8_lossy(&chunk));
                    for payload in drain_frames(&mut conn.buf) {
                        // Any received frame resets the reconnect counter.
                        self.attempt = 0;
                        match serde_json::from_str::<StreamEvent>(&payload) {
                            Ok(ev) => {
                                if ev.is_terminal() || ev.is_error() {
                                    self.done = true;
                                }
                                self.pending.push_back(SseItem::Event(ev));
                            }
                            Err(_) => self.pending.push_back(SseItem::Raw(payload)),
                        }
                    }
                }
                Some(Err(_)) | None => {
                    self.current = None;
                    if !self.done {
                        return Some(SseItem::Reconnecting {
                            attempt: self.attempt.max(1),
                            delay: backoff(self.attempt.max(1) + 1),
                        });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drains_complete_frames_only() {
        let mut buf = "data: {\"a\":1}\n\ndata: partial".to_string();
        let frames = drain_frames(&mut buf);
        assert_eq!(frames, vec!["{\"a\":1}"]);
        assert_eq!(buf, "data: partial");
    }

    #[test]
    fn drops_keepalive_comments() {
        let mut buf = ": ping\n\ndata: x\n\n".to_string();
        assert_eq!(drain_frames(&mut buf), vec!["x"]);
    }

    #[test]
    fn handles_crlf() {
        let mut buf = "data: y\r\n\r\n".to_string();
        assert_eq!(drain_frames(&mut buf), vec!["y"]);
    }

    #[test]
    fn multiple_frames_one_chunk() {
        let mut buf = "data: 1\n\ndata: 2\n\n".to_string();
        assert_eq!(drain_frames(&mut buf), vec!["1", "2"]);
    }

    #[test]
    fn backoff_caps_at_30s() {
        assert_eq!(backoff(1), Duration::from_millis(500));
        assert_eq!(backoff(2), Duration::from_millis(1000));
        assert_eq!(backoff(7), Duration::from_millis(30_000));
        assert_eq!(backoff(100), Duration::from_millis(30_000));
    }

    #[test]
    fn terminal_event_detection() {
        let ev: StreamEvent =
            serde_json::from_str(r#"{"type":"terminal","content":"completed"}"#).unwrap();
        assert!(ev.is_terminal());
        let ev: StreamEvent =
            serde_json::from_str(r#"{"task_id":3,"type":"log","content":"hi","timestamp":"t"}"#)
                .unwrap();
        assert!(!ev.is_terminal());
        assert_eq!(ev.task_id, Some(3));
        // An event-log row with event_type "error" is NOT the error sentinel.
        let ev: StreamEvent = serde_json::from_str(
            r#"{"task_id":null,"type":"error","content":"Planning failed","timestamp":"t"}"#,
        )
        .unwrap();
        assert!(!ev.is_error());
        let sentinel: StreamEvent =
            serde_json::from_str(r#"{"type":"error","content":"process not found"}"#).unwrap();
        assert!(sentinel.is_error());
    }
}
