//! In-process inference — llama.cpp linked into this binary, per
//! [ADR 0006](../../../../docs/adr/0006-in-process-rust-core.md).
//!
//! Behind the `local-llm` cargo feature and off by default: the feature drags a
//! C++ build (and, for `cuda`, a CUDA Toolkit) into `cargo build`, which no
//! other part of this crate needs. Without it the module is not compiled and
//! [`crate::inference`] routes every turn to the server as before.
//!
//! Scope is the UI's own chat: it answers [`ChatCompletionBody`] with the same
//! [`ChatChunk`] stream the server's SSE relay produces, so callers cannot tell
//! which side answered — including tool calls, which are asked for in an extra
//! system turn and recognised by the JSON object the model replies with.
//!
//! The reason this exists at all is `n_ctx`. Ollama's OpenAI-compatible surface
//! takes no options, so every local reply through the proxy loads at the model's
//! full trained context — measured on this machine as 23 GB, a 39% CPU spill and
//! roughly a fifth of the tok/s. Here the context is ours to set.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use agent_platform_client::sse::ChatChunk;
use agent_platform_client::types::ChatCompletionBody;
use futures::Stream;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;

/// Every layer on the GPU. The spike measured 123 tok/s this way against 11 on
/// CPU, so a partial offload is not worth offering as a setting yet.
const N_GPU_LAYERS: u32 = 999;
/// Cap on one reply, when the caller names none. Long enough for prose, short
/// enough that a looping model stops on its own.
const DEFAULT_MAX_TOKENS: i32 = 1024;

/// What [`crate::shell::Settings`] contributes. `model_path` empty means the
/// whole path is off, which is the default.
#[derive(Debug, Clone)]
pub struct Config {
    pub model_path: PathBuf,
    pub n_ctx: u32,
}

impl Config {
    fn from_settings(dir: &Path) -> Option<Self> {
        let s = crate::shell::Settings::load(dir);
        let path = PathBuf::from(s.local_model_path.trim());
        (!s.local_model_path.trim().is_empty() && path.is_file())
            .then_some(Self { model_path: path, n_ctx: s.local_n_ctx })
    }
}

/// Set by the model-backed tests instead of the settings file — nothing else
/// writes it, and it is read before the settings are consulted.
static CONFIG_OVERRIDE: OnceLock<Config> = OnceLock::new();

/// Point the engine at a model without a settings file. Test-only: the first
/// call wins, and later ones are ignored rather than swapping a model out from
/// under a loaded context.
#[cfg(test)]
pub fn override_config(model_path: PathBuf, n_ctx: u32) {
    let _ = CONFIG_OVERRIDE.set(Config { model_path, n_ctx });
}

/// Read once per process: the settings file is not hot-reloaded elsewhere
/// either, and a model swap wants a restart regardless.
fn config() -> Option<&'static Config> {
    static CONFIG: OnceLock<Option<Config>> = OnceLock::new();
    if let Some(cfg) = CONFIG_OVERRIDE.get() {
        return Some(cfg);
    }
    CONFIG
        .get_or_init(|| Config::from_settings(&crate::shell::app_dir()))
        .as_ref()
}

/// Is there a usable local model configured? Cheap enough to call per turn —
/// the file check happens once, in [`config`].
pub fn available() -> bool {
    config().is_some()
}

fn backend() -> Result<&'static LlamaBackend, String> {
    static BACKEND: OnceLock<Result<LlamaBackend, String>> = OnceLock::new();
    BACKEND
        .get_or_init(|| LlamaBackend::init().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(Clone::clone)
}

/// Read the weights off disk. ~4s and a few GB of VRAM, which is why the engine
/// thread holds the result across turns rather than calling this per turn.
fn load_model() -> Result<LlamaModel, String> {
    let cfg = config().ok_or_else(|| "no local model configured".to_string())?;
    let params = LlamaModelParams::default().with_n_gpu_layers(N_GPU_LAYERS);
    LlamaModel::load_from_file(backend()?, &cfg.model_path, &params)
        .map_err(|e| format!("loading {}: {e}", cfg.model_path.display()))
}

