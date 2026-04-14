import type { LLMMessage } from '@/core/llm/types';
import { visibleChatTurnCount } from '../chat/chatReadWatermark';

export function appendHistoryMessage(
  history: LLMMessage[] | undefined,
  role: 'user' | 'assistant',
  parts: readonly unknown[],
): LLMMessage[] {
  return [
    ...(history ?? []),
    {
      role,
      content: Array.isArray(parts)
        ? parts.map((p) => (typeof p === 'string' ? p : JSON.stringify(p))).join(' ')
        : String(parts),
    },
  ];
}

export function chatReadWatermarkForHistory(history: LLMMessage[] | undefined): number {
  return visibleChatTurnCount(history);
}
