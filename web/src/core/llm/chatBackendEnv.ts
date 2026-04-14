import { useChatPathStore } from '../../integration/store/chatPathStore';
import type { ChatCompletionBackendId } from './providerRegistry';
export type { ChatCompletionBackendId } from './providerRegistry';

/** Which backend handles `LLMProvider.generateCompletion` / agent tool loop for this session. */
export function resolveChatCompletionBackend(): ChatCompletionBackendId {
  const s = useChatPathStore.getState();
  if (s.status === 'ok' && s.serverProvider) {
    return s.serverProvider;
  }
  // Deterministic bootstrap backend until server defaults are loaded.
  return 'lm_studio';
}
