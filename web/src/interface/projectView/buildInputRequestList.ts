import type { LLMMessage } from '../../core/llm/types'
import {
  lastVisibleChatMessage,
} from '../../integration/projectOrchestrationSync'
import { unreadAssistantTurnsForAgent } from '../../integration/chat/chatReadWatermark'
import type { ProjectPhase } from '../../integration/store/coreStore'

export type InputRequestItem =
  | { id: string; kind: 'brief'; agentIndex: number; title: string; detail: string }
  | { id: string; kind: 'proposed_task'; agentIndex: number; taskId: string; title: string; detail: string }
  | { id: string; kind: 'review'; agentIndex: number; taskId: string; title: string; detail: string }
  | { id: string; kind: 'chat_reply'; agentIndex: number; title: string; detail: string }

export function buildInputRequestList(
  phase: ProjectPhase,
  isGeneratingAsset: boolean,
  orchestrationActive: boolean,
  leadIndex: number,
  leadName: string,
  tasks: {
    id: string
    title: string
    status: string
    assignedAgentId: number
    requiresUserApproval?: boolean
  }[],
  characters: { index: number; name: string; color: string }[],
  agentHistories: Record<number, LLMMessage[]>,
  chatReadVisibleLength: Record<number, number>,
): InputRequestItem[] {
  const out: InputRequestItem[] = []
  if (phase === 'idle' && !isGeneratingAsset) {
    out.push({
      id: 'brief',
      kind: 'brief',
      agentIndex: leadIndex,
      title: leadName,
      detail: 'Project brief — chat with the lead to define or refine scope',
    })
  }
  const proposed = tasks.filter((t) => t.status === 'scheduled' && t.requiresUserApproval)
  for (const t of proposed) {
    const agent = characters.find((c) => c.index === t.assignedAgentId)
    out.push({
      id: `proposed-${t.id}`,
      kind: 'proposed_task',
      agentIndex: t.assignedAgentId,
      taskId: t.id,
      title: agent?.name ?? `Agent ${t.assignedAgentId}`,
      detail: t.title || 'Proposed task — approve to queue work',
    })
  }
  const onHold = tasks.filter((t) => t.status === 'review')
  for (const t of onHold) {
    const agent = characters.find((c) => c.index === t.assignedAgentId)
    out.push({
      id: `review-${t.id}`,
      kind: 'review',
      agentIndex: t.assignedAgentId,
      taskId: t.id,
      title: agent?.name ?? `Agent ${t.assignedAgentId}`,
      detail: t.title || 'Review requested',
    })
  }

  if ((phase === 'working' || phase === 'done') && !isGeneratingAsset && !orchestrationActive) {
    const hist = agentHistories[leadIndex]
    const last = lastVisibleChatMessage(hist)
    const leadUnread = unreadAssistantTurnsForAgent(hist, chatReadVisibleLength[leadIndex])
    if (last?.role === 'assistant' && leadUnread === 0) {
      out.push({
        id: 'chat-lead-followup',
        kind: 'chat_reply',
        agentIndex: leadIndex,
        title: leadName,
        detail: 'Lead is waiting for your reply in chat — open chat to continue',
      })
    }
  }

  return out
}
