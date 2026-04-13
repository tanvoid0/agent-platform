import { create } from 'zustand';
import { getGeminiApiKeyFromEnv } from '../../core/llm/geminiApiKeyEnv';
import {
  normalizeLlmConfig,
  purgeCachedGeminiApiKeyFromStorage,
  readLlmConfigFromStorage,
} from '../../core/llm/llmConfigStorage';
import type { LLMConfig } from '../../core/llm/types';

/**
 * LLM / API client configuration for the delegation runtime (agents, media, pricing).
 * Lives in integration — not the UI chrome store — so core logic never depends on `useUiStore`.
 */
interface LlmSessionState {
  llmConfig: LLMConfig;
  setLlmConfig: (config: Partial<LLMConfig>) => void;
}

export const useLlmSessionStore = create<LlmSessionState>()((set) => ({
  llmConfig: (() => {
    purgeCachedGeminiApiKeyFromStorage();
    const stored = readLlmConfigFromStorage();
    const base = stored ?? normalizeLlmConfig(null);
    return { ...base, apiKey: getGeminiApiKeyFromEnv() };
  })(),
  setLlmConfig: (config) =>
    set((s) => ({
      llmConfig: { ...s.llmConfig, ...config, apiKey: getGeminiApiKeyFromEnv() },
    })),
}));

export function getLlmSessionConfig(): LLMConfig {
  return useLlmSessionStore.getState().llmConfig;
}
