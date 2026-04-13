/** User-visible label for the chat HTTP entrypoint (Agent Platform proxies to the configured LLM stack). */

export function getChatCompletionEndpointLabel(): string {
  return 'POST /api/v1/chat';
}
