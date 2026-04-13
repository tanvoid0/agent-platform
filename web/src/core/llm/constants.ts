import { GEMINI_CATALOG } from './providerModelCatalog';

/** Gemini defaults for persisted teams, asset generation, and production chat. */
export const DEFAULT_MODELS = {
  text: GEMINI_CATALOG.chat.defaultModel,
  image: GEMINI_CATALOG.image.defaultModel,
  music: GEMINI_CATALOG.music.defaultModel,
  video: GEMINI_CATALOG.video.defaultModel,
} as const;

export const AVAILABLE_MODELS = {
  text: [...GEMINI_CATALOG.chat.options],
  image: [...GEMINI_CATALOG.image.options],
  music: [...GEMINI_CATALOG.music.options],
  video: [...GEMINI_CATALOG.video.options],
} as const;

export type ModelType = keyof typeof AVAILABLE_MODELS;
