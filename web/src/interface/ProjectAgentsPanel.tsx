import { MessageSquare, Zap } from 'lucide-react';
import React, { useEffect, useMemo, useState } from 'react';
import { Button } from '@/components/ui/button';
import { getAllCharacters } from '../data/agents';
import { agentNeedsChatAttention } from '../integration/chat/agentChatAttention';
import { useDelegationConnectivity } from '../integration/hooks/useDelegationConnectivity';
import { hasActiveOrchestrationWork } from '../integration/projectOrchestrationSync';
import { useCoreStore } from '../integration/store/coreStore';
import { useActiveTeam } from '../integration/store/teamStore';
import { useUiStore } from '../integration/store/uiStore';
import { useSceneManager } from '../simulation/SceneContext';
import { AgentPresenceBadge } from './components/AgentPresenceBadge';

export function ProjectAgentsPanel() {
  const agentsOrchestrationPaused = useCoreStore((s) => s.agentsOrchestrationPaused);
  const tasks = useCoreStore((s) => s.tasks);
  const isGeneratingAsset = useCoreStore((s) => s.isGeneratingAsset);
  const taskExecution = useCoreStore((s) => s.taskExecution);
  const setAgentsOrchestrationPaused = useCoreStore((s) => s.setAgentsOrchestrationPaused);
  const phase = useCoreStore((s) => s.phase);
  const agentHistories = useCoreStore((s) => s.agentHistories);
  const chatReadVisibleLength = useCoreStore((s) => s.chatReadVisibleLength);
  const activeTeam = useActiveTeam();
  const scene = useSceneManager();
  const selectedNpcIndex = useUiStore((s) => s.selectedNpcIndex);
  const isChatting = useUiStore((s) => s.isChatting);
  const isThinking = useUiStore((s) => s.isThinking);
  const setSelectedNpc = useUiStore((s) => s.setSelectedNpc);
  const { backendBlocksChat, backendReason, projectsDown } = useDelegationConnectivity();

  const orchestrationActive = useMemo(
    () =>
      hasActiveOrchestrationWork({
        phase,
        agentsOrchestrationPaused,
        tasks,
        taskExecution,
        isGeneratingAsset,
        chatIsThinking: isThinking,
      }),
    [phase, agentsOrchestrationPaused, tasks, taskExecution, isGeneratingAsset, isThinking],
  );

  const [nowTs, setNowTs] = useState(() => Date.now());
  const hasActiveRuns = useMemo(
    () => Object.values(taskExecution).some((run) => run.status === 'running' || run.status === 'retry_queued'),
    [taskExecution],
  );

  useEffect(() => {
    if (!hasActiveRuns) return;
    const id = window.setInterval(() => setNowTs(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [hasActiveRuns]);

  const characters = useMemo(() => getAllCharacters(activeTeam), [activeTeam]);
  const teamAgents = useMemo(
    () => characters.filter((c) => c.index !== activeTeam.user.index),
    [characters, activeTeam.user.index],
  );

  const chatAttentionCtx = useMemo(
    () => ({
      phase,
      isGeneratingAsset,
      orchestrationActive,
      leadAgentIndex: activeTeam.leadAgent.index,
      agentHistories,
      chatReadVisibleLength,
      isChatting,
      selectedNpcIndex,
    }),
    [
      phase,
      isGeneratingAsset,
      orchestrationActive,
      activeTeam.leadAgent.index,
      agentHistories,
      chatReadVisibleLength,
      isChatting,
      selectedNpcIndex,
    ],
  );

  const tasksByAgent = useMemo(() => {
    const byAgent = new Map<number, typeof tasks>();
    for (const task of tasks) {
      if (task.status === 'done') continue;
      const existing = byAgent.get(task.assignedAgentId);
      if (existing) {
        existing.push(task);
      } else {
        byAgent.set(task.assignedAgentId, [task]);
      }
    }
    return byAgent;
  }, [tasks]);

  const agentStatusById = useMemo(() => {
    const runs = Object.values(taskExecution);
    const map: Record<
      number,
      {
        label: string;
        labelClass: string;
        heartbeatLabel: string;
        heartbeatTitle: string;
        showHeartbeat: boolean;
        taskCountLabel: string;
        quickActionTaskId?: string;
        quickActionLabel?: string;
        quickActionTitle?: string;
        quickActionDisabled: boolean;
        currentStep?: string;
      }
    > = {};

    for (const agent of teamAgents) {
      const agentTasks = tasksByAgent.get(agent.index) ?? [];
      const backlogCount = agentTasks.filter((t) => t.status === 'backlog').length;
      const queuedCount = agentTasks.filter((t) => t.status === 'scheduled' && !t.requiresUserApproval).length;
      const blockedCount = agentTasks.filter((t) => t.status === 'on_hold').length;
      const inProgressCount = agentTasks.filter((t) => t.status === 'in_progress').length;
      const reviewCount = agentTasks.filter((t) => t.status === 'review').length;
      const needsApprovalCount = agentTasks.filter((t) => t.status === 'scheduled' && t.requiresUserApproval).length;
      const activeRuns = runs.filter((run) => run.agentIndex === agent.index).sort((a, b) => b.updatedAt - a.updatedAt);
      const latestRun = activeRuns[0];
      const staleMs = latestRun ? nowTs - latestRun.lastHeartbeatAt : null;
      const isStalled = latestRun?.status === 'running' && staleMs !== null && staleMs > 25000;
      const retryCandidate = activeRuns.find((run) => run.status === 'failed' || run.status === 'retry_queued') ?? (isStalled ? latestRun : undefined);
      const nudgeCandidateTask =
        agentTasks.find((t) => t.status === 'in_progress') ??
        agentTasks.find((t) => t.status === 'scheduled' && !t.requiresUserApproval);
      const heartbeatSeconds = Math.max(0, Math.floor((staleMs ?? 0) / 1000));
      const hasLiveExecution = latestRun?.status === 'running' || latestRun?.status === 'retry_queued';
      const heartbeatLabel = latestRun
        ? hasLiveExecution
          ? `HB ${heartbeatSeconds}s`
          : `Last ${heartbeatSeconds}s`
        : inProgressCount > 0
          ? 'No pulse'
          : 'No run';
      const heartbeatTitle = latestRun
        ? 'Execution heartbeat for the task runner (not network connectivity)'
        : inProgressCount > 0
          ? 'Task is in progress but no active execution heartbeat has been reported yet'
          : 'No task execution is currently running for this agent';

      let label = 'Idle';
      let labelClass = 'bg-zinc-100 text-zinc-500 border-zinc-200';
      if (agentsOrchestrationPaused && inProgressCount > 0) {
        label = 'Paused';
        labelClass = 'bg-amber-50 text-amber-700 border-amber-200';
      } else if (latestRun?.status === 'failed') {
        label = 'Failed';
        labelClass = 'bg-red-50 text-red-600 border-red-100';
      } else if (latestRun?.status === 'retry_queued') {
        label = 'Retry queued';
        labelClass = 'bg-amber-50 text-amber-700 border-amber-100';
      } else if (isStalled) {
        label = 'Stalled';
        labelClass = 'bg-orange-50 text-orange-700 border-orange-100';
      } else if (latestRun?.status === 'running' || inProgressCount > 0) {
        label = 'Working';
        labelClass = 'bg-blue-50 text-blue-600 border-blue-100';
      } else if (needsApprovalCount > 0) {
        label = 'Needs input';
        labelClass = 'bg-indigo-50 text-indigo-600 border-indigo-100';
      } else if (reviewCount > 0) {
        label = 'Waiting review';
        labelClass = 'bg-violet-50 text-violet-600 border-violet-100';
      } else if (blockedCount > 0) {
        label = 'Blocked';
        labelClass = 'bg-amber-50 text-amber-800 border-amber-100';
      } else if (backlogCount > 0) {
        label = 'Backlog';
        labelClass = 'bg-zinc-100 text-zinc-600 border-zinc-200';
      } else if (queuedCount > 0) {
        label = 'Queued';
        labelClass = 'bg-zinc-100 text-zinc-600 border-zinc-200';
      }

      const taskStatusTakesPriority =
        (agentsOrchestrationPaused && inProgressCount > 0) ||
        latestRun?.status === 'failed' ||
        latestRun?.status === 'retry_queued' ||
        isStalled ||
        latestRun?.status === 'running' ||
        inProgressCount > 0 ||
        needsApprovalCount > 0 ||
        reviewCount > 0 ||
        blockedCount > 0 ||
        backlogCount > 0;

      const chatOpenWithThisAgent = isChatting && selectedNpcIndex === agent.index;
      if (chatOpenWithThisAgent && !taskStatusTakesPriority) {
        if (isThinking) {
          label = 'Responding';
          labelClass = 'bg-amber-50 text-amber-700 border-amber-100';
        } else {
          label = 'In chat';
          labelClass = 'bg-emerald-50 text-emerald-700 border-emerald-100';
        }
      }

      let heartbeatLabelFinal = heartbeatLabel;
      let heartbeatTitleFinal = heartbeatTitle;
      if (chatOpenWithThisAgent && !taskStatusTakesPriority) {
        heartbeatLabelFinal = isThinking ? 'Writing reply' : 'Live session';
        heartbeatTitleFinal = isThinking
          ? 'The agent is generating a chat reply'
          : 'You have an open chat with this agent (no board task run active)';
      }

      map[agent.index] = {
        label,
        labelClass,
        heartbeatLabel: heartbeatLabelFinal,
        heartbeatTitle: heartbeatTitleFinal,
        showHeartbeat: phase !== 'done',
        taskCountLabel: `${agentTasks.length} task${agentTasks.length === 1 ? '' : 's'}`,
        quickActionTaskId: retryCandidate?.taskId ?? nudgeCandidateTask?.id,
        quickActionLabel:
          agentsOrchestrationPaused && phase === 'working'
            ? 'Resume'
            : retryCandidate
              ? 'Retry'
              : nudgeCandidateTask
                ? 'Nudge'
                : undefined,
        quickActionTitle:
          agentsOrchestrationPaused && phase === 'working'
            ? 'Resume orchestration'
            : retryCandidate
              ? 'Retry latest stuck task'
              : nudgeCandidateTask
                ? 'Nudge this agent to pick up queued work'
                : undefined,
        quickActionDisabled: !(
          (agentsOrchestrationPaused && phase === 'working') ||
          !!retryCandidate ||
          !!nudgeCandidateTask
        ),
        currentStep: latestRun?.currentStep || undefined,
      };
    }

    return map;
  }, [
    tasksByAgent,
    taskExecution,
    teamAgents,
    nowTs,
    agentsOrchestrationPaused,
    phase,
    isChatting,
    isThinking,
    selectedNpcIndex,
  ]);

  return (
    <div className="space-y-4">
      <div className="flex items-center gap-2">
        <p className="text-[10px] font-black uppercase tracking-widest text-zinc-400">Agents</p>
        <div className="h-px flex-1 bg-zinc-100" />
        <span className="text-[9px] font-mono font-bold text-zinc-400 tabular-nums">{teamAgents.length}</span>
      </div>
      <ul className="flex flex-col gap-1.5">
        {teamAgents.map((agent) => {
          const isSelected = selectedNpcIndex === agent.index;
          /** Only hard-block opening chat when we cannot sync projects or a deliverable is in flight — not when the LLM path alone is unhealthy (user can read history; sending is gated in chat). */
          const chatOpenLocked = isGeneratingAsset || projectsDown;
          const chatOpenLockReason = isGeneratingAsset
            ? 'Unavailable while a deliverable is being generated.'
            : projectsDown
              ? 'Agent Platform API unreachable — check the server and network.'
              : '';
          const statusMeta = agentStatusById[agent.index];
          const canQuickAct = Boolean(statusMeta && !statusMeta.quickActionDisabled);
          const quickLocked = !canQuickAct || isGeneratingAsset || backendBlocksChat;
          const quickLockReason = backendBlocksChat
            ? backendReason
            : isGeneratingAsset
              ? 'Unavailable while a deliverable is being generated.'
              : !canQuickAct
                ? 'No quick action for this agent — nothing is queued, stuck, or failed on their tasks. Use Nudge when a run is stalled or Retry after a failure; Resume appears when orchestration is paused.'
                : '';
          const quickEnabledTitle =
            statusMeta?.quickActionTitle ||
            statusMeta?.quickActionLabel ||
            'Run the quick action for this agent (nudge, retry, or resume).';
          const agentBusyForChatHint =
            statusMeta &&
            (statusMeta.label === 'Working' ||
              statusMeta.label === 'Stalled' ||
              statusMeta.label === 'Failed' ||
              statusMeta.label === 'Retry queued' ||
              statusMeta.label === 'Paused' ||
              statusMeta.label === 'Responding');
          const chatEnabledTitle = chatOpenLocked
            ? undefined
            : backendBlocksChat
              ? `${backendReason} You can still open chat to read history; sending may fail until the backend is healthy.`
              : agentBusyForChatHint
                ? 'Chat while this agent has board work — messages are handled when the agent can reply; task execution continues.'
                : undefined;
          const showChatAttentionBadge = agentNeedsChatAttention(agent.index, chatAttentionCtx);
          return (
            <li key={`agent-chat-${agent.index}`}>
              <div
                className={`w-full rounded-xl border p-3 transition-all flex flex-col gap-2.5 min-w-0 ${
                  isSelected
                    ? 'border-darkDelegation/25 bg-zinc-50 shadow-sm ring-1 ring-darkDelegation/10'
                    : 'border-zinc-100 bg-white/80'
                }`}
              >
                <div className="flex items-start justify-between gap-2 min-w-0">
                  <div className="flex gap-2 min-w-0 flex-1">
                    <span
                      className="h-2 w-2 shrink-0 rounded-full mt-0.5"
                      style={{ backgroundColor: agent.color }}
                      aria-hidden
                    />
                    <span
                      className="text-[11px] font-black uppercase tracking-tight text-darkDelegation leading-snug break-words [overflow-wrap:anywhere]"
                      title={agent.name}
                    >
                      {agent.name}
                    </span>
                  </div>
                  <AgentPresenceBadge agentIndex={agent.index} size="sm" compact className="shrink-0 mt-0.5" />
                </div>

                {statusMeta && (
                  <>
                    <div className="flex flex-wrap items-center gap-1.5 gap-y-1 min-w-0">
                      <span
                        className={`px-2 py-0.5 rounded-md border text-[9px] font-black uppercase tracking-wide ${statusMeta.labelClass}`}
                      >
                        {statusMeta.label}
                      </span>
                      <span className="text-[10px] font-semibold text-zinc-400 tabular-nums shrink-0">
                        {statusMeta.taskCountLabel}
                      </span>
                    </div>
                    {statusMeta.showHeartbeat && (
                      <p
                        className="text-[10px] text-zinc-500 leading-snug font-medium"
                        title={statusMeta.heartbeatTitle}
                      >
                        {statusMeta.heartbeatLabel}
                      </p>
                    )}
                    {statusMeta.currentStep && (
                      <p
                        className="text-[10px] text-zinc-600 leading-snug line-clamp-2"
                        title={statusMeta.currentStep}
                      >
                        {statusMeta.currentStep}
                      </p>
                    )}
                  </>
                )}

                <div className="grid grid-cols-2 gap-2 w-full min-w-0">
                  <span
                    className="min-w-0 block"
                    title={quickLocked ? quickLockReason : quickEnabledTitle}
                  >
                    <Button
                      type="button"
                      variant="secondary"
                      disabled={quickLocked}
                      onClick={() => {
                        if (!statusMeta) return;
                        if (agentsOrchestrationPaused && phase === 'working') {
                          setAgentsOrchestrationPaused(false);
                        }
                        if (statusMeta.quickActionTaskId) {
                          scene?.retryTaskExecution(statusMeta.quickActionTaskId);
                        }
                      }}
                      className={`w-full flex items-center justify-center gap-1.5 min-h-9 rounded-lg px-2 py-2 text-[9px] font-black uppercase tracking-widest active:scale-[0.98] min-w-0 ${
                        quickLocked
                          ? 'cursor-not-allowed bg-zinc-100 text-zinc-300'
                          : 'bg-zinc-100 text-zinc-600 hover:bg-zinc-200'
                      }`}
                    >
                      <Zap size={12} strokeWidth={2.5} className="shrink-0" />
                      <span className="truncate">{statusMeta?.quickActionLabel || 'Quick'}</span>
                    </Button>
                  </span>
                  <span
                    className="min-w-0 block"
                    title={chatOpenLocked ? chatOpenLockReason : chatEnabledTitle}
                  >
                    <Button
                      type="button"
                      disabled={chatOpenLocked}
                      onClick={() => {
                        setSelectedNpc(agent.index);
                        scene?.startChat(agent.index);
                      }}
                      className={`relative w-full flex items-center justify-center gap-1.5 overflow-visible min-h-9 rounded-lg px-2 py-2 text-[9px] font-black uppercase tracking-widest active:scale-[0.98] min-w-0 ${
                        chatOpenLocked
                          ? 'cursor-not-allowed bg-zinc-100 text-zinc-400'
                          : 'bg-darkDelegation text-white hover:bg-black'
                      }`}
                    >
                      <MessageSquare size={12} strokeWidth={2.5} className="shrink-0" />
                      <span className="truncate">{isSelected ? 'Open' : 'Chat'}</span>
                      {showChatAttentionBadge && (
                        <span
                          className="pointer-events-none absolute -right-1 -top-1 flex h-4 min-w-4 items-center justify-center rounded-full bg-rose-500 px-1 text-[8px] font-black tabular-nums leading-none text-white shadow-sm ring-2 ring-white"
                          aria-hidden
                        >
                          1
                        </span>
                      )}
                    </Button>
                  </span>
                </div>
              </div>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
