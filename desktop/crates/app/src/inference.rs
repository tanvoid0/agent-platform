//! Where a chat turn gets answered: in this process, or by the server.
//!
//! Every screen calls [`chat_stream`] instead of `sse::chat_stream` directly, so
//! the choice lives in one place. Without the `local-llm` feature there is no
//! choice to make and this is a passthrough.

use agent_platform_client::sse::{self, ChatChunk};
use agent_platform_client::types::ChatCompletionBody;
use agent_platform_client::Client;
use futures::Stream;

/// The server answers unless in-process inference is built in, configured, and
/// able to serve *this* request.
///
/// Three things send a turn back to the server even with a local model loaded:
/// tools (the local path does not do tool calls), an explicit provider, and an
/// explicit model — the last two are the user naming an upstream, which is an
/// answer to "who should handle this".
pub fn chat_stream(client: Client, body: ChatCompletionBody) -> impl Stream<Item = ChatChunk> {
    #[cfg(feature = "local-llm")]
    if body.tools.is_none()
        && body.provider.is_none()
        && body.model.is_none()
        && crate::local_llm::available()
    {
        return futures::future::Either::Left(crate::local_llm::chat_stream(body));
    }

    #[cfg(feature = "local-llm")]
    return futures::future::Either::Right(sse::chat_stream(client, body));

    #[cfg(not(feature = "local-llm"))]
    sse::chat_stream(client, body)
}
