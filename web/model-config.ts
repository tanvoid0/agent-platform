/**
 * Central model configuration.
 *
 * Edit this file to swap defaults/options without touching app internals.
 * These values are consumed by both provider catalogs and generation defaults.
 */
export const MODEL_CONFIG = {
  /**
   * Backend routing by modality.
   * - `chat`: `auto` keeps env behavior (dev: server chat via Agent Platform, prod: cloud).
   * - `image`/`music`/`video`: pick which backend should handle final deliverables.
   *   Use `disabled` to intentionally block only that modality.
   */
  routing: {
    chat: 'auto', // 'auto' | 'gemini' | 'ollama'
    /** Same backend as resolved chat (`auto` → dev server path / prod cloud). Avoids mixing server chat with paid cloud media. */
    image: 'follow-chat', // 'follow-chat' | 'gemini' | 'ollama' | 'disabled'
    music: 'follow-chat',
    video: 'follow-chat',
  },
  ollama: {
    /**
     * Default models when chat is routed to the server path (`ChatCompletionBackendId` `ollama` — Agent Platform → orchestrator).
     * Vision-in-chat works if the orchestrator serves a vision-capable model alias.
     */
    chat: {
      // Good default for high-end local hardware.
      defaultModel: 'gemma4:latest',
      options: [
        'gemma4:latest',
        // Optional second vision model. Installed but unstable on this host in quick tests.
        'qwen2.5vl:7b',
      ],
    },
    /**
     * Media model lists are populated so config stays shape-compatible.
     * Runtime media on the server path is still gated by integration status.
     */
    image: {
      defaultModel: 'gemma4:latest',
      options: ['gemma4:latest', 'qwen2.5vl:7b'],
    },
    music: {
      defaultModel: 'gemma4:latest',
      options: ['gemma4:latest'],
    },
    video: {
      defaultModel: 'gemma4:latest',
      options: ['gemma4:latest'],
    },
  },
  gemini: {
    chat: {
      defaultModel: 'gemini-3-flash-preview',
      options: [
        'gemini-3-flash-preview',
        'gemini-3.1-pro-preview',
        'gemini-3.1-flash-lite-preview',
      ],
    },
    image: {
      defaultModel: 'gemini-3.1-flash-image-preview',
      options: [
        'gemini-3.1-flash-image-preview',
        'gemini-3-pro-image-preview',
        'gemini-2.5-flash-image',
      ],
    },
    music: {
      defaultModel: 'lyria-3-clip-preview',
      options: ['lyria-3-clip-preview', 'lyria-3-pro-preview'],
    },
    video: {
      defaultModel: 'veo-3.1-lite-generate-preview',
      options: [
        'veo-3.1-lite-generate-preview',
        'veo-3.1-fast-generate-preview',
        'veo-3.1-generate-preview',
      ],
    },
  },
} as const;

