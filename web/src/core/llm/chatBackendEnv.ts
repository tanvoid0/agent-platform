export function useGeminiInDev(): boolean {
  const v = import.meta.env.VITE_USE_GEMINI_IN_DEV;
  return v === '1' || v === 'true';
}

export type ChatCompletionBackendId = 'gemini' | 'ollama';

/** Which backend handles `LLMProvider.generateCompletion` / agent tool loop for this build/session. */
export function resolveChatCompletionBackend(): ChatCompletionBackendId {
  if (import.meta.env.PROD || useGeminiInDev()) return 'gemini';
  return 'ollama';
}