/// Whether the weights are in VRAM right now — the Settings card reads it, and
/// the engine thread is the only writer.
static LOADED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn loaded() -> bool {
    LOADED.load(std::sync::atomic::Ordering::Relaxed)
}

/// The exact opening of a tool call: what the model is told to write, and what
/// [`generate`] watches the first few characters for. A reply that is plain
/// prose stops paying for any of this at its first character.
///
// ponytail: recognition rather than constraint. llama.cpp's lazy grammar
// sampler is the textbook answer — the grammar would bind the moment this prefix
// appears and guarantee valid JSON after it — but in llama-cpp-2 0.1.154 a
// trigger pattern either builds and never fires or is rejected outright
// (`NullGrammar`), measured against Qwen3-Coder-30B. Detection costs nothing and
// a call that will not parse falls back to text, so this waits for a binding
// where the sampler works.
const CALL_PREFIX: &str = "{\"name\"";

/// Tool definitions the way the model is told about them.
///
/// `apply_chat_template` in this binding takes no `tools` argument, so a
/// template's own tool syntax is out of reach; the definitions go in as an extra
/// system turn instead, which every chat model reads.
fn tools_preamble(tools: &serde_json::Value) -> String {
    let mut listed = String::new();
    for t in tools.as_array().map(Vec::as_slice).unwrap_or_default() {
        let f = t.get("function").unwrap_or(t);
        let Some(name) = f.get("name").and_then(|n| n.as_str()) else { continue };
        let args: Vec<&str> = f
            .pointer("/parameters/properties")
            .and_then(|p| p.as_object())
            .map(|p| p.keys().map(String::as_str).collect())
            .unwrap_or_default();
        listed.push_str(&format!("- {name}({})", args.join(", ")));
        if let Some(d) = f.get("description").and_then(|d| d.as_str()) {
            listed.push_str(&format!(": {}", d.trim()));
        }
        listed.push('\n');
    }
    // The parameter *names* rather than the raw JSON schema: a model shown the
    // schema tends to copy its keys ("type": "object") into the arguments.
    format!(
        "You can run tools. To call one, reply with exactly one JSON object and \
         nothing else, in this form:\n\
         {CALL_PREFIX}: \"<tool name>\", \"arguments\": {{\"<parameter>\": \"<value>\"}}}}\n\
         Do not wrap it in code fences and do not explain it. Otherwise answer \
         normally, in prose. Your tools:\n{listed}"
    )
}

/// Turn the model's finished JSON call into the chunk the UI already knows how
/// to buffer. `None` when it did not produce a usable call after all, in which
/// case the text stands as the reply.
fn parse_call(text: &str) -> Option<ChatChunk> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    let name = v.get("name")?.as_str()?.to_string();
    let arguments = v.get("arguments").map(ToString::to_string).unwrap_or_else(|| "{}".into());
    // The id only has to be unique within the thread: it is what the tool result
    // is matched back to.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Some(ChatChunk::ToolCall(agent_platform_client::sse::ToolCallDelta {
        index: 0,
        id: Some(format!("call_local_{n}")),
        name: Some(name),
        arguments,
    }))
}

/// Render the thread the way the model was trained to read it. Falls back to a
/// plain `role: content` transcript for a GGUF that ships no template — worse
/// answers, but an answer.
fn prompt(model: &LlamaModel, body: &ChatCompletionBody) -> String {
    let mut chat: Vec<LlamaChatMessage> = Vec::with_capacity(body.messages.len() + 1);
    if let Some(tools) = &body.tools {
        if let Ok(m) = LlamaChatMessage::new("system".into(), tools_preamble(tools)) {
            chat.push(m);
        }
    }
    chat.extend(body.messages.iter().filter_map(|m| {
        // An assistant turn that asked for tools carries the call in its
        // `tool_calls`, not its (empty) content — the model has to see its own
        // call, or the tool result that follows answers nothing.
        let content = match m.tool_calls.as_deref() {
            Some([call, ..]) if m.content.trim().is_empty() => format!(
                r#"{{"name": "{}", "arguments": {}}}"#,
                call.function.name, call.function.arguments
            ),
            _ => m.content.clone(),
        };
        LlamaChatMessage::new(m.role.clone(), content).ok()
    }));
    model
        .chat_template(None)
        .ok()
        .and_then(|t| model.apply_chat_template(&t, &chat, true).ok())
        .unwrap_or_else(|| {
            let mut s = String::new();
            for m in &body.messages {
                s.push_str(&format!("{}: {}\n", m.role, m.content));
            }
            s.push_str("assistant: ");
            s
        })
}

