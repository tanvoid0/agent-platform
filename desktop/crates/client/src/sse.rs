//! SSE stream for `GET /api/v1/processes/{id}/stream`.
//!
//! Server contract (see `app/process_routes.py`): data-only frames — no `event:`
//! field — carrying either a log row `{task_id, type, content, timestamp}` or a
//! sentinel `{"type": "terminal"|"error", "content": ...}`. `:` comment lines are
//! keep-alives. The stream self-terminates on terminal/approval states.
//!
//! Consumers may treat any frame as a "refetch now" trigger (the web UI does);
//! the payload is still parsed so terminal/error can end subscriptions.

use crate::client::{detail_message, Client};
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

/// One item from a streaming chat completion.
#[derive(Debug, Clone)]
pub enum ChatChunk {
    /// A piece of assistant text. Concatenating every `Delta` yields the full reply.
    Delta(String),
    /// The stream ended cleanly (`data: [DONE]`, or the body closed).
    Done,
    /// Transport failure or an error frame from the proxy; the stream ends after this.
    Failed(String),
}

/// Extract the assistant text delta from one `chat.completion.chunk` payload.
///
/// Returns `None` for frames that carry no text (role-only first chunk, the final
/// `finish_reason` chunk, tool-call deltas). An `error` object — which the proxy
/// injects mid-stream when the upstream dies (`sse_error_chunk`) — comes back as
/// `Err` so the caller can surface it instead of silently truncating the reply.
pub fn chat_delta(payload: &str) -> std::result::Result<Option<String>, String> {
    let v: serde_json::Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        // A non-JSON data frame is not worth failing the whole reply over.
        Err(_) => return Ok(None),
    };
    if let Some(msg) = v.pointer("/error/message").and_then(|m| m.as_str()) {
        return Err(msg.to_string());
    }
    Ok(v.pointer("/choices/0/delta/content")
        .and_then(|c| c.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string))
}

/// Stream `POST /api/v1/chat` with `stream: true`, yielding assistant text deltas.
///
/// Unlike `process_stream` this never reconnects: a chat completion is not
/// resumable, so a dropped connection ends the stream with `Failed` and the
/// caller keeps whatever text arrived.
pub fn chat_stream(
    client: Client,
    body: crate::types::ChatCompletionBody,
) -> impl Stream<Item = ChatChunk> {
    let mut body = body;
    body.stream = Some(true);
    futures::stream::unfold(ChatState::Start(client, Box::new(body)), |st| async move {
        match st {
            ChatState::Start(client, body) => {
                let url = client.url("/api/v1/chat");
                let resp = client
                    .authed(
                        client
                            .http()
                            .post(url)
                            .header("Accept", "text/event-stream")
                            .json(&*body),
                    )
                    .send()
                    .await;
                match resp {
                    Ok(r) if r.status().is_success() => {
                        let conn = CurrentConn { stream: Box::pin(r.bytes_stream()), buf: String::new() };
                        Some((None, ChatState::Reading(Box::new(conn), Default::default())))
                    }
                    // Errors arrive as a normal JSON body (the route returns the
                    // upstream status before it starts streaming).
                    Ok(r) => {
                        let status = r.status().as_u16();
                        let text = r.text().await.unwrap_or_default();
                        let msg = serde_json::from_str::<serde_json::Value>(&text)
                            .map(|v| detail_message(&v))
                            .unwrap_or(text);
                        Some((Some(ChatChunk::Failed(format!("HTTP {status}: {msg}"))), ChatState::End))
                    }
                    Err(e) => Some((Some(ChatChunk::Failed(e.to_string())), ChatState::End)),
                }
            }
            ChatState::Reading(mut conn, mut pending) => {
                if let Some(item) = pending.pop_front() {
                    let done = matches!(item, ChatChunk::Done | ChatChunk::Failed(_));
                    let next = if done { ChatState::End } else { ChatState::Reading(conn, pending) };
                    return Some((Some(item), next));
                }
                match conn.stream.next().await {
                    Some(Ok(chunk)) => {
                        conn.buf.push_str(&String::from_utf8_lossy(&chunk));
                        for payload in drain_frames(&mut conn.buf) {
                            if payload.trim() == "[DONE]" {
                                pending.push_back(ChatChunk::Done);
                                break;
                            }
                            match chat_delta(&payload) {
                                Ok(Some(text)) => pending.push_back(ChatChunk::Delta(text)),
                                Ok(None) => {}
                                Err(msg) => {
                                    pending.push_back(ChatChunk::Failed(msg));
                                    break;
                                }
                            }
                        }
                        Some((None, ChatState::Reading(conn, pending)))
                    }
                    Some(Err(e)) => Some((Some(ChatChunk::Failed(e.to_string())), ChatState::End)),
                    // Body closed without [DONE]: whatever arrived is the reply.
                    None => Some((Some(ChatChunk::Done), ChatState::End)),
                }
            }
            ChatState::End => None,
        }
    })
    .filter_map(|item| async move { item })
}

enum ChatState {
    Start(Client, Box<crate::types::ChatCompletionBody>),
    Reading(Box<CurrentConn>, std::collections::VecDeque<ChatChunk>),
    End,
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
    fn chat_delta_reads_content_and_skips_empty_frames() {
        let text = r#"{"choices":[{"delta":{"content":"Hi"}}]}"#;
        assert_eq!(chat_delta(text).unwrap().as_deref(), Some("Hi"));
        // Role-only opener and the finish chunk carry no text.
        assert_eq!(chat_delta(r#"{"choices":[{"delta":{"role":"assistant"}}]}"#).unwrap(), None);
        assert_eq!(
            chat_delta(r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#).unwrap(),
            None
        );
        assert_eq!(chat_delta("[DONE]").unwrap(), None, "non-JSON frames are ignored");
    }

    #[test]
    fn chat_delta_surfaces_a_mid_stream_error_frame() {
        // What llm_proxy's sse_error_chunk emits when the upstream dies.
        let frame = r#"{"error":{"message":"upstream gone","code":"upstream_error"}}"#;
        assert_eq!(chat_delta(frame).unwrap_err(), "upstream gone");
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
