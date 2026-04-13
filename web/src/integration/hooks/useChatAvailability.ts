import { useCoreStore } from '../store/coreStore'
import { useActiveTeam } from '../store/teamStore'
import { useUiStore } from '../store/uiStore'
import { useDelegationConnectivity } from './useDelegationConnectivity'

export interface ChatAvailability {
  canChat: boolean
  reason: string
}

/**
 * Derives whether the player can chat with a given agent based on
 * the current project phase and the agent's task state.
 */
export function useChatAvailability(agentIndex: number | null): ChatAvailability {
  const { phase, tasks, isGeneratingAsset } = useCoreStore()
  const agentStatus = useUiStore((s) => (agentIndex !== null ? s.agentStatuses[agentIndex] : 'idle'))
  const system = useActiveTeam()
  const { backendBlocksChat, backendReason } = useDelegationConnectivity()

  if (agentIndex === null) return { canChat: false, reason: '' }
  if (isGeneratingAsset) return { canChat: false, reason: 'Delivering...' }
  if (backendBlocksChat) return { canChat: false, reason: backendReason }

  const isLead = agentIndex === system.leadAgent.index

  // 1. Idle Phase: Only Lead Agent can chat (to set the brief)
  if (phase === 'idle') {
    return isLead ? { canChat: true, reason: '' } : { canChat: false, reason: 'Waiting for brief' }
  }

  // 2. Working Phase: allow chat for active agents too, so users can add context mid-run.
  // We only block explicit review states.
  if (isLead || agentStatus === 'idle' || agentStatus === 'working' || agentStatus === 'talking') {
    return { canChat: true, reason: '' }
  }

  // Provide specific reason for busy agents
  if (agentStatus === 'on_hold') return { canChat: false, reason: 'Review requested...' }

  const activeTask = tasks.find((t) => t.assignedAgentId === agentIndex && t.status === 'in_progress')
  return { 
    canChat: false, 
    reason: activeTask ? `Working on: "${activeTask.title}"` : 'Agent is busy' 
  }
}