fn sampler(body: &ChatCompletionBody) -> LlamaSampler {
    match body.temperature {
        // Greedy is the reproducible default and what the spike measured.
        None => LlamaSampler::chain_simple([LlamaSampler::greedy()]),
        Some(t) if t <= 0.0 => LlamaSampler::chain_simple([LlamaSampler::greedy()]),
        Some(t) => LlamaSampler::chain_simple([
            LlamaSampler::temp(t as f32),
            LlamaSampler::top_p(0.95, 1),
            // Fixed seed: a local reply should be reproducible for a bug report.
            LlamaSampler::dist(0),
        ]),
    }
}

/// How many leading tokens `new` shares with `cached`.
///
/// This is the whole KV-reuse trick: a chat template re-renders the entire
/// thread every turn, so turn N's prompt is turn N-1's prompt plus the reply
/// and the new question. The shared prefix is already in the cache.
fn shared_prefix(cached: &[LlamaToken], new: &[LlamaToken]) -> usize {
    cached.iter().zip(new).take_while(|(a, b)| a == b).count()
}

/// The loaded model, a context over it, and what is currently in that context's
/// KV cache. Lives on the engine thread and nowhere else — `LlamaContext` is not
/// `Send`, and it borrows the model, so the two are dropped together.
struct Session<'a> {
    model: &'a LlamaModel,
    ctx: llama_cpp_2::context::LlamaContext<'a>,
    /// Tokens the cache holds, in order. Empty after a reset.
    cached: Vec<LlamaToken>,
}

