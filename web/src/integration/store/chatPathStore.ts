/**
 * Server-resolved chat path (Ollama vs LM Studio vs AIMLAPI vs Gemini) for the Flow UI.
 * Mirrors `get_resolved_proxy_defaults()` on Agent Platform so the browser does not depend on Vite flags.
 */
import { create } from 'zustand';
import type { ChatCompletionBackendId } from '../../core/llm/chatBackendEnv';
import { parseChatCompletionBackendId } from '../../core/llm/providerRegistry';
import { fetchChatResolvedDefaults } from '../../api/client';

export type ChatPathLoadStatus = 'idle' | 'loading' | 'ok' | 'error';

function parseProvider(raw: string): ChatCompletionBackendId | null {
  return parseChatCompletionBackendId(raw);
}

interface ChatPathState {
  status: ChatPathLoadStatus;
  serverProvider: ChatCompletionBackendId | null;
  serverModel: string | null;
  lastLoadedAt: number | null;
  lastError: string | null;
  load: () => Promise<void>;
}

export const useChatPathStore = create<ChatPathState>()((set, get) => ({
  status: 'idle',
  serverProvider: null,
  serverModel: null,
  lastLoadedAt: null,
  lastError: null,
  load: async () => {
    if (get().status === 'loading') return;
    set({ status: 'loading', lastError: null });
    try {
      const d = await fetchChatResolvedDefaults();
      const p = parseProvider(d.provider);
      if (!p) {
        set({
          status: 'error',
          serverProvider: null,
          serverModel: null,
          lastError: `Invalid provider value: ${d.provider || '(empty)'}`,
        });
        return;
      }
      set({
        status: 'ok',
        serverProvider: p,
        serverModel: d.model?.trim() ? d.model.trim() : null,
        lastLoadedAt: Date.now(),
        lastError: null,
      });
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      set({
        status: 'error',
        serverProvider: null,
        serverModel: null,
        lastError: message || 'Failed to load resolved chat path',
      });
    }
  },
}));
