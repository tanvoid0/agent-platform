import { DEFAULT_MODELS } from './constants';
import { resolveMediaBackend } from './llmFacade';
import {
  CHAT_COMPLETION_BACKEND_IDS,
  defaultChatModelsByBackend,
  getProviderModelCatalog,
} from './providerModelCatalog';
import type { ChatModelsByBackend, LLMConfig } from './types';

function defaultMediaModelFromRouting(kind: 'image' | 'music' | 'video'): string {
  const b = resolveMediaBackend(kind);
  if (b === 'disabled') return DEFAULT_MODELS[kind];
  const slice = getProviderModelCatalog(b)[kind];
  return slice?.defaultModel ?? DEFAULT_MODELS[kind];
}

export const BYOK_CONFIG_STORAGE_KEY = 'byok-config';

function readStringMapField(raw: Record<string, unknown>, key: string): string {
  const v = raw[key];
  return typeof v === 'string' && v.trim() ? v.trim() : '';
}

/** Merge saved JSON with defaults. Chat completion models live only under `chatModelsByBackend`. */
export function normalizeLlmConfig(raw: unknown): LLMConfig {
  const r = raw && typeof raw === 'object' ? (raw as Record<string, unknown>) : {};
  const chatModelsByBackend: ChatModelsByBackend = defaultChatModelsByBackend();

  const map =
    r.chatModelsByBackend && typeof r.chatModelsByBackend === 'object'
      ? (r.chatModelsByBackend as Record<string, unknown>)
      : {};

  for (const id of CHAT_COMPLETION_BACKEND_IDS) {
    const v = readStringMapField(map, id);
    if (v) chatModelsByBackend[id] = v;
  }

  return {
    // Never hydrate Gemini keys from browser storage (use VITE_GEMINI_API_KEY only).
    apiKey: '',
    baseUrl: typeof r.baseUrl === 'string' ? r.baseUrl : undefined,
    chatModelsByBackend,
    imageModel:
      typeof r.imageModel === 'string' && r.imageModel.trim()
        ? r.imageModel
        : defaultMediaModelFromRouting('image'),
    musicModel:
      typeof r.musicModel === 'string' && r.musicModel.trim()
        ? r.musicModel
        : defaultMediaModelFromRouting('music'),
    videoModel:
      typeof r.videoModel === 'string' && r.videoModel.trim()
        ? r.videoModel
        : defaultMediaModelFromRouting('video'),
  };
}

export function readLlmConfigFromStorage(): LLMConfig | null {
  try {
    const saved = localStorage.getItem(BYOK_CONFIG_STORAGE_KEY);
    if (!saved) return null;
    return normalizeLlmConfig(JSON.parse(saved));
  } catch {
    return null;
  }
}

export function persistLlmConfigToStorage(cfg: LLMConfig): void {
  const { apiKey: _omit, ...rest } = cfg;
  localStorage.setItem(BYOK_CONFIG_STORAGE_KEY, JSON.stringify(rest));
}

/** Remove any legacy cached Gemini key from localStorage (one-time hygiene per load). */
export function purgeCachedGeminiApiKeyFromStorage(): void {
  try {
    const raw = localStorage.getItem(BYOK_CONFIG_STORAGE_KEY);
    if (!raw) return;
    const o = JSON.parse(raw) as Record<string, unknown>;
    if (typeof o.apiKey === 'string' && o.apiKey.length > 0) {
      delete o.apiKey;
      localStorage.setItem(BYOK_CONFIG_STORAGE_KEY, JSON.stringify(o));
    }
  } catch {
    /* ignore */
  }
}