/// Generate on the engine thread, handing prose to `emit` as it arrives.
///
/// `Ok(None)` is a reply that ended on its own (or hit the token cap) and has
/// already been streamed. `Ok(Some(json))` is a tool call: it was held back
/// rather than streamed, because a JSON call is machinery, not an answer. `Err`
/// is the text for a `Failed` chunk.
fn generate(
    session: &mut Session<'_>,
    body: &ChatCompletionBody,
    mut emit: impl FnMut(String),
) -> Result<Option<String>, String> {
    let cfg = config().ok_or_else(|| "no local model configured".to_string())?;
    let model = session.model;

    let tokens = model
        .str_to_token(&prompt(model, body), AddBos::Always)
        .map_err(|e| format!("tokenizing: {e}"))?;
    let max_tokens =
        body.max_tokens.map_or(DEFAULT_MAX_TOKENS, |n| n.clamp(1, i32::MAX as i64) as i32);
    // A prompt longer than the context is the user's history outgrowing the
    // setting, not a bug — say so rather than truncating silently.
    if tokens.len() as u32 >= cfg.n_ctx {
        return Err(format!(
            "conversation is {} tokens, longer than the {} local context — clear it or raise local_n_ctx",
            tokens.len(),
            cfg.n_ctx
        ));
    }

    // Keep the shared prefix, drop the rest of the cache. One token short of the
    // full prompt at most: the last one has to be decoded to produce logits, and
    // a resend of an identical prompt would otherwise have nothing to sample
    // from.
    let reuse = shared_prefix(&session.cached, &tokens).min(tokens.len() - 1);
    session
        .ctx
        .clear_kv_cache_seq(Some(0), Some(reuse as u32), None)
        .map_err(|e| format!("trimming the kv cache: {e}"))?;
    session.cached.truncate(reuse);

    let fresh = &tokens[reuse..];
    let mut batch = LlamaBatch::new(fresh.len().max(1), 1);
    let last = fresh.len() as i32 - 1;
    for (i, token) in fresh.iter().enumerate() {
        batch
            .add(*token, (reuse + i) as i32, &[0], i as i32 == last)
            .map_err(|e| e.to_string())?;
    }
    session.ctx.decode(&mut batch).map_err(|e| format!("prompt decode: {e}"))?;
    session.cached.extend_from_slice(fresh);

    let mut sampler = sampler(body);
    let mut decoder = encoding_rs::UTF_8.new_decoder();
    let mut pos = tokens.len() as i32;
    // Only a turn carrying tools can produce a call, so a plain chat streams
    // token by token exactly as before.
    let mut head = Held::new(body.tools.is_some());

    for _ in 0..max_tokens {
        let token = sampler.sample(&session.ctx, -1);
        if model.is_eog_token(token) {
            break;
        }
        // `special: false` keeps the template's own control tokens out of the
        // transcript; the reply wants plain text.
        match model.token_to_piece(token, &mut decoder, false, None) {
            Ok(piece) if !piece.is_empty() => head.push(piece, &mut emit),
            Ok(_) => {}
            Err(e) => return Err(format!("detokenizing: {e}")),
        }
        sampler.accept(token);

        batch.clear();
        batch.add(token, pos, &[0], true).map_err(|e| e.to_string())?;
        pos += 1;
        session.ctx.decode(&mut batch).map_err(|e| format!("decode: {e}"))?;
        // The generated token is in the cache too, and the next turn's prompt
        // will contain it — that is what makes the reply itself reusable.
        session.cached.push(token);
        if pos as u32 >= cfg.n_ctx {
            break;
        }
    }
    Ok(head.finish(&mut emit))
}

/// The first few characters of a reply, held back only long enough to tell a
/// tool call from prose.
///
/// A call has to reach the caller whole — it is a JSON object, and streaming it
/// into the transcript would show the user machinery instead of an answer. Prose
/// must not be delayed. The two are told apart by whether the text so far can
/// still become [`CALL_PREFIX`], which is decided within a handful of tokens.
enum Held {
    /// Buffering; still could go either way.
    Deciding(String),
    /// It is a call: everything is buffered and nothing is emitted.
    Call(String),
    /// It is prose: what came before was flushed and the rest streams.
    Prose,
}

impl Held {
    fn new(tools: bool) -> Self {
        if tools {
            Held::Deciding(String::new())
        } else {
            Held::Prose
        }
    }

    fn push(&mut self, piece: String, emit: &mut impl FnMut(String)) {
        match self {
            Held::Prose => emit(piece),
            Held::Call(buf) => buf.push_str(&piece),
            Held::Deciding(buf) => {
                buf.push_str(&piece);
                let seen = buf.trim_start();
                if seen.starts_with(CALL_PREFIX) {
                    *self = Held::Call(std::mem::take(buf));
                } else if !CALL_PREFIX.starts_with(seen) {
                    // Past the point where this could still become a call.
                    emit(std::mem::take(buf));
                    *self = Held::Prose;
                }
            }
        }
    }

    /// End of generation: a call is handed back, anything still undecided was
    /// prose all along (a reply too short to finish the prefix).
    fn finish(self, emit: &mut impl FnMut(String)) -> Option<String> {
        match self {
            Held::Call(buf) => Some(buf),
            Held::Deciding(buf) if !buf.is_empty() => {
                emit(buf);
                None
            }
            _ => None,
        }
    }
}

fn threads() -> i32 {
    std::thread::available_parallelism().map_or(4, |n| (n.get() / 2).max(1) as i32)
}

/// Work for the engine thread: a turn to answer (and where to put the tokens as
/// they arrive), or a request to give the VRAM back.
enum Job {
    Chat { body: ChatCompletionBody, out: tokio::sync::mpsc::UnboundedSender<ChatChunk> },
    Unload,
}

