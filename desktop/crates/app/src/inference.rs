//! Where a chat turn gets answered: in this process, or by the server.
//!
//! Every screen calls [`chat_stream`] instead of `sse::chat_stream` directly, so
//! the choice lives in one place. Without the `local-llm` feature there is no
//! choice to make and this is a passthrough.

#[cfg(feature = "local-llm")]
use std::sync::atomic::{AtomicU8, Ordering};

use agent_platform_client::sse::{self, ChatChunk};
use agent_platform_client::types::ChatCompletionBody;
use agent_platform_client::Client;
use futures::Stream;

/// The server answers unless in-process inference is built in, configured, and
/// able to serve *this* request.
///
/// Two things send a turn back to the server even with a local model loaded: an
/// explicit provider and an explicit model. Both are the user naming an
/// upstream, which is an answer to "who should handle this". Tools are handled
/// locally — [`crate::local_llm`] recognises a call in the reply and holds it
/// back from the stream.
pub fn chat_stream(client: Client, body: ChatCompletionBody) -> impl Stream<Item = ChatChunk> {
    #[cfg(feature = "local-llm")]
    if body.provider.is_none() && body.model.is_none() && crate::local_llm::available() {
        LAST_LOCAL.store(1, Ordering::Relaxed);
        return futures::future::Either::Left(crate::local_llm::chat_stream(body));
    }

    #[cfg(feature = "local-llm")]
    {
        LAST_LOCAL.store(2, Ordering::Relaxed);
        return futures::future::Either::Right(sse::chat_stream(client, body));
    }

    #[cfg(not(feature = "local-llm"))]
    sse::chat_stream(client, body)
}

/// What the chat header calls the in-process engine. Not a provider the proxy
/// knows about — picking it means *unsetting* both fields, which is the only
/// state [`chat_stream`] routes here. It is listed anyway because "leave both
/// boxes empty" is not a thing anyone can see, and a local engine nobody can
/// select is a local engine nobody uses.
pub const LOCAL_ID: &str = "local";

/// Whether [`LOCAL_ID`] is worth offering: the feature is in, a GGUF is
/// configured, and the file is there. Same condition [`chat_stream`] routes on,
/// deliberately — a listed choice that silently answers elsewhere is worse than
/// no choice at all.
pub fn local_available() -> bool {
    #[cfg(feature = "local-llm")]
    {
        crate::local_llm::available()
    }
    #[cfg(not(feature = "local-llm"))]
    false
}

/// Which side answered last, for the Settings badge: `0` nothing yet, `1` here,
/// `2` the server. Written where the choice is made, which is above.
#[cfg(feature = "local-llm")]
static LAST_LOCAL: AtomicU8 = AtomicU8::new(0);

/// `None` until a turn has been routed. A badge is the only reader, so a relaxed
/// load is enough — it is allowed to be one frame stale.
#[cfg(feature = "local-llm")]
pub fn last_turn_was_local() -> Option<bool> {
    match LAST_LOCAL.load(Ordering::Relaxed) {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}
