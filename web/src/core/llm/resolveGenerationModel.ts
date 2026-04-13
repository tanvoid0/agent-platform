import { resolveChatModelForSession, resolveMediaBackend } from './llmFacade';
import {
  CLOUD_MEDIA_COMPLETION_BACKEND_ID,
  getProviderModelCatalog,
} from './providerModelCatalog';
import type { LLMConfig } from './types';
import type { ChatCompletionBackendId } from './chatBackendEnv';

/** Minimal team fields needed to resolve the same model id `AgentBrain` uses for final output. */
export type GenerationModelTeamPick = {
  outputType: 'text' | 'image' | 'music' | 'video';
  outputModel: string;
  /** Text deliverables use the active chat stack (server vs cloud); lead preset participates in resolution. */
  leadAgent?: { model: string };
};

function geminiChatModelId(llmConfig: LLMConfig): string {
  const fromMap = llmConfig.chatModelsByBackend[CLOUD_MEDIA_COMPLETION_BACKEND_ID]?.trim();
  if (fromMap) return fromMap;
  return getProviderModelCatalog(CLOUD_MEDIA_COMPLETION_BACKEND_ID).chat.defaultModel;
}

function mediaCatalogSlice(
  backend: ChatCompletionBackendId,
  kind: 'image' | 'music' | 'video'
) {
  return getProviderModelCatalog(backend)[kind];
}

function resolveMediaGenerationModel(
  llmConfig: LLMConfig,
  kind: 'image' | 'music' | 'video',
  teamOutputModel: string
): string {
  const backend = resolveMediaBackend(kind);
  if (backend === 'disabled') {
    return teamOutputModel.trim() || '';
  }
  const slice = mediaCatalogSlice(backend, kind);
  const options = slice?.options ?? [];
  const fallback = slice?.defaultModel ?? '';
  const settingsKey = kind === 'image' ? 'imageModel' : kind === 'music' ? 'musicModel' : 'videoModel';
  const fromSettings = (llmConfig[settingsKey] ?? '').trim();
  const explicit = teamOutputModel.trim();

  const pickFirstValid = (id: string) => (id && options.includes(id) ? id : '');

  if (pickFirstValid(explicit)) return explicit;
  if (pickFirstValid(fromSettings)) return fromSettings;
  if (backend === 'gemini') {
    return fallback || geminiChatModelId(llmConfig);
  }
  return fallback;
}

/**
 * Effective cloud / generation model for the active team, matching `AgentBrain.processFinalAsset`
 * (and manual-review defaults) so the UI never shows a stale `team.outputModel` alone.
 */
export function resolveEffectiveGenerationModel(
  llmConfig: LLMConfig,
  team: GenerationModelTeamPick
): string {
  const explicit = team.outputModel?.trim() ?? '';
  const out = team.outputType;

  if (out === 'image') {
    return resolveMediaGenerationModel(llmConfig, 'image', explicit);
  }
  if (out === 'music') {
    return resolveMediaGenerationModel(llmConfig, 'music', explicit);
  }
  if (out === 'video') {
    return resolveMediaGenerationModel(llmConfig, 'video', explicit);
  }
  // Text: no separate “generation” API — agents use the active chat backend (not Gemini’s slot in chatModelsByBackend).
  return resolveChatModelForSession(llmConfig, team.leadAgent?.model);
}

/** Veo “lite” (and similar) APIs accept only one reference image. */
export function maxReferenceImagesForVideoModelId(modelId: string): number {
  return modelId.toLowerCase().includes('lite') ? 1 : 3;
}
