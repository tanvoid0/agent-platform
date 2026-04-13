import React, { type RefObject } from 'react'
import type { LLMMessage, PlanningFormAnswers, PlanningFormSpec } from '../../core/llm/types'
import type { AgenticSystem, AgentNode } from '../../data/agents'
import type { Task } from '../../integration/store/coreStore'
import { ChatMessageRow } from './ChatMessageRow'
import { ChatThinkingIndicator } from './ChatThinkingIndicator'
import type { TeamProjectReviewDraft } from './chatReviewTypes'

export const ChatMessageList: React.FC<{
  scrollRef: RefObject<HTMLDivElement | null>
  chatMessages: LLMMessage[]
  selectedNpcIndex: number
  agent: AgentNode | null
  activeTeam: AgenticSystem
  tasks: Task[]
  setActiveAuditTaskId: (taskId: string) => void
  setTemplateHandshake: (v: { teamId: string; headlineName?: string }) => void
  switchSimulationToTeam: (teamId: string, teamName: string) => void
  openReviewFlow: (draft: TeamProjectReviewDraft) => void
  switchSimulationToProject: (projectId: string, projectTitle: string) => void
  onPlanningFormSubmit: (
    historyIndex: number,
    spec: PlanningFormSpec,
    answers: PlanningFormAnswers,
  ) => void
  isThinking: boolean
  lastVisibleHistoryIndex: number
  pendingRetryText: string | null
  canChat: boolean
  chatBlockedReason: string | null
  onRetryLastChat: () => void
}> = ({
  scrollRef,
  chatMessages,
  selectedNpcIndex,
  agent,
  activeTeam,
  tasks,
  setActiveAuditTaskId,
  setTemplateHandshake,
  switchSimulationToTeam,
  openReviewFlow,
  switchSimulationToProject,
  onPlanningFormSubmit,
  isThinking,
  lastVisibleHistoryIndex,
  pendingRetryText,
  canChat,
  chatBlockedReason,
  onRetryLastChat,
}) => (
  <div
    ref={scrollRef}
    className="min-h-0 min-w-0 flex-1 space-y-6 overflow-y-auto p-1 [scrollbar-width:none] [-ms-overflow-style:none] [&::-webkit-scrollbar]:display-none"
  >
    {chatMessages.map((msg, historyIndex) => (
      <ChatMessageRow
        key={`chat-${selectedNpcIndex}-${historyIndex}`}
        msg={msg}
        historyIndex={historyIndex}
        selectedNpcIndex={selectedNpcIndex}
        agent={agent}
        activeTeam={activeTeam}
        tasks={tasks}
        setActiveAuditTaskId={setActiveAuditTaskId}
        setTemplateHandshake={setTemplateHandshake}
        switchSimulationToTeam={switchSimulationToTeam}
        openReviewFlow={openReviewFlow}
        switchSimulationToProject={switchSimulationToProject}
        onPlanningFormSubmit={onPlanningFormSubmit}
        isThinking={isThinking}
        lastVisibleHistoryIndex={lastVisibleHistoryIndex}
        pendingRetryText={pendingRetryText}
        canChat={canChat}
        chatBlockedReason={chatBlockedReason}
        onRetryLastChat={onRetryLastChat}
      />
    ))}

    {isThinking && <ChatThinkingIndicator />}
  </div>
)
