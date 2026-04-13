import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { getAllAgents } from '../data/agents'
import { buildPlanningAnswersUserMessage } from '../core/agent/tools/presentPlanningForm'
import type { PlanningFormAnswers, PlanningFormSpec } from '../core/llm/types'
import { useChatAvailability } from '../integration/hooks/useChatAvailability'
import { pendingChatRetryText } from '../integration/projectOrchestrationSync'
import { switchActiveProject } from '../integration/projectPersistence'
import { useCoreStore } from '../integration/store/coreStore'
import { showToast } from '../integration/store/toastStore'
import { useActiveTeam, useTeamStore } from '../integration/store/teamStore'
import { useChatPanelUi } from '../integration/store/uiSelectors'
import { useUiStore } from '../integration/store/uiStore'
import { useSceneManager } from '../simulation/SceneContext'
import TeamTemplateReviewModal from './TeamTemplateReviewModal'
import { formatChatTranscript } from './chat/chatClipboard'
import { ChatComposer } from './chat/ChatComposer'
import { ChatMessageList } from './chat/ChatMessageList'
import { ChatPanelClearDialog } from './chat/ChatPanelClearDialog'
import { ChatPanelHeader } from './chat/ChatPanelHeader'
import { ChatReviewFlowModal } from './chat/ChatReviewFlowModal'
import { ChatStuckBanner } from './chat/ChatStuckBanner'
import type { TeamProjectReviewDraft } from './chat/chatReviewTypes'
import type { LLMMessage } from '../core/llm/types'

const CHAT_THINKING_STUCK_MS = 120_000

const EMPTY_CHAT_MESSAGES: LLMMessage[] = []

