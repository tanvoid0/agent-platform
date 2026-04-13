import { agentPlatformAuthHeaders, apiUrl } from '../../../api/client';
import { mapMessagesToOpenAI, normalizeToolCallsFromOpenAI, openAiToolsFromDefinitions } from '../openAiChatMessages';
import { getProviderModelCatalog } from '../providerModelCatalog';
import {
  LLMMessage,
  LLMProvider,
  LLMResponse,
  LLMToolDefinition,
} from '../types';

/**
 * Chat completions **only** via Agent Platform (`POST /api/v1/chat`). The UI does not call
 * Ollama, the orchestrator, or any LLM host directly — only this origin + path from the browser.
 */
export class OrchestratorProxyProvider implements LLMProvider {
  async generateCompletion(
    messages: LLMMessage[],
    tools?: LLMToolDefinition[],
    systemInstruction?: string,
    modelName?: string
  ): Promise<LLMResponse> {
    const openaiMessages = mapMessagesToOpenAI(messages, systemInstruction);
    const body: Record<string, unknown> = {
      model: modelName || getProviderModelCatalog('ollama').chat.defaultModel,
      messages: openaiMessages,
      stream: false,
    };

    if (tools && tools.length > 0) {
      body.tools = openAiToolsFromDefinitions(tools);
    }

    const headers: Record<string, string> = {
      'Content-Type': 'application/json',
      ...agentPlatformAuthHeaders(),
    };

    const res = await fetch(apiUrl('/api/v1/chat'), {
      method: 'POST',
      headers,
      body: JSON.stringify(body),
    });

    const raw = await res.json().catch(() => ({}));
    if (!res.ok) {
      const msg =
        (raw as { detail?: string; error?: { message?: string } | string })?.detail ||
        (raw as { error?: { message?: string } })?.error?.message ||
        (typeof (raw as { error?: string }).error === 'string'
          ? (raw as { error: string }).error
          : null) ||
        res.statusText;
      throw new Error(`LLM: ${msg || res.status}`);
    }

    const choice = (raw as { choices?: Array<{ message?: Record<string, unknown>; finish_reason?: string }> })
      .choices?.[0];
    const message = choice?.message;
    const content =
      typeof message?.content === 'string'
        ? message.content
        : Array.isArray(message?.content)
          ? (message!.content as { type?: string; text?: string }[])
              .map((p) => (p.type === 'text' && p.text ? p.text : ''))
              .join('')
          : null;

    const tool_calls = normalizeToolCallsFromOpenAI(message?.tool_calls);

    const usageRaw = (raw as { usage?: Record<string, number> }).usage;
    const usage =
      usageRaw &&
      (usageRaw.prompt_tokens != null || usageRaw.completion_tokens != null)
        ? {
            promptTokens: usageRaw.prompt_tokens ?? 0,
            completionTokens: usageRaw.completion_tokens ?? 0,
            totalTokens: usageRaw.total_tokens ?? (usageRaw.prompt_tokens ?? 0) + (usageRaw.completion_tokens ?? 0),
          }
        : undefined;

    return {
      content,
      tool_calls: tool_calls.length > 0 ? tool_calls : undefined,
      usage,
      finishReason: choice?.finish_reason,
      raw,
      request: {
        contents: openaiMessages,
        systemInstruction,
        tools: body.tools as any[],
      },
    };
  }
}