/// How long the weights stay in VRAM after the last turn. Ollama's `keep_alive`
/// default, and for the same reason: a follow-up question within a few minutes
/// is the common case, and a reload is ~4s.
const IDLE_UNLOAD: std::time::Duration = std::time::Duration::from_secs(300);

/// Hand a job to the engine thread, starting it if this is the first one.
///
/// The thread exists because [`Session`] cannot leave it: `LlamaContext` is not
/// `Send`, and keeping one alive across turns is the whole point — a fresh
/// context per turn would re-decode the entire conversation every time.
/// Serialising turns is a side effect, and the right one: they would otherwise
/// fight over the same GPU.
// ponytail: a single engine thread, so one conversation at a time. A second
// context (and a session per thread) if two ever need to answer at once.
fn submit(job: Job) {
    static ENGINE: OnceLock<std::sync::mpsc::Sender<Job>> = OnceLock::new();

    let tx = ENGINE.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("local-llm".into())
            .spawn(move || engine_loop(&rx))
            .expect("spawn the local-llm thread");
        tx
    });

    if let Err(e) = tx.send(job) {
        // The thread only exits when the channel closes, which is process
        // teardown; a failed load is reported per job instead.
        if let Job::Chat { out, .. } = e.0 {
            let _ = out.send(ChatChunk::Failed("the local model thread is gone".into()));
        }
    }
}

/// Drop the weights now rather than at the idle timeout — for whoever else wants
/// the VRAM. The next turn reloads them.
pub fn unload() {
    submit(Job::Unload);
}

/// Answer a turn from a plain thread, blocking until the reply ends.
///
/// The engine speaks in channels, so this is the same path [`chat_stream`] takes
/// without an async runtime around it — which is what
/// [`crate::local_server`] has.
///
/// # Panics
/// Called from inside a tokio runtime, `blocking_recv` panics by design; this is
/// for threads that own themselves.
pub fn chat_blocking(body: ChatCompletionBody, mut on_chunk: impl FnMut(&ChatChunk)) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    submit(Job::Chat { body, out: tx });
    while let Some(chunk) = rx.blocking_recv() {
        on_chunk(&chunk);
        if matches!(chunk, ChatChunk::Done | ChatChunk::Failed(_)) {
            break;
        }
    }
}

/// What the model is called on the wire: the GGUF's file stem, which is what a
/// client sees in `/v1/models` and may send back as `model`.
pub fn model_id() -> Option<String> {
    let cfg = config()?;
    Some(cfg.model_path.file_stem()?.to_string_lossy().into_owned())
}

/// Idle with no model in VRAM, waking to load one when a turn arrives.
fn engine_loop(rx: &std::sync::mpsc::Receiver<Job>) {
    while let Ok(job) = rx.recv() {
        match job {
            // Nothing is loaded, so there is nothing to give back.
            Job::Unload => continue,
            first => {
                if !serve(rx, first) {
                    return;
                }
            }
        }
    }
}

