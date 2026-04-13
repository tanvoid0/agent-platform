import type { ChatCompletionBackendId } from './chatBackendEnv';
import { resolveChatCompletionBackend } from './chatBackendEnv';
import type { ChatModelsByBackend } from './types';
import { MODEL_CONFIG } from '../../../model-config';

/**
 * Same shape for every LLM vendor: one default id and a pick list.
 * Empty `options` means the app does not wire that modality to this backend yet.
 */
export interface ModelListSlice {
  defaultModel: string;
  options: readonly string[];
}

/**
 * Full catalog per connection class. Cloud includes chat + asset models; server path is chat-focused in-app.
 */
export interface ProviderModelCatalog {
  id: ChatCompletionBackendId;
  /** Short label for settings / debug */
  label: string;
  chat: ModelListSlice;
  image?: ModelListSlice;
  music?: ModelListSlice;
  video?: ModelListSlice;
}

/** @see https://ai.google.dev/gemini-api/docs/models */
export const GEMINI_CATALOG: ProviderModelCatalog = {
  id: 'gemini',
  label: 'Cloud',
  chat: MODEL_CONFIG.gemini.chat,
  image: MODEL_CONFIG.gemini.image,
  music: MODEL_CONFIG.gemini.music,
  video: MODEL_CONFIG.gemini.video,
};

/** Dev default: chat via Agent Platform → server-configured models. */
export const OLLAMA_CATALOG: ProviderModelCatalog = {
  id: 'ollama',
  label: 'Server',
  chat: MODEL_CONFIG.ollama.chat,
  image: MODEL_CONFIG.ollama.image,
  music: MODEL_CONFIG.ollama.music,
  video: MODEL_CONFIG.ollama.video,
};

export const PROVIDER_MODEL_CATALOGS: Record<ChatCompletionBackendId, ProviderModelCatalog> = {
  gemini: GEMINI_CATALOG,
  ollama: OLLAMA_CATALOG,
};

/** Stable iteration order for every registered chat-completion backend. */
export const CHAT_COMPLETION_BACKEND_IDS: readonly ChatCompletionBackendId[] = Object.freeze(
  Object.keys(PROVIDER_MODEL_CATALOGS) as ChatCompletionBackendId[]
);

/** Completion backend that owns cloud asset modalities in this build (image / audio / video APIs). */
export const CLOUD_MEDIA_COMPLETION_BACKEND_ID: ChatCompletionBackendId = 'gemini';

export function defaultChatModelsByBackend(): ChatModelsByBackend {
  return CHAT_COMPLETION_BACKEND_IDS.reduce((acc, id) => {
    acc[id] = PROVIDER_MODEL_CATALOGS[id].chat.defaultModel;
    return acc;
  }, {} as ChatModelsByBackend);
}

export function getProviderModelCatalog(id: ChatCompletionBackendId): ProviderModelCatalog {
  return PROVIDER_MODEL_CATALOGS[id];
}

/** Chat / completion model list for the active connection (server vs cloud). */
export function getActiveChatCompletionSlice(): ModelListSlice {
  return getProviderModelCatalog(resolveChatCompletionBackend()).chat;
}

/** Asset slice from the Gemini catalog (used when routing targets Gemini). */
export function getGeminiAssetSlice(kind: 'image' | 'music' | 'video'): ModelListSlice {
  const c = GEMINI_CATALOG;
  return c[kind];
}
