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
   * - `image`/`music`/`video`: only Gemini implements generation here; use `gemini`, `follow-chat` (inherits active chat — useful when chat is already cloud), or `disabled`.
   */
  routing: {
    chat: 'auto', // 'auto' | 'gemini' | 'ollama' | 'lm_studio' | 'aimlapi'
    /**
     * Image / music / video: only Gemini exposes those APIs in this app.
     * `follow-chat` uses the active chat backend — Ollama and LM Studio are chat-only (no media APIs), so those paths stay not-ready until you route to `gemini` or `disabled`.
     */
    image: 'follow-chat', // 'follow-chat' | 'gemini' | 'disabled'
    music: 'follow-chat', // 'follow-chat' | 'gemini' | 'disabled'
    video: 'follow-chat', // 'follow-chat' | 'gemini' | 'disabled'
  },
  ollama: {
    /**
     * Default models when chat is routed to the server path (`ChatCompletionBackendId` `ollama` — embedded LLM proxy).
     * Vision-in-chat works if the proxy serves a vision-capable model alias.
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
  },
  lm_studio: {
    /**
     * Defaults when chat is routed to LM Studio (server-resolved or `model-config` override). Requests go to
     * Agent Platform → embedded `/v1` proxy; ids should match LM Studio / proxy `config.yaml`.
     */
    chat: {
      defaultModel: 'google/gemma-4-e4b',
      options: ['google/gemma-4-e4b'],
    },
  },
  aimlapi: {
    /**
     * Defaults when chat is routed to AIMLAPI via the embedded server proxy.
     */
    chat: {
      defaultModel: 'openai/gpt-4.1-mini',
      options: [
        'openai/gpt-4.1-mini',
        'openai/gpt-4.1',
        'anthropic/claude-3.7-sonnet',
      ],
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