const ChatPanel: React.FC = () => {
  const {
    isChatting,
    isThinking,
    selectedNpcIndex,
    setIsTyping,
    setActiveAuditTaskId,
  } = useChatPanelUi()
  const scene = useSceneManager()
  const navigate = useNavigate()
  const activeTeam = useActiveTeam()
  const agents = getAllAgents(activeTeam)

  const [input, setInput] = useState('')
  const [reviewDraft, setReviewDraft] = useState<TeamProjectReviewDraft | null>(null)
  const [templateHandshake, setTemplateHandshake] = useState<{
    teamId: string
    headlineName?: string
  } | null>(null)
  const handshakeOpenedFor = useRef<Set<string>>(new Set())
  const [teamReviewDone, setTeamReviewDone] = useState(false)
  const [projectReviewDone, setProjectReviewDone] = useState(false)
  const [isApplyingReviewedSetup, setIsApplyingReviewedSetup] = useState(false)
  const [clearChatDialogOpen, setClearChatDialogOpen] = useState(false)
  const scrollRef = useRef<HTMLDivElement>(null)
  const chatInputRef = useRef<HTMLTextAreaElement>(null)
  const stopTypingTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const chatInputFocusNonce = useUiStore((s) => s.chatInputFocusNonce)
  const [showThinkingStuck, setShowThinkingStuck] = useState(false)

  const agent =
    selectedNpcIndex !== null ? (agents.find((a) => a.index === selectedNpcIndex) ?? null) : null

  const agentHistories = useCoreStore((s) => s.agentHistories)
  const tasks = useCoreStore((s) => s.tasks)
  const chatMessages =
    selectedNpcIndex !== null
      ? (agentHistories[selectedNpcIndex] ?? EMPTY_CHAT_MESSAGES)
      : EMPTY_CHAT_MESSAGES

  const pendingRetryText = useMemo(() => pendingChatRetryText(chatMessages), [chatMessages])
  const lastVisibleHistoryIndex = useMemo(() => {
    for (let i = chatMessages.length - 1; i >= 0; i--) {
      if (!chatMessages[i]?.metadata?.internal) return i
    }
    return -1
  }, [chatMessages])
  const { canChat, reason: chatBlockedReason } = useChatAvailability(selectedNpcIndex)

  const hasVisibleMessages = chatMessages.some((msg) => !msg.metadata?.internal)

  useEffect(() => {
    if (!isChatting || selectedNpcIndex === null) return
    const idx = selectedNpcIndex
    useCoreStore.getState().markAgentChatRead(idx)
    return () => {
      useCoreStore.getState().markAgentChatRead(idx)
    }
  }, [isChatting, selectedNpcIndex, chatMessages])

  useEffect(() => {
    return () => {
      if (stopTypingTimeoutRef.current) clearTimeout(stopTypingTimeoutRef.current)
    }
  }, [])

  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight
    }
  }, [chatMessages, isThinking, isChatting])

  useEffect(() => {
    if (isChatting && scrollRef.current) {
      setTimeout(() => {
        if (scrollRef.current) scrollRef.current.scrollTop = scrollRef.current.scrollHeight
      }, 100)
    }
  }, [isChatting])

  useEffect(() => {
    if (!isChatting || chatInputFocusNonce === 0) return
    const id = window.requestAnimationFrame(() => {
      chatInputRef.current?.focus()
    })
    return () => window.cancelAnimationFrame(id)
  }, [chatInputFocusNonce, isChatting])

  useEffect(() => {
    if (!isThinking) {
      setShowThinkingStuck(false)
      return
    }
    const id = window.setTimeout(() => setShowThinkingStuck(true), CHAT_THINKING_STUCK_MS)
    return () => clearTimeout(id)
  }, [isThinking])

  const openClearChatDialog = useCallback(() => {
    if (selectedNpcIndex === null || isThinking || !hasVisibleMessages) return
    setClearChatDialogOpen(true)
  }, [selectedNpcIndex, isThinking, hasVisibleMessages])

  const handleCopyConversation = useCallback(async () => {
    if (!agent) return
    const text = formatChatTranscript(chatMessages, agent.name)
    if (!text) {
      showToast('Nothing to copy.', 'info')
      return
    }
    try {
      await navigator.clipboard.writeText(text)
      showToast('Conversation copied to clipboard.', 'success')
    } catch {
      showToast('Could not copy to clipboard.', 'error')
    }
  }, [chatMessages, agent])

  const confirmClearChat = useCallback(() => {
    if (selectedNpcIndex === null) return
    const idx = selectedNpcIndex
    useCoreStore.getState().bumpAgentHistoryClearGeneration(idx)
    useCoreStore.getState().setAgentHistory(idx, [])
    useCoreStore.getState().setAgentSummary(idx, '')
    useUiStore.getState().setThinking(false)
    setInput('')
    setClearChatDialogOpen(false)
  }, [selectedNpcIndex])

  useEffect(() => {
    setClearChatDialogOpen(false)
  }, [selectedNpcIndex])

  const handleSend = async () => {
    if (!input.trim() || isThinking) return
    if (!canChat) {
      showToast(chatBlockedReason || 'Cannot send right now.', 'error')
      return
    }
    if (stopTypingTimeoutRef.current) clearTimeout(stopTypingTimeoutRef.current)
    setIsTyping(false)

    const text = input
    setInput('')
    await scene?.sendMessage(text)
  }

  const handleRetryLastChat = useCallback(async () => {
    if (!pendingRetryText || isThinking || selectedNpcIndex === null) return
    if (!canChat) {
      showToast(chatBlockedReason || 'Cannot chat with this agent right now.', 'error')
      return
    }
    setShowThinkingStuck(false)
    await scene?.sendMessage(pendingRetryText, { replaceLastUser: true })
  }, [pendingRetryText, isThinking, selectedNpcIndex, canChat, chatBlockedReason, scene])

  const handleResetStuckThinking = useCallback(async () => {
    useUiStore.getState().setThinking(false)
    setShowThinkingStuck(false)
    if (pendingRetryText && canChat) {
      await scene?.sendMessage(pendingRetryText, { replaceLastUser: true })
    } else if (pendingRetryText && !canChat) {
      showToast(chatBlockedReason || 'Reset thinking; chat is blocked until review completes.', 'info')
    }
  }, [pendingRetryText, canChat, chatBlockedReason, scene])

  const handlePlanningFormSubmit = useCallback(
    async (historyIndex: number, spec: PlanningFormSpec, answers: PlanningFormAnswers) => {
      if (selectedNpcIndex === null) return
      const text = buildPlanningAnswersUserMessage(spec, answers)
      const hist = [...(useCoreStore.getState().agentHistories[selectedNpcIndex] ?? [])]
      const target = hist[historyIndex]
      if (!target || target.role !== 'assistant') return
      hist[historyIndex] = {
        ...target,
        metadata: {
          ...target.metadata,
          planningFormStatus: 'submitted',
          planningFormAnswers: answers,
        },
      }
      useCoreStore.getState().setAgentHistory(selectedNpcIndex, hist)
      if (scene) await scene.sendMessage(text)
    },
    [selectedNpcIndex, scene],
  )

  const switchSimulationToTeam = useCallback(
    (teamId: string, teamName: string) => {
      if (
        !window.confirm(
          `Switch the simulation to "${teamName}"? This resets the current project workspace to a fresh state for that team.`,
        )
      ) {
        return
      }
      useTeamStore.getState().setActiveTeam(teamId)
      showToast(`Active team is now "${teamName}".`, 'success')
      navigate('/')
    },
    [navigate],
  )

  const openReviewFlow = useCallback((draft: TeamProjectReviewDraft) => {
    setReviewDraft(draft)
    setTeamReviewDone(false)
    setProjectReviewDone(false)
  }, [])

  const switchSimulationToProject = useCallback(
    async (projectId: string, projectTitle: string) => {
      if (
        !window.confirm(
          `Switch to project "${projectTitle}"? This will load that workspace in the simulation.`,
        )
      ) {
        return
      }
      try {
        await switchActiveProject(projectId)
        scene?.resetScene()
        showToast(`Switched to project "${projectTitle}".`, 'success')
        navigate('/')
      } catch (e) {
        showToast(
          e instanceof Error ? e.message : 'Could not switch to the selected project.',
          'error',
        )
      }
    },
    [scene, navigate],
  )

  const closeReviewFlow = useCallback(() => {
    if (isApplyingReviewedSetup) return
    setReviewDraft(null)
    setTeamReviewDone(false)
    setProjectReviewDone(false)
  }, [isApplyingReviewedSetup])

  const closeTemplateHandshake = useCallback(() => setTemplateHandshake(null), [])

  useEffect(() => {
    const last = [...chatMessages]
      .reverse()
      .find(
        (m) =>
          m.role === 'assistant' &&
          typeof m.metadata?.savedTeamTemplateId === 'string' &&
          m.metadata.savedTeamTemplateId.length > 0,
      )
    const id = last?.metadata?.savedTeamTemplateId
    if (!id || handshakeOpenedFor.current.has(id)) return
    handshakeOpenedFor.current.add(id)
    setTemplateHandshake({
      teamId: id,
      headlineName:
        typeof last?.metadata?.savedTeamTemplateName === 'string'
          ? last.metadata.savedTeamTemplateName
          : undefined,
    })
  }, [chatMessages])

  const openTeamDraftInNewTab = useCallback((teamId: string) => {
    window.open(`/teams?focusTeam=${encodeURIComponent(teamId)}`, '_blank', 'noopener,noreferrer')
  }, [])

  const openProjectDraftInNewTab = useCallback((projectId: string) => {
    window.open(`/projects?focusProject=${encodeURIComponent(projectId)}`, '_blank', 'noopener,noreferrer')
  }, [])

  const applyReviewedSetup = useCallback(async () => {
    if (!reviewDraft || isApplyingReviewedSetup) return
    setIsApplyingReviewedSetup(true)
    try {
      await switchActiveProject(reviewDraft.projectId)
      useTeamStore.getState().setActiveTeam(reviewDraft.teamId)
      scene?.resetScene()
      showToast(
        `Now using project "${reviewDraft.projectTitle}" with team "${reviewDraft.teamName}".`,
        'success',
      )
      setReviewDraft(null)
      setTeamReviewDone(false)
      setProjectReviewDone(false)
      navigate('/')
    } catch (e) {
      showToast(
        e instanceof Error ? e.message : 'Could not switch to the reviewed project/team.',
        'error',
      )
    } finally {
      setIsApplyingReviewedSetup(false)
    }
  }, [reviewDraft, isApplyingReviewedSetup, scene, navigate])

  if (!isChatting || !agent || selectedNpcIndex === null) {
    return null
  }

  return (
    <div className="flex h-full min-w-0 w-full flex-col overflow-hidden bg-white pointer-events-auto">
      <ChatPanelClearDialog
        open={clearChatDialogOpen}
        onOpenChange={setClearChatDialogOpen}
        agentName={agent.name}
        onConfirm={confirmClearChat}
      />

      <ChatPanelHeader
        agent={agent}
        selectedNpcIndex={selectedNpcIndex}
        activeTeam={activeTeam}
        isThinking={isThinking}
        hasVisibleMessages={hasVisibleMessages}
        onCopyConversation={() => void handleCopyConversation()}
        onClearChat={openClearChatDialog}
      />

      <ChatMessageList
        scrollRef={scrollRef}
        chatMessages={chatMessages}
        selectedNpcIndex={selectedNpcIndex}
        agent={agent}
        activeTeam={activeTeam}
        tasks={tasks}
        setActiveAuditTaskId={setActiveAuditTaskId}
        setTemplateHandshake={setTemplateHandshake}
        switchSimulationToTeam={switchSimulationToTeam}
        openReviewFlow={openReviewFlow}
        switchSimulationToProject={switchSimulationToProject}
        onPlanningFormSubmit={handlePlanningFormSubmit}
        isThinking={isThinking}
        lastVisibleHistoryIndex={lastVisibleHistoryIndex}
        pendingRetryText={pendingRetryText}
        canChat={canChat}
        chatBlockedReason={chatBlockedReason}
        onRetryLastChat={handleRetryLastChat}
      />

      <ChatStuckBanner
        visible={showThinkingStuck}
        isThinking={isThinking}
        onDismissStuck={() => setShowThinkingStuck(false)}
        onResetRetry={handleResetStuckThinking}
      />

      <ChatComposer
        input={input}
        onInputChange={(e) => {
          const val = e.target.value
          setInput(val)
          if (val.length > 0) {
            setIsTyping(true)
            if (stopTypingTimeoutRef.current) clearTimeout(stopTypingTimeoutRef.current)
            stopTypingTimeoutRef.current = setTimeout(() => setIsTyping(false), 1000)
          } else {
            setIsTyping(false)
          }
        }}
        onSend={() => void handleSend()}
        isThinking={isThinking}
        canSend={canChat}
        sendBlockedReason={chatBlockedReason}
        agentColor={agent.color}
        textareaRef={chatInputRef}
      />

      <TeamTemplateReviewModal
        isOpen={templateHandshake !== null}
        teamId={templateHandshake?.teamId ?? null}
        headlineName={templateHandshake?.headlineName}
        onClose={closeTemplateHandshake}
      />

      {reviewDraft && (
        <ChatReviewFlowModal
          reviewDraft={reviewDraft}
          teamReviewDone={teamReviewDone}
          projectReviewDone={projectReviewDone}
          isApplyingReviewedSetup={isApplyingReviewedSetup}
          onClose={closeReviewFlow}
          onOpenTeamDraft={openTeamDraftInNewTab}
          onOpenProjectDraft={openProjectDraftInNewTab}
          onToggleTeamReview={() => setTeamReviewDone((v) => !v)}
          onToggleProjectReview={() => setProjectReviewDone((v) => !v)}
          onApply={applyReviewedSetup}
        />
      )}
    </div>
  )
}

export default ChatPanel
