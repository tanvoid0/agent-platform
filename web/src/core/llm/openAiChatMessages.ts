/**
 * Maps app `LLMMessage[]` to OpenAI chat-completions message objects (tools, multimodal, tool loop).
 * Shared by the agent-platform proxy provider and any direct OpenAI-compat client.
 */
import type { LLMMessage, LLMToolCall, LLMToolDefinition } from './types';

export function normalizeToolCallsFromOpenAI(raw: unknown): LLMToolCall[] {
  if (!Array.isArray(raw)) return [];
  const out: LLMToolCall[] = [];
  for (let i = 0; i < raw.length; i++) {
    const tc = raw[i] as {
      id?: string;
      type?: string;
      function?: { name?: string; arguments?: string };
    };
    const name = tc.function?.name;
    if (!name) continue;
    let args = tc.function?.arguments ?? '{}';
    if (typeof args !== 'string') args = JSON.stringify(args);
    out.push({
      id: tc.id || `openai-tool-${i}-${Math.random().toString(36).slice(2, 9)}`,
      type: 'function',
      function: { name, arguments: args },
    });
  }
  return out;
}

function userContentParts(m: LLMMessage): string | unknown[] {
  if (!m.images?.length) return m.content ?? '';

  const parts: unknown[] = [];
  for (const img of m.images) {
    const url = toImageUrlForOpenAI(img);
    if (url) parts.push({ type: 'image_url', image_url: { url } });
  }
  if (m.content) parts.push({ type: 'text', text: m.content });
  if (parts.length === 0) return m.content ?? '';
  return parts;
}

/** OpenAI-compat shims reject null/empty assistant text in some stacks. */
function assistantPlainText(text: string | undefined | null): string {
  return text != null && text !== '' ? text : '';
}

function toImageUrlForOpenAI(img: string): string | null {
  const base64Match = img.match(/^data:(image\/[a-z+.-]+);base64,(.+)$/i);
  if (base64Match) return `data:${base64Match[1]};base64,${base64Match[2]}`;
  if (img.startsWith('data:') || img.startsWith('http://') || img.startsWith('https://')) return img;
  return `data:image/jpeg;base64,${img}`;
}

/**
 * OpenAI-compatible APIs require a `tool` message after each assistant message that has `tool_calls`.
 * This app merges tool results delivered as internal `user` messages ([SYSTEM]…) with synthetic ok payloads.
 */
export function mapMessagesToOpenAI(
  messages: LLMMessage[],
  systemInstruction?: string
): Record<string, unknown>[] {
  const out: Record<string, unknown>[] = [];
  if (systemInstruction?.trim()) {
    out.push({ role: 'system', content: systemInstruction });
  }

  let i = 0;
  while (i < messages.length) {
    const m = messages[i];
    if (m.role === 'assistant' && m.tool_calls?.length) {
      out.push({
        role: 'assistant',
        content: assistantPlainText(m.content),
        tool_calls: m.tool_calls.map((tc) => ({
          id: tc.id,
          type: 'function',
          function: {
            name: tc.function.name,
            arguments: tc.function.arguments,
          },
        })),
      });
      i++;
      for (const tc of m.tool_calls) {
        const next = messages[i];
        const isToolFeedback =
          next?.role === 'user' &&
          (next.content?.startsWith('[SYSTEM]') || next.metadata?.internal === true);
        if (isToolFeedback) {
          out.push({
            role: 'tool',
            tool_call_id: tc.id,
            content: next.content || '',
          });
          i++;
        } else {
          out.push({
            role: 'tool',
            tool_call_id: tc.id,
            content: '{"success":true}',
          });
        }
      }
      continue;
    }

    if (m.role === 'user') {
      out.push({ role: 'user', content: userContentParts(m) });
      i++;
      continue;
    }

    if (m.role === 'assistant') {
      out.push({ role: 'assistant', content: assistantPlainText(m.content) });
      i++;
      continue;
    }

    i++;
  }

  return out;
}

export function openAiToolsFromDefinitions(tools: LLMToolDefinition[]): unknown[] {
  return tools.map((t) => ({
    type: 'function',
    function: {
      name: t.function.name,
      description: t.function.description,
      parameters: t.function.parameters ?? { type: 'object', properties: {} },
    },
  }));
}
