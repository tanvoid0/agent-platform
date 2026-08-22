//! Re-exports the shared engine; config comes from Settings at first use.

use std::path::PathBuf;
use std::sync::Once;

use agent_platform_client::sse::ChatChunk;
use agent_platform_client::types::ChatCompletionBody;
use futures::Stream;

/// Ensure the shared engine sees desktop Settings (path + n_ctx).
pub fn ensure_configured() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let s = crate::shell::Settings::load(&crate::shell::app_dir());
        let path = PathBuf::from(s.local_model_path.trim());
        if !s.local_model_path.trim().is_empty() && path.is_file() {
            agent_platform_local_llm::configure(agent_platform_local_llm::Config {
                model_path: path,
                n_ctx: s.local_n_ctx,
            });
        }
    });
}

pub fn available() -> bool {
    ensure_configured();
    agent_platform_local_llm::available()
}

pub fn loaded() -> bool {
    ensure_configured();
    agent_platform_local_llm::loaded()
}

pub fn unload() {
    ensure_configured();
    agent_platform_local_llm::unload();
}

pub fn model_id() -> Option<String> {
    ensure_configured();
    agent_platform_local_llm::model_id()
}

pub fn chat_stream(body: ChatCompletionBody) -> impl Stream<Item = ChatChunk> {
    ensure_configured();
    agent_platform_local_llm::chat_stream(body)
}

pub fn chat_blocking(body: ChatCompletionBody, on_chunk: impl FnMut(&ChatChunk)) {
    ensure_configured();
    agent_platform_local_llm::chat_blocking(body, on_chunk);
}

/// Test helper: point the engine at a GGUF without a settings file.
///
/// Uses [`agent_platform_local_llm::configure`] — the shared crate's
/// `override_config` is `cfg(test)` on that crate only and is not visible here.
#[cfg(test)]
pub fn override_config(model_path: PathBuf, n_ctx: u32) {
    agent_platform_local_llm::configure(agent_platform_local_llm::Config {
        model_path,
        n_ctx,
    });
}