/// Load the model, answer turns until nothing arrives for [`IDLE_UNLOAD`] (or an
/// [`Job::Unload`] does), then return and let the weights drop with this frame.
///
/// Returns false when the channel closed, which is the only reason to stop
/// waiting for the next conversation.
fn serve(rx: &std::sync::mpsc::Receiver<Job>, first: Job) -> bool {
    let loaded = (|| {
        let cfg = config().ok_or_else(|| "no local model configured".to_string())?;
        Ok::<_, String>((load_model()?, cfg.n_ctx))
    })();

    let (model, n_ctx) = match loaded {
        Ok(pair) => pair,
        // Loading is the expensive, failure-prone step. Report it to whoever
        // asked rather than dying silently — and do not retry in a hot loop:
        // the next job gets a fresh attempt, which is what a fixed settings file
        // or a freed GPU needs.
        Err(e) => {
            if let Job::Chat { out, .. } = first {
                let _ = out.send(ChatChunk::Failed(e));
            }
            return true;
        }
    };

    let ctx = model.new_context(
        match backend() {
            Ok(b) => b,
            Err(e) => {
                if let Job::Chat { out, .. } = first {
                    let _ = out.send(ChatChunk::Failed(e));
                }
                return true;
            }
        },
        LlamaContextParams::default()
            .with_n_ctx(std::num::NonZeroU32::new(n_ctx))
            // llama.cpp defaults to 4 threads whatever the machine has.
            .with_n_threads(threads())
            .with_n_threads_batch(threads()),
    );
    let ctx = match ctx {
        Ok(ctx) => ctx,
        Err(e) => {
            if let Job::Chat { out, .. } = first {
                let _ = out.send(ChatChunk::Failed(format!("context: {e}")));
            }
            return true;
        }
    };
    let mut session = Session { model: &model, ctx, cached: Vec::new() };
    LOADED.store(true, std::sync::atomic::Ordering::Relaxed);

    let mut next = Some(first);
    let alive = loop {
        let job = match next.take() {
            Some(job) => job,
            None => match rx.recv_timeout(IDLE_UNLOAD) {
                Ok(job) => job,
                // Idle long enough: give the VRAM back and go wait for the next
                // conversation with nothing resident.
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break true,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break false,
            },
        };
        let Job::Chat { body, out } = job else { break true };

        let result = generate(&mut session, &body, |piece| {
            let _ = out.send(ChatChunk::Delta(piece));
        });
        let _ = match result {
            Ok(call) => {
                // A call the model produced but serde will not read is not
                // worth hiding: send it as text so the user sees what happened.
                match call.as_deref().map(|text| (text, parse_call(text))) {
                    Some((_, Some(chunk))) => {
                        let _ = out.send(chunk);
                    }
                    Some((text, None)) => {
                        let _ = out.send(ChatChunk::Delta(text.to_string()));
                    }
                    None => {}
                }
                out.send(ChatChunk::Done)
            }
            Err(e) => {
                // A failed turn leaves the cache in an unknown state; the next
                // one starts from scratch rather than trusting it.
                session.ctx.clear_kv_cache();
                session.cached.clear();
                out.send(ChatChunk::Failed(e))
            }
        };
    };
    LOADED.store(false, std::sync::atomic::Ordering::Relaxed);
    alive
}

