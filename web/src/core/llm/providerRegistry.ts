export const CHAT_PROVIDER_REGISTRY = {
  ollama: { id: 'ollama', label: 'Server', pathKind: 'server' },
  lm_studio: { id: 'lm_studio', label: 'LM Studio', pathKind: 'server' },
  aimlapi: { id: 'aimlapi', label: 'AIMLAPI', pathKind: 'server' },
  gemini: { id: 'gemini', label: 'Cloud', pathKind: 'cloud' },
} as const;

export type ChatCompletionBackendId = keyof typeof CHAT_PROVIDER_REGISTRY;
export type ChatProviderMeta = (typeof CHAT_PROVIDER_REGISTRY)[ChatCompletionBackendId];

export const CHAT_COMPLETION_BACKEND_IDS = Object.freeze(
  Object.keys(CHAT_PROVIDER_REGISTRY) as ChatCompletionBackendId[]
);

export function isChatCompletionBackendId(value: string): value is ChatCompletionBackendId {
  return value in CHAT_PROVIDER_REGISTRY;
}

export function parseChatCompletionBackendId(raw: string): ChatCompletionBackendId | null {
  const p = raw.trim().toLowerCase();
  return isChatCompletionBackendId(p) ? p : null;
}

export function getChatProviderMeta(id: ChatCompletionBackendId): ChatProviderMeta {
  return CHAT_PROVIDER_REGISTRY[id];
}
