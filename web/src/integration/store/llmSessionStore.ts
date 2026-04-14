import { create } from 'zustand';
import {
  normalizeLlmConfig,
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
    const stored = readLlmConfigFromStorage();
    return stored ?? normalizeLlmConfig(null);
  })(),
  setLlmConfig: (config) =>
    set((s) => ({
      llmConfig: { ...s.llmConfig, ...config, apiKey: '' },
    })),
}));

export function getLlmSessionConfig(): LLMConfig {
  return useLlmSessionStore.getState().llmConfig;
}
