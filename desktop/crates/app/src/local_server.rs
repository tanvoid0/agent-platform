//! An OpenAI-compatible endpoint in front of [`crate::local_llm`], so the Python
//! server's agents can answer on this machine's model too (ADR 0006, step 5).
//!
//! Off unless `local_server_port` is set. Point the proxy's OpenAI-compatible
//! provider at it and nothing on the Python side changes:
//!
//! ```text
//! LM_STUDIO_API_BASE=http://127.0.0.1:18411
//! ```
//!
//! Only what that provider actually calls is implemented: `GET /v1/models` for
//! discovery and `POST /v1/chat/completions`, streaming or buffered, with
//! `tools` in and `tool_calls` out — an agent turn is the reason the server
//! would point here at all. Bound to
//! loopback and unauthenticated, which is the same trust boundary Ollama and LM
//! Studio draw — any process on this machine can already reach those.
//!
//! Hand-rolled HTTP rather than a web framework: two routes, one client, and the
//! connection is closed after every response, so there is no keep-alive, no
//! chunked request body and no router to justify a dependency.
// ponytail: one thread per connection, blocking IO. The engine answers one turn
// at a time anyway, so a pool would queue on the same lock.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};

use agent_platform_client::sse::{ChatChunk, ToolCallDelta};
use agent_platform_client::types::{ChatCompletionBody, ChatMessage};

/// Start the listener on a background thread. `Err` is a port that could not be
/// bound — the caller logs it and carries on without the endpoint.
pub fn start(port: u16) -> std::io::Result<SocketAddr> {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))?;
    let addr = listener.local_addr()?;
    std::thread::Builder::new().name("local-llm-http".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let _ = handle(stream);
            });
        }
    })?;
    Ok(addr)
}

/// Method, path and body of one request. Headers past `Content-Length` are not
/// read: nothing here varies by them.
struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn read_request(stream: &mut BufReader<&TcpStream>) -> std::io::Result<Option<Request>> {
    let mut line = String::new();
    if stream.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let (method, path) = match (parts.next(), parts.next()) {
        (Some(m), Some(p)) => (m.to_string(), p.to_string()),
        _ => return Ok(None),
    };

    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if stream.read_line(&mut header)? == 0 {
            break;
        }
        if header.trim().is_empty() {
            break;
        }
        if let Some(v) = header.to_ascii_lowercase().strip_prefix("content-length:") {
            length = v.trim().parse().unwrap_or(0);
        }
    }

    let mut body = vec![0u8; length];
    if length > 0 {
        stream.read_exact(&mut body)?;
    }
    Ok(Some(Request { method, path, body }))
}

fn handle(stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(&stream);
    let Some(req) = read_request(&mut reader)? else { return Ok(()) };
    let mut out = &stream;

    // The path may carry a query string; nothing here reads one.
    let path = req.path.split('?').next().unwrap_or("").to_string();
    match (req.method.as_str(), path.as_str()) {
        ("GET", "/v1/models") => write_json(&mut out, 200, &models_json()),
        ("POST", "/v1/chat/completions") => completions(&mut out, &req.body),
        _ => write_json(&mut out, 404, &error_json("not found")),
    }
}

