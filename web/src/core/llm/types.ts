import type { ChatCompletionBackendId } from './chatBackendEnv';

export type LLMRole = 'system' | 'user' | 'assistant' | 'tool';

export type PlanningFormFieldKind =
  | 'boolean'
  | 'single_select'
  | 'multi_select'
  | 'text'
  | 'textarea';

export interface PlanningFormField {
  id: string;
  label: string;
  kind: PlanningFormFieldKind;
  /** Required for `single_select` and `multi_select`. */
  options?: string[];
  required?: boolean;
  helpText?: string;
}

export interface PlanningFormSpec {
  title?: string;
  description?: string;
  fields: PlanningFormField[];
}

/** Values submitted for planning fields; keyed by field `id`. */
export type PlanningFormAnswers = Record<string, string | boolean | string[]>;

/** Optional fields persisted on messages for UI / session features; extra keys allowed for forward compatibility. */
export interface LLMMessageMetadata {
  internal?: boolean;
  reviewTaskId?: string;
  savedTeamTemplateId?: string;
  savedTeamTemplateName?: string;
  createdProjectId?: string;
  createdProjectTitle?: string;
  planningForm?: PlanningFormSpec;
  planningFormStatus?: 'open' | 'submitted';
  planningFormAnswers?: PlanningFormAnswers;
  [key: string]: unknown;
}

export interface LLMMessage {
  role: LLMRole;
  content: string;
  name?: string; // Required for tool responses in some APIs
  tool_calls?: LLMToolCall[];
  images?: string[]; // Optional base64 images
  metadata?: LLMMessageMetadata;
}

export interface LLMToolCall {
  id: string;
  type: 'function';
  function: {
    name: string;
    arguments: string; // JSON string
  };
}

export interface LLMToolDefinition {
  type: 'function';
  function: {
    name: string;
    description: string;
    /** JSON Schema object passed through to providers. */
    parameters: unknown;
  };
}

/** Chat completion model id keyed by registered completion backend (see `PROVIDER_MODEL_CATALOGS`). */
export type ChatModelsByBackend = Record<ChatCompletionBackendId, string>;

export interface LLMConfig {
  apiKey?: string;
  baseUrl?: string;
  /** Per–chat-backend completion model ids (see `PROVIDER_MODEL_CATALOGS`). */
  chatModelsByBackend: ChatModelsByBackend;
  /** Default image model for cloud deliverables (teams may override with outputModel). */
  imageModel?: string;
  musicModel?: string;
  videoModel?: string;
}

export interface LLMRequestDetails {
  contents: unknown[];
  systemInstruction?: string;
  tools?: unknown[];
}

export interface LLMTokenUsage {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
}

export interface LLMResponse {
  content: string | null;
  tool_calls?: LLMToolCall[];
  usage?: LLMTokenUsage;
  finishReason?: string;
  raw?: unknown;
  request?: LLMRequestDetails;
}

export interface LLMProvider {
  generateCompletion(
    messages: LLMMessage[],
    tools?: LLMToolDefinition[],
    systemInstruction?: string,
    modelName?: string
  ): Promise<LLMResponse>;
}