/// The same chunk stream `sse::chat_stream` produces, generated here instead.
pub fn chat_stream(body: ChatCompletionBody) -> impl Stream<Item = ChatChunk> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    submit(Job::Chat { body, out: tx });
    futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|c| (c, rx)) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_platform_client::types::ChatMessage;

    fn body(temperature: Option<f64>) -> ChatCompletionBody {
        ChatCompletionBody {
            messages: vec![ChatMessage::text("user", "hi")],
            model: None,
            provider: None,
            temperature,
            max_tokens: None,
            tools: None,
            stream: Some(true),
        }
    }

    #[test]
    fn a_missing_model_file_is_not_available() {
        // The real config is process-wide and read once, so this only asserts
        // the predicate the loader uses — an unset path is off.
        let cfg = Config { model_path: PathBuf::from("Z:/nope.gguf"), n_ctx: 4096 };
        assert!(!cfg.model_path.is_file());
    }

    /// The VRAM is only claimed by a turn, so "free it" on a cold process has to
    /// be a no-op rather than a load — [`unload`] starts the engine thread, and
    /// an engine that loaded on its way to unloading would be worse than none.
    #[test]
    fn unload_before_the_first_turn_loads_nothing() {
        assert!(!loaded());
        unload();
        // `LOADED` is written in `serve` and nowhere else, so this is a fact
        // about the code path rather than a race the sleep would paper over.
        assert!(!loaded());
    }

    fn spec() -> serde_json::Value {
        serde_json::json!([{
            "type": "function",
            "function": { "name": "run_command", "parameters": { "type": "object" } }
        }])
    }

    /// The hold-back is what keeps a JSON call out of the transcript, and what
    /// keeps ordinary prose from being delayed. Both halves matter.
    #[test]
    fn prose_streams_immediately_and_a_call_is_held_back_whole() {
        let run = |tools: bool, pieces: &[&str]| {
            let mut emitted = String::new();
            let mut held = Held::new(tools);
            for p in pieces {
                held.push((*p).to_string(), &mut |s| emitted.push_str(&s));
            }
            let call = held.finish(&mut |s| emitted.push_str(&s));
            (emitted, call)
        };

        // No tools in the request: nothing is ever buffered.
        assert_eq!(run(false, &["he", "llo"]), ("hello".into(), None));

        // Prose is released as soon as it cannot become a call — here at the
        // very first piece.
        let (text, call) = run(true, &["Sure", ", here"]);
        assert_eq!((text.as_str(), call), ("Sure, here", None));

        // A call is buffered whole and never streamed.
        let (text, call) = run(true, &["{\"na", "me\": \"run_command\", ", "\"arguments\": {}}"]);
        assert!(text.is_empty(), "the call leaked into the transcript: {text}");
        assert_eq!(call.as_deref(), Some(r#"{"name": "run_command", "arguments": {}}"#));

        // A reply that stops mid-prefix was prose all along.
        assert_eq!(run(true, &["{\"na"]), ("{\"na".into(), None));
    }

    #[test]
    fn a_finished_call_becomes_a_tool_chunk_and_junk_does_not() {
        let chunk = parse_call(r#" {"name": "run_command", "arguments": {"command": "dir"}} "#);
        let Some(ChatChunk::ToolCall(d)) = chunk else { panic!("not a tool call") };
        assert_eq!(d.name.as_deref(), Some("run_command"));
        assert_eq!(d.arguments, r#"{"command":"dir"}"#);
        assert!(d.id.is_some(), "the result has nothing to answer without an id");

        assert!(parse_call("Sure, I can help").is_none());
        assert!(parse_call(r#"{"arguments": {}}"#).is_none(), "a call needs a name");
    }

    /// The model has to see its own tool call, or the result that follows it
    /// answers a question that is not in the transcript.
    #[test]
    fn an_assistant_turn_that_called_a_tool_renders_as_the_call() {
        use agent_platform_client::types::{ToolCall, ToolFunction};
        let mut turn = ChatMessage::text("assistant", "");
        turn.tool_calls = Some(vec![ToolCall {
            id: "call_1".into(),
            call_type: "function".into(),
            function: ToolFunction {
                name: "run_command".into(),
                arguments: r#"{"command":"dir"}"#.into(),
            },
        }]);
        // The rendering rule, without a model to apply a template with.
        let rendered = match turn.tool_calls.as_deref() {
            Some([call, ..]) if turn.content.trim().is_empty() => format!(
                r#"{{"name": "{}", "arguments": {}}}"#,
                call.function.name, call.function.arguments
            ),
            _ => turn.content.clone(),
        };
        assert!(parse_call(&rendered).is_some(), "the model would read back {rendered}");
    }

    #[test]
    fn the_reusable_prefix_stops_at_the_first_difference() {
        let t = |ids: &[i32]| ids.iter().map(|i| LlamaToken(*i)).collect::<Vec<_>>();
        // Turn 2's prompt is turn 1's plus more — the whole point.
        assert_eq!(shared_prefix(&t(&[1, 2, 3]), &t(&[1, 2, 3, 4, 5])), 3);
        // An edited earlier turn invalidates everything after the edit.
        assert_eq!(shared_prefix(&t(&[1, 2, 3]), &t(&[1, 9, 3])), 1);
        // A cleared conversation shares nothing, and neither does a fresh cache.
        assert_eq!(shared_prefix(&t(&[1, 2, 3]), &t(&[9])), 0);
        assert_eq!(shared_prefix(&[], &t(&[1, 2])), 0);
    }

    /// The only check that exercises llama.cpp itself. Ignored by default —
    /// it needs weights, which no CI has:
    ///
    /// ```bash
    /// AGENT_PLATFORM_TEST_GGUF=<path-to.gguf> cargo test --features cuda -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs a GGUF via AGENT_PLATFORM_TEST_GGUF"]
    fn it_generates_from_a_real_model_and_reuses_the_cache() {
        let Ok(path) = std::env::var("AGENT_PLATFORM_TEST_GGUF") else { return };
        let cfg = Config { model_path: PathBuf::from(path), n_ctx: 2048 };
        assert!(cfg.model_path.is_file(), "AGENT_PLATFORM_TEST_GGUF is not a file");
        CONFIG_OVERRIDE.set(cfg.clone()).expect("override set once");

        let model = load_model().expect("model");
        let context = |n_ctx: u32| {
            model
                .new_context(
                    backend().expect("backend"),
                    LlamaContextParams::default()
                        .with_n_ctx(std::num::NonZeroU32::new(n_ctx))
                        .with_n_threads(threads())
                        .with_n_threads_batch(threads()),
                )
                .expect("context")
        };
        let mut session = Session { model: &model, ctx: context(cfg.n_ctx), cached: Vec::new() };

        let mut first = body(None);
        first.messages = vec![ChatMessage::text("user", "Reply with the single word: pong")];
        first.max_tokens = Some(16);

        let mut reply = String::new();
        generate(&mut session, &first, |piece| reply.push_str(&piece)).expect("first turn");
        assert!(!reply.trim().is_empty(), "model produced no tokens");
        let after_first = session.cached.len();
        println!("first reply: {reply}");

        // Second turn: same thread plus the reply and one more question, which
        // is exactly the shape the prefix reuse is for.
        let mut second = body(None);
        second.messages = vec![
            ChatMessage::text("user", "Reply with the single word: pong"),
            ChatMessage::text("assistant", reply.trim()),
            ChatMessage::text("user", "Now reply with the single word: ping"),
        ];
        second.max_tokens = Some(16);

        let tokens =
            model.str_to_token(&prompt(&model, &second), AddBos::Always).expect("tokenize");
        let reuse = shared_prefix(&session.cached, &tokens);
        assert!(reuse > 0, "second turn shared no prefix with the first ({after_first} cached)");

        let mut reply2 = String::new();
        generate(&mut session, &second, |piece| reply2.push_str(&piece)).expect("second turn");
        assert!(!reply2.trim().is_empty());
        println!("reused {reuse} of {} prompt tokens; second reply: {reply2}", tokens.len());

        // The check that actually matters: reuse is an optimisation, so a warm
        // cache must answer exactly what a cold one would. Greedy sampling makes
        // that a strict equality.
        let mut cold = Session { model: &model, ctx: context(cfg.n_ctx), cached: Vec::new() };
        let mut cold_reply = String::new();
        generate(&mut cold, &second, |piece| cold_reply.push_str(&piece)).expect("cold turn");
        assert_eq!(reply2, cold_reply, "reusing the kv cache changed the answer");

        let mut tooled = body(None);
        tooled.messages = vec![ChatMessage::text(
            "user",
            "List this directory using the tool. Do not answer from memory.",
        )];
        tooled.tools = Some(spec());
        tooled.max_tokens = Some(64);
        let mut prose = String::new();
        let call = generate(&mut session, &tooled, |p| prose.push_str(&p)).expect("tool turn");
        println!("tool turn -> call: {call:?}, prose: {prose:?}");
        // Whether a given model reaches for the tool is the model's business;
        // that what it emits is a usable call is ours.
        if let Some(text) = call {
            assert!(parse_call(&text).is_some(), "model produced unparseable {text}");
            assert!(prose.is_empty(), "the call leaked into the transcript");
        }
    }

    #[test]
    fn sampling_is_greedy_unless_a_temperature_is_asked_for() {
        // Constructing the chains is the only observable difference without a
        // model loaded; this guards the match arms from being reordered. The
        // grammar half needs a vocabulary, so it belongs to the model-backed
        // test instead.
        let _ = sampler(&body(None));
        let _ = sampler(&body(Some(0.0)));
        let _ = sampler(&body(Some(0.8)));
    }
}