fn models_json() -> String {
    let data = match crate::local_llm::model_id() {
        // `created` is required by the shape and meaningless here.
        Some(id) => format!(
            r#"{{"id":{},"object":"model","created":0,"owned_by":"agent-platform"}}"#,
            json_string(&id)
        ),
        None => String::new(),
    };
    format!(r#"{{"object":"list","data":[{data}]}}"#)
}

fn error_json(message: &str) -> String {
    format!(r#"{{"error":{{"message":{}}}}}"#, json_string(message))
}

/// Read the OpenAI request into the body the engine takes. Only the fields the
/// engine acts on are carried over; the rest (`n`, `stop`, penalties) are
/// silently ignored, as a minimal upstream should.
fn parse_completion(raw: &[u8]) -> Result<(ChatCompletionBody, bool), String> {
    let v: serde_json::Value =
        serde_json::from_slice(raw).map_err(|e| format!("invalid JSON body: {e}"))?;
    let messages: Vec<ChatMessage> = v
        .get("messages")
        .and_then(|m| m.as_array())
        .ok_or("messages is required")?
        .iter()
        .map(|m| {
            ChatMessage::text(
                m.get("role").and_then(|r| r.as_str()).unwrap_or("user"),
                m.get("content").and_then(|c| c.as_str()).unwrap_or_default(),
            )
        })
        .collect();
    if messages.is_empty() {
        return Err("messages is empty".into());
    }
    let stream = v.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
    Ok((
        ChatCompletionBody {
            messages,
            // Provider and model are the caller naming an upstream, and this
            // *is* the upstream: whatever model is configured answers.
            model: None,
            provider: None,
            temperature: v.get("temperature").and_then(|t| t.as_f64()),
            max_tokens: v.get("max_tokens").and_then(|t| t.as_i64()),
            tools: v.get("tools").cloned(),
            stream: Some(stream),
        },
        stream,
    ))
}

fn completions(out: &mut impl Write, raw: &[u8]) -> std::io::Result<()> {
    let (body, stream) = match parse_completion(raw) {
        Ok(parsed) => parsed,
        Err(e) => return write_json(out, 400, &error_json(&e)),
    };
    if !crate::local_llm::available() {
        return write_json(out, 503, &error_json("no local model is configured"));
    }
    let id = crate::local_llm::model_id().unwrap_or_else(|| "local".into());

    if !stream {
        let mut text = String::new();
        let mut call = None;
        let mut failure = None;
        crate::local_llm::chat_blocking(body, |chunk| match chunk {
            ChatChunk::Delta(piece) => text.push_str(piece),
            ChatChunk::ToolCall(d) => call = Some(d.clone()),
            ChatChunk::Failed(e) => failure = Some(e.clone()),
            _ => {}
        });
        return match failure {
            Some(e) => write_json(out, 500, &error_json(&e)),
            None => write_json(out, 200, &completion_json(&id, &text, call.as_ref())),
        };
    }

    write_all(
        out,
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\
          Connection: close\r\n\r\n",
    )?;
    // Once the headers are out there is no way to report a failure as a status,
    // so a failed turn ends the stream like a finished one; the proxy sees a
    // short reply, which is what every other SSE upstream does too.
    let mut broken = false;
    let mut called = false;
    crate::local_llm::chat_blocking(body, |chunk| {
        if broken {
            return;
        }
        let frame = match chunk {
            ChatChunk::Delta(piece) => Some(chunk_json(&id, piece)),
            ChatChunk::ToolCall(d) => {
                called = true;
                Some(tool_call_chunk_json(&id, d))
            }
            ChatChunk::Done | ChatChunk::Failed(_) => Some(done_json(&id, called)),
            _ => None,
        };
        if let Some(frame) = frame {
            broken = write_all(out, format!("data: {frame}\n\n").as_bytes()).is_err();
        }
    });
    let _ = write_all(out, b"data: [DONE]\n\n");
    Ok(())
}

fn completion_json(model: &str, text: &str, call: Option<&ToolCallDelta>) -> String {
    // A turn is one or the other: [`crate::local_llm`] holds a recognised call
    // back from the stream, so `text` is empty whenever `call` is set.
    let (message, finish) = match call {
        Some(d) => (
            format!(r#"{{"role":"assistant","content":null,"tool_calls":[{}]}}"#, tool_call_json(d)),
            "tool_calls",
        ),
        None => {
            (format!(r#"{{"role":"assistant","content":{}}}"#, json_string(text)), "stop")
        }
    };
    format!(
        r#"{{"id":"chatcmpl-local","object":"chat.completion","created":0,"model":{},"choices":[{{"index":0,"message":{message},"finish_reason":"{finish}"}}]}}"#,
        json_string(model)
    )
}

/// One `tool_calls[i]`. The engine emits a whole call at once, so there is no
/// partial fragment to stitch and `arguments` is already complete JSON.
fn tool_call_json(d: &ToolCallDelta) -> String {
    format!(
        r#"{{"index":{},"id":{},"type":"function","function":{{"name":{},"arguments":{}}}}}"#,
        d.index,
        json_string(d.id.as_deref().unwrap_or("call_local")),
        json_string(d.name.as_deref().unwrap_or_default()),
        json_string(&d.arguments)
    )
}

fn tool_call_chunk_json(model: &str, d: &ToolCallDelta) -> String {
    format!(
        r#"{{"id":"chatcmpl-local","object":"chat.completion.chunk","created":0,"model":{},"choices":[{{"index":0,"delta":{{"tool_calls":[{}]}},"finish_reason":null}}]}}"#,
        json_string(model),
        tool_call_json(d)
    )
}

fn chunk_json(model: &str, piece: &str) -> String {
    format!(
        r#"{{"id":"chatcmpl-local","object":"chat.completion.chunk","created":0,"model":{},"choices":[{{"index":0,"delta":{{"content":{}}},"finish_reason":null}}]}}"#,
        json_string(model),
        json_string(piece)
    )
}

fn done_json(model: &str, called: bool) -> String {
    let finish = if called { "tool_calls" } else { "stop" };
    format!(
        r#"{{"id":"chatcmpl-local","object":"chat.completion.chunk","created":0,"model":{},"choices":[{{"index":0,"delta":{{}},"finish_reason":"{finish}"}}]}}"#,
        json_string(model)
    )
}

/// A string as a JSON literal, quotes included — the reply is model output, so
/// it carries newlines and quotes that have to survive the wire.
fn json_string(s: &str) -> String {
    serde_json::Value::String(s.to_string()).to_string()
}

fn write_json(out: &mut impl Write, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    write_all(out, head.as_bytes())?;
    write_all(out, body.as_bytes())
}

fn write_all(out: &mut impl Write, bytes: &[u8]) -> std::io::Result<()> {
    out.write_all(bytes)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The request reader is the one place a malformed client can wedge the
    /// thread, and the body length is what a POST depends on.
    #[test]
    fn a_request_is_read_up_to_its_content_length() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut c = TcpStream::connect(addr).unwrap();
            c.write_all(
                b"POST /v1/chat/completions HTTP/1.1\r\nHost: x\r\nContent-Length: 9\r\n\r\n\
                  {\"a\": 1}!trailing",
            )
            .unwrap();
        });
        let (stream, _) = listener.accept().unwrap();
        let req = read_request(&mut BufReader::new(&stream)).unwrap().expect("a request");
        assert_eq!((req.method.as_str(), req.path.as_str()), ("POST", "/v1/chat/completions"));
        // Exactly Content-Length bytes, so the trailing junk is left on the wire
        // rather than folded into the body.
        assert_eq!(req.body, b"{\"a\": 1}!");
    }

    #[test]
    fn a_completion_request_keeps_the_fields_the_engine_reads() {
        let raw = br#"{"model":"ignored","messages":[{"role":"user","content":"hi"}],
                      "stream":true,"temperature":0.5,"max_tokens":32}"#;
        let (body, stream) = parse_completion(raw).expect("parsed");
        assert!(stream);
        assert_eq!(body.messages.len(), 1);
        assert_eq!(body.messages[0].content, "hi");
        assert_eq!(body.temperature, Some(0.5));
        assert_eq!(body.max_tokens, Some(32));
        // The caller's model name is dropped on purpose: this endpoint *is* the
        // upstream, and honouring it would mean routing back out to the server.
        assert!(body.model.is_none());

        assert!(parse_completion(b"not json").is_err());
        assert!(parse_completion(br#"{"messages":[]}"#).is_err());
        assert!(parse_completion(br#"{"stream":true}"#).is_err());
    }

    /// Model output is not JSON-safe: a reply containing a quote or a newline
    /// must not break the frame it is carried in.
    #[test]
    fn a_reply_is_escaped_into_its_frame() {
        let frame = chunk_json("m", "say \"hi\"\nthen stop");
        let v: serde_json::Value = serde_json::from_str(&frame).expect("valid JSON chunk");
        assert_eq!(
            v.pointer("/choices/0/delta/content").and_then(|c| c.as_str()),
            Some("say \"hi\"\nthen stop")
        );
        let full: serde_json::Value = serde_json::from_str(&completion_json("m", "a\\b", None))
            .expect("valid JSON completion");
        assert_eq!(
            full.pointer("/choices/0/message/content").and_then(|c| c.as_str()),
            Some("a\\b")
        );
        assert!(serde_json::from_str::<serde_json::Value>(&done_json("m", false)).is_ok());
    }

    /// A tool turn is the point of pointing the server here, and OpenAI clients
    /// read the call off `tool_calls` with `finish_reason: tool_calls` — not off
    /// the content, where a JSON reply would look like prose.
    #[test]
    fn a_tool_call_leaves_as_tool_calls_not_as_content() {
        let call = ToolCallDelta {
            index: 0,
            id: Some("call_local_1".into()),
            name: Some("run_command".into()),
            arguments: r#"{"command":"dir \"C:\\Program Files\""}"#.into(),
        };

        let v: serde_json::Value = serde_json::from_str(&completion_json("m", "", Some(&call)))
            .expect("valid JSON completion");
        let tc = v.pointer("/choices/0/message/tool_calls/0").expect("a call");
        assert_eq!(tc["id"], "call_local_1");
        assert_eq!(tc["type"], "function");
        assert_eq!(tc.pointer("/function/name").unwrap(), "run_command");
        // Arguments cross the wire as a JSON *string*, so the quotes and
        // backslashes inside have to survive being escaped twice.
        assert_eq!(
            tc.pointer("/function/arguments").and_then(|a| a.as_str()),
            Some(call.arguments.as_str())
        );
        assert!(v.pointer("/choices/0/message/content").is_some_and(|c| c.is_null()));
        assert_eq!(v.pointer("/choices/0/finish_reason").unwrap(), "tool_calls");

        let chunk: serde_json::Value =
            serde_json::from_str(&tool_call_chunk_json("m", &call)).expect("valid JSON chunk");
        assert_eq!(
            chunk.pointer("/choices/0/delta/tool_calls/0/function/name").unwrap(),
            "run_command"
        );
        // The stream says why it stopped, or the caller runs nothing.
        let done: serde_json::Value =
            serde_json::from_str(&done_json("m", true)).expect("valid JSON done");
        assert_eq!(done.pointer("/choices/0/finish_reason").unwrap(), "tool_calls");
    }

    /// The whole path, over a real socket, with real weights: the endpoint is
    /// only worth anything if an OpenAI client gets an answer out of it.
    ///
    /// Ignored by default — it needs a GGUF, and run it on its own so it does
    /// not share a process (and the machine's memory) with the other
    /// model-backed test:
    ///
    /// ```bash
    /// AGENT_PLATFORM_TEST_GGUF=<path.gguf> cargo test --features local-llm -- --ignored local_server --nocapture
    /// ```
    #[test]
    #[ignore = "needs a GGUF via AGENT_PLATFORM_TEST_GGUF"]
    fn it_answers_a_completion_from_the_real_model() {
        let Ok(path) = std::env::var("AGENT_PLATFORM_TEST_GGUF") else { return };
        crate::local_llm::override_config(path.into(), 2048);
        assert!(crate::local_llm::available(), "AGENT_PLATFORM_TEST_GGUF is not a usable file");

        let addr = start(0).expect("bound");
        let body = br#"{"model":"whatever","messages":[{"role":"user","content":"Reply with the single word: pong"}],"max_tokens":16}"#;
        let mut c = TcpStream::connect(addr).unwrap();
        let head = format!(
            "POST /v1/chat/completions HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n",
            body.len()
        );
        c.write_all(head.as_bytes()).unwrap();
        c.write_all(body).unwrap();

        let mut response = String::new();
        c.read_to_string(&mut response).unwrap();
        let (head, json) = response.split_once("\r\n\r\n").expect("headers then body");
        assert!(head.starts_with("HTTP/1.1 200 OK"), "{response}");
        let v: serde_json::Value = serde_json::from_str(json).expect("valid JSON");
        let reply = v.pointer("/choices/0/message/content").and_then(|c| c.as_str());
        println!("endpoint replied: {reply:?}");
        assert!(reply.is_some_and(|r| !r.trim().is_empty()), "no reply in {json}");
    }

    /// A running endpoint has to answer discovery whether or not a model is
    /// configured — an empty list is the honest answer, not a dead socket.
    #[test]
    fn the_endpoint_answers_model_discovery() {
        let addr = start(0).expect("bound");
        let mut c = TcpStream::connect(addr).unwrap();
        c.write_all(b"GET /v1/models HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut response = String::new();
        c.read_to_string(&mut response).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").expect("headers then body");
        assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
        let v: serde_json::Value = serde_json::from_str(body).expect("valid JSON");
        assert_eq!(v["object"], "list");
        assert!(v["data"].is_array());

        let mut c = TcpStream::connect(addr).unwrap();
        c.write_all(b"GET /nope HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut response = String::new();
        c.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 404"), "{response}");
    }
}
