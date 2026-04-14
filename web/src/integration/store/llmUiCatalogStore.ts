/** Server LLM UI catalog (`GET /api/v1/llm/ui-catalog`). */
import { create } from 'zustand';
import { fetchLlmUiCatalog } from '../../api/client';
import type { ChatCompletionBackendId } from '../../core/llm/chatBackendEnv';
import { parseChatCompletionBackendId } from '../../core/llm/providerRegistry';

export type LlmUiCatalogChatSlice = {
  defaultModel: string;
  options: readonly string[];
};

export type LlmUiCatalogProvider = {
  id: ChatCompletionBackendId;
  label: string;
  configured: boolean;
  reachable: boolean | null;
  chat: LlmUiCatalogChatSlice;
};

export type LlmUiCatalogSnapshot = {
  resolvedDefaults: { provider: string; model: string };
  providers: LlmUiCatalogProvider[];
  geminiMedia: {
    image: LlmUiCatalogChatSlice;
    music: LlmUiCatalogChatSlice;
    video: LlmUiCatalogChatSlice;
  };
};

export type LlmUiCatalogLoadStatus = 'idle' | 'loading' | 'ok' | 'error';

function parseProviderId(raw: string): ChatCompletionBackendId | null {
  return parseChatCompletionBackendId(raw);
}

interface LlmUiCatalogState {
  status: LlmUiCatalogLoadStatus;
  snapshot: LlmUiCatalogSnapshot | null;
  load: () => Promise<void>;
}

export const useLlmUiCatalogStore = create<LlmUiCatalogState>()((set, get) => ({
  status: 'idle',
  snapshot: null,
  load: async () => {
    if (get().status === 'loading') return;
    set({ status: 'loading' });
    try {
      const j = await fetchLlmUiCatalog();
      const providers: LlmUiCatalogProvider[] = [];
      for (const row of j.providers ?? []) {
        const id = parseProviderId(row.id);
        if (!id) continue;
        providers.push({
          id,
          label: typeof row.label === 'string' ? row.label : id,
          configured: Boolean(row.configured),
          reachable: row.reachable === null || row.reachable === undefined ? null : Boolean(row.reachable),
          chat: {
            defaultModel:
              typeof row.chat?.default_model === 'string' ? row.chat.default_model : '',
            options: Array.isArray(row.chat?.options)
              ? row.chat.options.filter((x): x is string => typeof x === 'string')
              : [],
          },
        });
      }
      const gm = j.gemini_media;
      const snap: LlmUiCatalogSnapshot = {
        resolvedDefaults: {
          provider: typeof j.resolved_defaults?.provider === 'string' ? j.resolved_defaults.provider : '',
          model: typeof j.resolved_defaults?.model === 'string' ? j.resolved_defaults.model : '',
        },
        providers,
        geminiMedia: {
          image: {
            defaultModel: gm?.image?.default_model ?? '',
            options: Array.isArray(gm?.image?.options) ? gm.image.options : [],
          },
          music: {
            defaultModel: gm?.music?.default_model ?? '',
            options: Array.isArray(gm?.music?.options) ? gm.music.options : [],
          },
          video: {
            defaultModel: gm?.video?.default_model ?? '',
            options: Array.isArray(gm?.video?.options) ? gm.video.options : [],
          },
        },
      };
      set({ status: 'ok', snapshot: snap });
    } catch {
      set({ status: 'error', snapshot: null });
    }
  },
}));
