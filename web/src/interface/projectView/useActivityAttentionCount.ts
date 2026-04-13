import { useMemo } from 'react'
import { getAllCharacters } from '../../data/agents'
import { totalUnreadAssistantTurns } from '../../integration/chat/chatReadWatermark'
import { hasActiveOrchestrationWork } from '../../integration/projectOrchestrationSync'
import { useCoreStore } from '../../integration/store/coreStore'
import { useActiveTeam } from '../../integration/store/teamStore'
import { useUiStore } from '../../integration/store/uiStore'
import { buildInputRequestList, type InputRequestItem } from './buildInputRequestList'

export function useActivityAttentionCount(): {
  activityAttentionCount: number
  inputRequests: InputRequestItem[]
  orchestrationActive: boolean
} {
  const phase = useCoreStore((s) => s.phase)
  const agentsOrchestrationPaused = useCoreStore((s) => s.agentsOrchestrationPaused)
  const tasks = useCoreStore((s) => s.tasks)
  const isGeneratingAsset = useCoreStore((s) => s.isGeneratingAsset)
  const taskExecution = useCoreStore((s) => s.taskExecution)
  const agentHistories = useCoreStore((s) => s.agentHistories)
  const chatReadVisibleLength = useCoreStore((s) => s.chatReadVisibleLength)
  const chatIsThinking = useUiStore((s) => s.isThinking)
  const activeTeam = useActiveTeam()

  const characters = useMemo(() => getAllCharacters(activeTeam), [activeTeam])

  const orchestrationActive = useMemo(
    () =>
      hasActiveOrchestrationWork({
        phase,
        agentsOrchestrationPaused,
        tasks,
        taskExecution,
        isGeneratingAsset,
        chatIsThinking,
      }),
    [phase, agentsOrchestrationPaused, tasks, taskExecution, isGeneratingAsset, chatIsThinking],
  )

  const inputRequests = useMemo(
    () =>
      buildInputRequestList(
        phase,
        isGeneratingAsset,
        orchestrationActive,
        activeTeam.leadAgent.index,
        activeTeam.leadAgent.name,
        tasks,
        characters,
        agentHistories,
        chatReadVisibleLength,
      ),
    [
      phase,
      isGeneratingAsset,
      orchestrationActive,
      activeTeam.leadAgent.index,
      activeTeam.leadAgent.name,
      tasks,
      characters,
      agentHistories,
      chatReadVisibleLength,
    ],
  )

  const activityAttentionCount =
    inputRequests.length + totalUnreadAssistantTurns(agentHistories, chatReadVisibleLength)

  return { activityAttentionCount, inputRequests, orchestrationActive }
}
