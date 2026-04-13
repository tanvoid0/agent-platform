/**
 * Single entry point for LLM operations outside of raw providers.
 * App code should not branch on vendor names; use this facade for capabilities, routing, and error shape.
 */
import { BudgetExceededError } from '../finance/budgetPolicy';
import { resolveChatCompletionBackend, useGeminiInDev, type ChatCompletionBackendId } from './chatBackendEnv';
import { GeminiProvider } from './providers/GeminiProvider';
import { OrchestratorProxyProvider } from './providers/OrchestratorProxyProvider';
import { getProviderModelCatalog } from './providerModelCatalog';
import type { LLMConfig, LLMProvider } from './types';
import { MODEL_CONFIG } from '../../../model-config';

export type { ChatCompletionBackendId } from './chatBackendEnv';

export class LlmError extends Error {
  declare readonly cause?: unknown;

  constructor(
    message: string,
    public readonly code: 'MISSING_API_KEY' | 'PROVIDER_ERROR' | 'UNKNOWN',
    options?: { cause?: unknown }
  ) {
    super(message);
    this.name = 'LlmError';
    if (options?.cause !== undefined) {
      (this as Error & { cause?: unknown }).cause = options.cause;
    }
  }

  static is(e: unknown): e is LlmError {
    return e instanceof LlmError;
  }
}

/** Immutable snapshot for UI (settings, header). Labels describe connection shape, not a specific LLM vendor. */
export function describeLlmSetup(): {
  chatBackendId: ChatCompletionBackendId;
  /** Short label for pickers: "Server" vs "Cloud" style. */
  chatBackendLabel: string;
  chatRequiresStoredApiKey: boolean;
  /** When true, show server-side chat readiness (Agent Platform → orchestrator probe). */
  showServerChatHealth: boolean;
  apiKeyOptionalForChat: boolean;
  geminiChatForcedInDev: boolean;
} {
  const id = resolveChatCompletionBackend();
  return {
    chatBackendId: id,
    chatBackendLabel: id === 'gemini' ? 'Cloud' : 'Server',
    /** Cloud chat needs `VITE_GEMINI_API_KEY` (not browser-stored). */
    chatRequiresStoredApiKey: id === 'gemini',
    showServerChatHealth: id === 'ollama',
    apiKeyOptionalForChat: id === 'ollama',
    geminiChatForcedInDev: import.meta.env.DEV && useGeminiInDev(),
  };
}

export function getActiveChatBackendId(): ChatCompletionBackendId {
  const configured = MODEL_CONFIG.routing.chat as 'auto' | 'gemini' | 'ollama';
  if (configured === 'gemini' || configured === 'ollama') return configured;
  return resolveChatCompletionBackend();
}

/**
 * Resolves the model id for the active chat completion backend (from catalog + stored map).
 * Empty `agentModel` means “use the session default” (`chatModelsByBackend[active]` + env overrides).
 */
export function resolveChatModelForSession(
  llmConfig: Pick<LLMConfig, 'chatModelsByBackend'>,
  agentModel: string | undefined
): string {
  const id = resolveChatCompletionBackend();
  const slice = getProviderModelCatalog(id).chat;
  const settingsModel = llmConfig.chatModelsByBackend[id]?.trim() ?? '';
  const agent = agentModel?.trim() ?? '';

  if (id === 'ollama') {
    const envModel = import.meta.env.VITE_OLLAMA_MODEL?.trim();
    if (envModel) return envModel;
    if (agent && !agent.toLowerCase().includes('gemini')) return agent;
    if (settingsModel && !settingsModel.toLowerCase().includes('gemini')) return settingsModel;
    return slice.defaultModel;
  }

  if (!agent) {
    if (settingsModel) return settingsModel;
    return slice.defaultModel;
  }
  return agent;
}

/**
 * Builds the chat `LLMProvider` for the current environment. Throws `LlmError` if credentials are missing.
 *
 * **Network boundary:** `OrchestratorProxyProvider` (local / server stack) only uses
 * `POST {Agent Platform}/api/v1/chat` — the browser never talks to Ollama or the orchestrator host.
 * `GeminiProvider` uses `@google/genai` in the browser to Google; if product policy requires
 * that only the backend may call any LLM, Gemini chat must be moved behind a FastAPI proxy.
 */
export function createChatLlmProvider(apiKey: string | undefined): LLMProvider {
  const id = getActiveChatBackendId();
  if (id === 'gemini') {
    const k = apiKey?.trim();
    if (!k) {
      throw new LlmError('API key required for chat', 'MISSING_API_KEY');
    }
    return new GeminiProvider(k);
  }
  return new OrchestratorProxyProvider();
}

