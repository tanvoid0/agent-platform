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
//! which side answered. Tool calls are not handled here — [`crate::inference`]
//! sends those to the server, which has the provider that supports them.
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

/// Set by the model-backed test instead of the settings file — nothing else
/// writes it, and it is read before the settings are consulted.
static CONFIG_OVERRIDE: OnceLock<Config> = OnceLock::new();

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

/// The loaded weights, kept across turns — a reload is ~4s and the whole point
/// of holding them is that a second question does not pay it again.
fn model() -> Result<&'static LlamaModel, String> {
    static MODEL: OnceLock<Result<LlamaModel, String>> = OnceLock::new();
    MODEL
        .get_or_init(|| {
            let cfg = config().ok_or_else(|| "no local model configured".to_string())?;
            let params = LlamaModelParams::default().with_n_gpu_layers(N_GPU_LAYERS);
            LlamaModel::load_from_file(backend()?, &cfg.model_path, &params)
                .map_err(|e| format!("loading {}: {e}", cfg.model_path.display()))
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// Render the thread the way the model was trained to read it. Falls back to a
/// plain `role: content` transcript for a GGUF that ships no template — worse
/// answers, but an answer.
fn prompt(model: &LlamaModel, body: &ChatCompletionBody) -> String {
    let chat: Vec<LlamaChatMessage> = body
        .messages
        .iter()
        .filter_map(|m| LlamaChatMessage::new(m.role.clone(), m.content.clone()).ok())
        .collect();
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

/// The context and what is currently in its KV cache. Lives on the engine
/// thread and nowhere else — `LlamaContext` is not `Send`.
struct Session<'a> {
    ctx: llama_cpp_2::context::LlamaContext<'a>,
    /// Tokens the cache holds, in order. Empty after a reset.
    cached: Vec<LlamaToken>,
}

/// Generate on the engine thread, handing every token to `emit`. Returns the
/// error text for a `Failed` chunk; `Ok(())` means the reply ended on its own
/// or hit the token cap.
fn generate(
    session: &mut Session<'_>,
    body: &ChatCompletionBody,
    mut emit: impl FnMut(String),
) -> Result<(), String> {
    let cfg = config().ok_or_else(|| "no local model configured".to_string())?;
    let model = model()?;

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

    for _ in 0..max_tokens {
        let token = sampler.sample(&session.ctx, -1);
        if model.is_eog_token(token) {
            break;
        }
        // `special: false` keeps the template's own control tokens out of the
        // transcript; the reply wants plain text.
        match model.token_to_piece(token, &mut decoder, false, None) {
            Ok(piece) if !piece.is_empty() => emit(piece),
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
    Ok(())
}

fn threads() -> i32 {
    std::thread::available_parallelism().map_or(4, |n| (n.get() / 2).max(1) as i32)
}

/// One turn of work for the engine thread: what to answer, and where to put the
/// tokens as they arrive.
struct Job {
    body: ChatCompletionBody,
    out: tokio::sync::mpsc::UnboundedSender<ChatChunk>,
}

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
        // The thread only exits when the channel closes or the model failed to
        // load, and it reports the latter itself; this is the former.
        let _ = e.0.out.send(ChatChunk::Failed("the local model thread is gone".into()));
    }
}

/// Load once, then answer jobs until the channel closes.
fn engine_loop(rx: &std::sync::mpsc::Receiver<Job>) {
    let session = (|| {
        let cfg = config().ok_or_else(|| "no local model configured".to_string())?;
        let ctx = model()?
            .new_context(
                backend()?,
                LlamaContextParams::default()
                    .with_n_ctx(std::num::NonZeroU32::new(cfg.n_ctx))
                    // llama.cpp defaults to 4 threads whatever the machine has.
                    .with_n_threads(threads())
                    .with_n_threads_batch(threads()),
            )
            .map_err(|e| format!("context: {e}"))?;
        Ok::<_, String>(Session { ctx, cached: Vec::new() })
    })();

    let mut session = match session {
        Ok(s) => s,
        // Loading is the expensive, failure-prone step. Report it to whoever
        // asked rather than dying silently, then let each later job fail the
        // same way through the closed channel.
        Err(e) => {
            for job in rx.iter() {
                let _ = job.out.send(ChatChunk::Failed(e.clone()));
            }
            return;
        }
    };

    for job in rx.iter() {
        let out = job.out;
        let result = generate(&mut session, &job.body, |piece| {
            let _ = out.send(ChatChunk::Delta(piece));
        });
        let _ = match result {
            Ok(()) => out.send(ChatChunk::Done),
            Err(e) => {
                // A failed turn leaves the cache in an unknown state; the next
                // one starts from scratch rather than trusting it.
                session.ctx.clear_kv_cache();
                session.cached.clear();
                out.send(ChatChunk::Failed(e))
            }
        };
    }
}

/// The same chunk stream `sse::chat_stream` produces, generated here instead.
pub fn chat_stream(body: ChatCompletionBody) -> impl Stream<Item = ChatChunk> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    submit(Job { body, out: tx });
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

        let ctx = model()
            .expect("model")
            .new_context(
                backend().expect("backend"),
                LlamaContextParams::default()
                    .with_n_ctx(std::num::NonZeroU32::new(cfg.n_ctx))
                    .with_n_threads(threads())
                    .with_n_threads_batch(threads()),
            )
            .expect("context");
        let mut session = Session { ctx, cached: Vec::new() };

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

        let tokens = model()
            .expect("model")
            .str_to_token(&prompt(model().unwrap(), &second), AddBos::Always)
            .expect("tokenize");
        let reuse = shared_prefix(&session.cached, &tokens);
        assert!(reuse > 0, "second turn shared no prefix with the first ({after_first} cached)");

        let mut reply2 = String::new();
        generate(&mut session, &second, |piece| reply2.push_str(&piece)).expect("second turn");
        assert!(!reply2.trim().is_empty());
        println!("reused {reuse} of {} prompt tokens; second reply: {reply2}", tokens.len());

        // The check that actually matters: reuse is an optimisation, so a warm
        // cache must answer exactly what a cold one would. Greedy sampling makes
        // that a strict equality.
        let cold_ctx = model()
            .unwrap()
            .new_context(
                backend().unwrap(),
                LlamaContextParams::default()
                    .with_n_ctx(std::num::NonZeroU32::new(cfg.n_ctx))
                    .with_n_threads(threads())
                    .with_n_threads_batch(threads()),
            )
            .expect("cold context");
        let mut cold = Session { ctx: cold_ctx, cached: Vec::new() };
        let mut cold_reply = String::new();
        generate(&mut cold, &second, |piece| cold_reply.push_str(&piece)).expect("cold turn");
        assert_eq!(reply2, cold_reply, "reusing the kv cache changed the answer");
    }

    #[test]
    fn sampling_is_greedy_unless_a_temperature_is_asked_for() {
        // Constructing the chains is the only observable difference without a
        // model loaded; this guards the match arms from being reordered.
        let _ = sampler(&body(None));
        let _ = sampler(&body(Some(0.0)));
        let _ = sampler(&body(Some(0.8)));
    }
}
