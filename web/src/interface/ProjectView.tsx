import React, { useCallback, useMemo, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { hasRemoteProjectBackend } from '../integration/api/projectRemoteApi'
import { unreadAssistantTurnsForAgent } from '../integration/chat/chatReadWatermark'
import {
  createAndSwitchToNewProject,
  persistClearedProjectWorkspace,
} from '../integration/projectPersistence'
import { useCoreStore } from '../integration/store/coreStore'
import { useActiveTeam } from '../integration/store/teamStore'
import { useUiStore } from '../integration/store/uiStore'
import { getAllCharacters } from '../data/agents'
import { useSceneManager } from '../simulation/SceneContext'
import { useHeartbeatNow } from '../integration/hooks/useHeartbeatNow'
import {
  persistProjectSideTab,
  readStoredProjectSideTab,
} from '../integration/ui/projectSideTabStorage'
import PricingModal from './PricingModal'
import ResetModal from './ResetModal'
import type { InputRequestItem } from './projectView/buildInputRequestList'
import { useProjectStatusBadge } from './projectView/useProjectStatusBadge'
import { ExecutionMonitorSection } from './projectView/ExecutionMonitorSection'
import { InputRequestsSection } from './projectView/InputRequestsSection'
import { ProjectResetActions } from './projectView/ProjectResetActions'
import { ProjectSideTabs, type ProjectSideTab } from './projectView/ProjectSideTabs'
import { ProjectStatsSection } from './projectView/ProjectStatsSection'
import { ProjectViewHeader } from './projectView/ProjectViewHeader'
import { TokenUsageSection } from './projectView/TokenUsageSection'
import { UnreadMessagesSection } from './projectView/UnreadMessagesSection'
import { UserBriefSection } from './projectView/UserBriefSection'

export type ProjectViewProps = {
  sideTab?: ProjectSideTab
  onSideTab?: (tab: ProjectSideTab) => void
  hideTabBar?: boolean
}

const ProjectView: React.FC<ProjectViewProps> = ({
  sideTab: controlledSideTab,
  onSideTab: controlledOnSideTab,
  hideTabBar,
}) => {
  const userBrief = useCoreStore((s) => s.userBrief)
  const referenceImages = useCoreStore((s) => s.referenceImages)
  const phase = useCoreStore((s) => s.phase)
  const actionLog = useCoreStore((s) => s.actionLog)
  const resetProject = useCoreStore((s) => s.resetProject)
  const bumpSimSceneReset = useCoreStore((s) => s.bumpSimSceneReset)
  const agentsOrchestrationPaused = useCoreStore((s) => s.agentsOrchestrationPaused)
  const tasks = useCoreStore((s) => s.tasks)
  const isGeneratingAsset = useCoreStore((s) => s.isGeneratingAsset)
  const taskExecution = useCoreStore((s) => s.taskExecution)
  const agentHistories = useCoreStore((s) => s.agentHistories)
  const chatReadVisibleLength = useCoreStore((s) => s.chatReadVisibleLength)
  const setAgentsOrchestrationPaused = useCoreStore((s) => s.setAgentsOrchestrationPaused)
  const setPhase = useCoreStore((s) => s.setPhase)
  const addLogEntry = useCoreStore((s) => s.addLogEntry)
  const totalEstimatedCost = useCoreStore((s) => s.totalEstimatedCost)
  const totalTokenUsage = useCoreStore((s) => s.totalTokenUsage)
  const agentTokenUsage = useCoreStore((s) => s.agentTokenUsage)
  const agentEstimatedCost = useCoreStore((s) => s.agentEstimatedCost)
  const [isResetModalOpen, setIsResetModalOpen] = useState(false)
  const [isPricingModalOpen, setIsPricingModalOpen] = useState(false)
  const [internalSideTab, setInternalSideTab] = useState<ProjectSideTab>(() =>
    readStoredProjectSideTab(['overview', 'activity'], 'overview'),
  )
  const activeTeam = useActiveTeam()
  const navigate = useNavigate()
  const scene = useSceneManager()
  const selectedNpcIndex = useUiStore((s) => s.selectedNpcIndex)
  const isChatting = useUiStore((s) => s.isChatting)
  const activeAuditTaskId = useUiStore((s) => s.activeAuditTaskId)
  const setSelectedNpc = useUiStore((s) => s.setSelectedNpc)
  const setActiveAuditTaskId = useUiStore((s) => s.setActiveAuditTaskId)
  const approveAllAwaitingUserInput = useCoreStore((s) => s.approveAllAwaitingUserInput)

  const isControlled =
    controlledSideTab !== undefined && controlledOnSideTab !== undefined
  const sideTab = isControlled ? controlledSideTab : internalSideTab
  const setSideTab = useCallback(
    (tab: ProjectSideTab) => {
      if (isControlled) {
        controlledOnSideTab!(tab)
      } else {
        setInternalSideTab(tab)
        persistProjectSideTab(tab)
      }
    },
    [isControlled, controlledOnSideTab],
  )

  const { badge: projectStatusBadge, activityAttentionCount, inputRequests } = useProjectStatusBadge()

  const characters = useMemo(() => getAllCharacters(activeTeam), [activeTeam])

  const hasExecutionRows = useMemo(
    () => tasks.some((task) => Boolean(taskExecution[task.id])),
    [tasks, taskExecution],
  )

  const nowTs = useHeartbeatNow(sideTab === 'activity' && hasExecutionRows)

  const handleInputRequestClick = useCallback(
    (item: InputRequestItem) => {
      setSelectedNpc(item.agentIndex)
      if (item.kind === 'brief' || item.kind === 'chat_reply') {
        scene?.startChat(item.agentIndex)
      } else {
        setActiveAuditTaskId(item.taskId)
      }
    },
    [scene, setSelectedNpc, setActiveAuditTaskId],
  )

  const hasLogs = actionLog.length > 0
  const serverProjectsEnabled = hasRemoteProjectBackend()

  const handleClearThisProjectOnly = async () => {
    scene?.resetScene()
    resetProject()
    bumpSimSceneReset()
    await persistClearedProjectWorkspace()
  }

  const handleStartFreshProject = async (userTitle: string) => {
    await createAndSwitchToNewProject(userTitle)
    scene?.resetScene()
  }

  const handleImproveProject = () => {
    setPhase('working')
    setAgentsOrchestrationPaused(false)
    setSelectedNpc(activeTeam.leadAgent.index)
    scene?.startChat(activeTeam.leadAgent.index)
    addLogEntry({
      agentIndex: activeTeam.leadAgent.index,
      action: 'Project reopened for iteration — waiting for improvement instructions.',
    })
  }

  const executionRows = useMemo(() => {
    return tasks
      .map((task) => {
        const run = taskExecution[task.id]
        if (!run) return null
        return { task, run }
      })
      .filter(Boolean) as Array<{
        task: (typeof tasks)[number]
        run: NonNullable<(typeof taskExecution)[string]>
      }>
  }, [tasks, taskExecution])

  const unreadChatByAgent = useMemo(
    () =>
      characters
        .map((c) => ({
          agentIndex: c.index,
          name: c.name,
          color: c.color,
          count: unreadAssistantTurnsForAgent(
            agentHistories[c.index],
            chatReadVisibleLength[c.index],
          ),
        }))
        .filter((row) => row.count > 0),
    [characters, agentHistories, chatReadVisibleLength],
  )

  return (
    <div className="flex flex-col h-full overflow-y-auto p-6 bg-white/50">
      <ProjectViewHeader
        badge={projectStatusBadge}
        onRequestCleanSlate={() => setIsResetModalOpen(true)}
        onViewProjectOutput={phase === 'done' ? () => navigate('/project-output') : undefined}
      />

      <div className="h-px bg-zinc-100 w-full mb-4" />

      {!hideTabBar && (
        <ProjectSideTabs
          sideTab={sideTab}
          onSideTab={setSideTab}
          activityAttentionCount={activityAttentionCount}
        />
      )}

      {sideTab === 'activity' && executionRows.length > 0 && (
        <ExecutionMonitorSection
          executionRows={executionRows}
          nowTs={nowTs}
          onRetryTask={(taskId) => scene?.retryTaskExecution(taskId)}
          onRetryAllFailedStalled={() => {
            scene?.retryAllFailedOrStalledTaskExecutions()
          }}
        />
      )}

      {sideTab === 'overview' && hasLogs && (
        <ProjectResetActions
          phase={phase}
          serverProjectsEnabled={serverProjectsEnabled}
          onImprove={handleImproveProject}
          onOpenReset={() => setIsResetModalOpen(true)}
        />
      )}

      {sideTab === 'overview' && (
        <UserBriefSection
          userBrief={userBrief}
          referenceImages={referenceImages}
          outputType={activeTeam.outputType}
        />
      )}

      {sideTab === 'overview' && (
        <ProjectStatsSection tasks={tasks} actionLog={actionLog} taskExecution={taskExecution} />
      )}

      {sideTab === 'activity' && unreadChatByAgent.length > 0 && (
        <UnreadMessagesSection
          rows={unreadChatByAgent}
          totalUnread={activityAttentionCount - inputRequests.length}
          leadAgentIndex={activeTeam.leadAgent.index}
          selectedNpcIndex={selectedNpcIndex}
          isChatting={isChatting}
          onOpenAgentChat={(agentIndex) => {
            setSelectedNpc(agentIndex)
            scene?.startChat(agentIndex)
          }}
        />
      )}

      {sideTab === 'activity' && inputRequests.length > 0 && (
        <InputRequestsSection
          items={inputRequests}
          characters={characters}
          selectedNpcIndex={selectedNpcIndex}
          activeAuditTaskId={activeAuditTaskId}
          onItemClick={handleInputRequestClick}
          onApproveAll={approveAllAwaitingUserInput}
        />
      )}

      {sideTab === 'overview' && (
        <TokenUsageSection
          activeTeam={activeTeam}
          totalEstimatedCost={totalEstimatedCost}
          totalTokenUsage={totalTokenUsage}
          agentTokenUsage={agentTokenUsage}
          agentEstimatedCost={agentEstimatedCost}
          onOpenPricing={() => setIsPricingModalOpen(true)}
        />
      )}

      <ResetModal
        isOpen={isResetModalOpen}
        onClose={() => setIsResetModalOpen(false)}
        serverProjectsEnabled={serverProjectsEnabled}
        onConfirmClearThisProject={handleClearThisProjectOnly}
        onConfirmStartFreshProject={serverProjectsEnabled ? handleStartFreshProject : undefined}
      />

      {isPricingModalOpen && <PricingModal onClose={() => setIsPricingModalOpen(false)} />}
    </div>
  )
}

export default ProjectView