/** When true, chat completion should count toward cloud budget policy. */
export function chatCompletionUsesCloudBilling(): boolean {
  return getActiveChatBackendId() === 'gemini';
}

/** Cloud pipeline (Gemini) can run final image/audio/video when `VITE_GEMINI_API_KEY` is set. */
export function isCloudMediaPipelineReady(apiKey: string | undefined | null): boolean {
  return Boolean(apiKey?.trim());
}

export type MediaOutputKind = 'image' | 'music' | 'video';
export type MediaBackendId = 'gemini' | 'ollama' | 'disabled';

export function resolveMediaBackend(kind: MediaOutputKind): MediaBackendId {
  const configured = MODEL_CONFIG.routing[kind] as MediaBackendId | 'follow-chat';
  if (configured === 'follow-chat') {
    return getActiveChatBackendId();
  }
  if (configured === 'gemini' || configured === 'ollama' || configured === 'disabled') {
    return configured;
  }
  return getActiveChatBackendId();
}

/** Model id options for team / review pickers (text = active chat backend; media = routed backend). */
export function getOutputModelPickerOptions(
  outputType: 'text' | 'image' | 'music' | 'video'
): readonly string[] {
  if (outputType === 'text') {
    return getProviderModelCatalog(getActiveChatBackendId()).chat.options;
  }
  const kind: MediaOutputKind = outputType === 'music' ? 'music' : outputType;
  const backend = resolveMediaBackend(kind);
  if (backend === 'disabled') return [];
  const slice = getProviderModelCatalog(backend)[kind];
  return slice?.options ?? [];
}

export function defaultOutputModelForType(
  outputType: 'text' | 'image' | 'music' | 'video'
): string {
  if (outputType === 'text') {
    return getProviderModelCatalog(getActiveChatBackendId()).chat.defaultModel;
  }
  const kind: MediaOutputKind = outputType === 'music' ? 'music' : outputType;
  const backend = resolveMediaBackend(kind);
  if (backend === 'disabled') return '';
  const slice = getProviderModelCatalog(backend)[kind];
  return slice?.defaultModel ?? '';
}

/** True when any non-text deliverable is routed to Gemini (needs API key for real media generation). */
export function anyMediaRoutedToGemini(): boolean {
  return (
    resolveMediaBackend('image') === 'gemini' ||
    resolveMediaBackend('music') === 'gemini' ||
    resolveMediaBackend('video') === 'gemini'
  );
}

export function getMediaReadiness(
  kind: MediaOutputKind,
  apiKey: string | undefined | null
): { ready: boolean; backend: MediaBackendId; reason?: string } {
  const backend = resolveMediaBackend(kind);
  if (backend === 'disabled') {
    return {
      ready: false,
      backend,
      reason: `Generation for ${kind} is disabled in model-config routing.`,
    };
  }
  if (backend === 'gemini') {
    if (!isCloudMediaPipelineReady(apiKey)) {
      return {
        ready: false,
        backend,
        reason: 'Gemini backend selected but no API key is configured.',
      };
    }
    return { ready: true, backend };
  }
  // Server-path media generation is not implemented in this app yet.
  return {
    ready: false,
    backend,
    reason: `Server ${kind} generation is not integrated yet in this app.`,
  };
}

/** Client for Gemini-only multimodal APIs (today). */
export function createCloudMediaClient(apiKey: string): GeminiProvider {
  return new GeminiProvider(apiKey.trim());
}

export type AgentThinkFailure =
  | { kind: 'budget'; message: string }
  | { kind: 'llm_failure'; message: string; openByok: boolean };

/** Normalize any chat/think error into a small union for UI and logging. */
export function analyzeAgentThinkFailure(error: unknown): AgentThinkFailure {
  if (BudgetExceededError.is(error)) {
    return { kind: 'budget', message: error.message };
  }
  if (LlmError.is(error) && error.code === 'MISSING_API_KEY') {
    return { kind: 'llm_failure', message: error.message, openByok: true };
  }
  const msg = error instanceof Error ? error.message : String(error);
  return { kind: 'llm_failure', message: msg, openByok: true };
}

export type CloudMediaFailure =
  | { kind: 'budget'; message: string }
  | { kind: 'credentials_gap'; message: string }
  | { kind: 'provider'; message: string };

export function analyzeCloudMediaFailure(
  error: unknown,
  ctx: { multimodalOutput: boolean }
): CloudMediaFailure {
  if (BudgetExceededError.is(error)) {
    return { kind: 'budget', message: error.message };
  }
  const msg = error instanceof Error ? error.message : String(error);
  if (
    ctx.multimodalOutput &&
    /api key|API key|required for final|401|403|unauthori/i.test(msg)
  ) {
    return { kind: 'credentials_gap', message: msg };
  }
  return { kind: 'provider', message: msg };
}
