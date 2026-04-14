import type { ChatCompletionBackendId } from './chatBackendEnv';
import { resolveChatCompletionBackend } from './chatBackendEnv';
import { CHAT_COMPLETION_BACKEND_IDS, getChatProviderMeta } from './providerRegistry';
import type { ChatModelsByBackend } from './types';
import { useLlmUiCatalogStore } from '../../integration/store/llmUiCatalogStore';
import { MODEL_CONFIG } from '../../../model-config';

export { CHAT_COMPLETION_BACKEND_IDS };

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
  label: getChatProviderMeta('gemini').label,
  chat: MODEL_CONFIG.gemini.chat,
  image: MODEL_CONFIG.gemini.image,
  music: MODEL_CONFIG.gemini.music,
  video: MODEL_CONFIG.gemini.video,
};

/** Dev default: chat via Agent Platform → server-configured models. */
export const OLLAMA_CATALOG: ProviderModelCatalog = {
  id: 'ollama',
  label: getChatProviderMeta('ollama').label,
  chat: MODEL_CONFIG.ollama.chat,
};

export const LM_STUDIO_CATALOG: ProviderModelCatalog = {
  id: 'lm_studio',
  label: getChatProviderMeta('lm_studio').label,
  chat: MODEL_CONFIG.lm_studio.chat,
};

export const AIMLAPI_CATALOG: ProviderModelCatalog = {
  id: 'aimlapi',
  label: getChatProviderMeta('aimlapi').label,
  chat: MODEL_CONFIG.aimlapi.chat,
};

/** Static fallback when `GET /api/v1/llm/ui-catalog` has not loaded or failed. */
export const PROVIDER_MODEL_CATALOGS: Record<ChatCompletionBackendId, ProviderModelCatalog> = {
  gemini: GEMINI_CATALOG,
  ollama: OLLAMA_CATALOG,
  lm_studio: LM_STUDIO_CATALOG,
  aimlapi: AIMLAPI_CATALOG,
};

/**
 * Union server-reported ids with `model-config.ts` so pickers stay populated when the
 * catalog is empty or a provider is not wired yet; users can still type any id — chat
 * may error at request time if the id is invalid upstream.
 */
export function mergeModelListSlices(fallback: ModelListSlice, server: ModelListSlice): ModelListSlice {
  const dm =
    (server.defaultModel && server.defaultModel.trim()) || (fallback.defaultModel && fallback.defaultModel.trim()) || '';
  const seen = new Set<string>();
  const options: string[] = [];
  const push = (raw: string) => {
    const t = raw.trim();
    if (!t || seen.has(t)) return;
    seen.add(t);
    options.push(t);
  };
  push(dm);
  for (const x of server.options) push(x);
  for (const x of fallback.options) push(x);
  const finalDm = dm || (fallback.defaultModel && fallback.defaultModel.trim()) || options[0] || '';
  if (options.length === 0 && finalDm) {
    options.push(finalDm);
  }
  return { defaultModel: finalDm, options };
}

function providerCatalogFromServer(id: ChatCompletionBackendId): ProviderModelCatalog | null {
  const { status, snapshot } = useLlmUiCatalogStore.getState();
  if (status !== 'ok' || !snapshot) return null;
  const p = snapshot.providers.find((x) => x.id === id);
  if (!p) return null;
  const chatServer: ModelListSlice = {
    defaultModel: p.chat.defaultModel,
    options:
      p.chat.options.length > 0 ? p.chat.options : p.chat.defaultModel ? [p.chat.defaultModel] : [],
  };
  const chat = mergeModelListSlices(PROVIDER_MODEL_CATALOGS[id].chat, chatServer);
  if (id === 'gemini') {
    const gm = snapshot.geminiMedia;
    return {
      id: 'gemini',
      label: p.label,
      chat,
      image: mergeModelListSlices(GEMINI_CATALOG.image!, {
        defaultModel: gm.image.defaultModel,
        options: gm.image.options.length > 0 ? gm.image.options : gm.image.defaultModel ? [gm.image.defaultModel] : [],
      }),
      music: mergeModelListSlices(GEMINI_CATALOG.music!, {
        defaultModel: gm.music.defaultModel,
        options: gm.music.options.length > 0 ? gm.music.options : gm.music.defaultModel ? [gm.music.defaultModel] : [],
      }),
      video: mergeModelListSlices(GEMINI_CATALOG.video!, {
        defaultModel: gm.video.defaultModel,
        options: gm.video.options.length > 0 ? gm.video.options : gm.video.defaultModel ? [gm.video.defaultModel] : [],
      }),
    };
  }
  return { id, label: p.label, chat };
}

/** Completion backend that owns cloud asset modalities in this build (image / audio / video APIs). */
export const CLOUD_MEDIA_COMPLETION_BACKEND_ID: ChatCompletionBackendId = 'gemini';

export function defaultChatModelsByBackend(): ChatModelsByBackend {
  return CHAT_COMPLETION_BACKEND_IDS.reduce((acc, id) => {
    acc[id] = getProviderModelCatalog(id).chat.defaultModel;
    return acc;
  }, {} as ChatModelsByBackend);
}

/** Prefer server catalog from Agent Platform; fall back to `model-config.ts`. */
export function getProviderModelCatalog(id: ChatCompletionBackendId): ProviderModelCatalog {
  return providerCatalogFromServer(id) ?? PROVIDER_MODEL_CATALOGS[id];
}

/** Effective chat backend: explicit `model-config` routing, or env-driven dev defaults. */
export function getActiveChatBackendId(): ChatCompletionBackendId {
  const configured = MODEL_CONFIG.routing.chat as
    | 'auto'
    | 'gemini'
    | 'ollama'
    | 'lm_studio'
    | 'aimlapi';
  if (
    configured === 'gemini' ||
    configured === 'ollama' ||
    configured === 'lm_studio' ||
    configured === 'aimlapi'
  ) {
    return configured;
  }
  return resolveChatCompletionBackend();
}

/** Chat / completion model list for the active connection (server vs cloud). */
export function getActiveChatCompletionSlice(): ModelListSlice {
  return getProviderModelCatalog(getActiveChatBackendId()).chat;
}

/** Asset slice from the Gemini catalog (used when routing targets Gemini). */
export function getGeminiAssetSlice(kind: 'image' | 'music' | 'video'): ModelListSlice {
  const fallback = GEMINI_CATALOG[kind];
  const { status, snapshot } = useLlmUiCatalogStore.getState();
  if (status === 'ok' && snapshot) {
    const s = snapshot.geminiMedia[kind];
    const server: ModelListSlice = {
      defaultModel: s.defaultModel,
      options: s.options.length > 0 ? s.options : s.defaultModel ? [s.defaultModel] : [],
    };
    return mergeModelListSlices(fallback, server);
  }
  return fallback;
}
