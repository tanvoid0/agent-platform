import type { LLMMessage } from '../../core/llm/types';
import {
  chatReplyPendingFromHistory,
  lastVisibleChatMessage,
} from '../projectOrchestrationSync';
import { unreadAssistantTurnsForAgent } from './chatReadWatermark';

export type AgentChatAttentionContext = {
  phase: 'idle' | 'working' | 'done';
  isGeneratingAsset: boolean;
  orchestrationActive: boolean;
  leadAgentIndex: number;
  agentHistories: Record<number, LLMMessage[]>;
  chatReadVisibleLength: Record<number, number>;
  isChatting: boolean;
  selectedNpcIndex: number | null;
};

/**
 * True when this agent's chat thread deserves a nudge (unread / pending reply / lead follow-up),
 * matching the chat-related items in ProjectView's "needs input" list.
 * Suppressed while the user already has an open chat with this agent.
 */
export function agentNeedsChatAttention(agentIndex: number, ctx: AgentChatAttentionContext): boolean {
  if (ctx.phase === 'done') return false;
  if (ctx.isChatting && ctx.selectedNpcIndex === agentIndex) return false;

  const hist = ctx.agentHistories[agentIndex];

  if (unreadAssistantTurnsForAgent(hist, ctx.chatReadVisibleLength[agentIndex]) > 0) return true;
  if (chatReplyPendingFromHistory(hist)) return true;

  if (ctx.phase === 'idle' && !ctx.isGeneratingAsset && agentIndex === ctx.leadAgentIndex) return true;

  if (
    ctx.phase === 'working' &&
    !ctx.isGeneratingAsset &&
    !ctx.orchestrationActive &&
    agentIndex === ctx.leadAgentIndex
  ) {
    const last = lastVisibleChatMessage(hist);
    const leadUnread = unreadAssistantTurnsForAgent(hist, ctx.chatReadVisibleLength[ctx.leadAgentIndex]);
    if (last?.role === 'assistant' && leadUnread === 0) return true;
  }

  return false;
}

export function countAgentsNeedingChatAttention(
  agentIndices: number[],
  ctx: AgentChatAttentionContext,
): number {
  return agentIndices.filter((idx) => agentNeedsChatAttention(idx, ctx)).length;
}
