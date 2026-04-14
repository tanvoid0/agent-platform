import { FolderOpen, Lock, MessageSquare, MessageSquareWarning, GitPullRequest } from 'lucide-react';
import React, { useCallback, useMemo, useState } from 'react';
import { Button } from '@/components/ui/button';
import { useShallow } from 'zustand/react/shallow';
import { getAllAgents, getAllCharacters } from '../data/agents';
import { countAgentsNeedingChatAttention } from '../integration/chat/agentChatAttention';
import { hasActiveOrchestrationWork } from '../integration/projectOrchestrationSync';
import { USER_COLOR, USER_COLOR_LIGHT, USER_COLOR_SOFT } from '../theme/brand';
import { useChatAvailability } from '../integration/hooks/useChatAvailability';
import { useCoreStore } from '../integration/store/coreStore';
import { useActiveTeam } from '../integration/store/teamStore';
import { useInspectorNpcUi } from '../integration/store/uiSelectors';
import { useUiStore } from '../integration/store/uiStore';
import { useSceneManager } from '../simulation/SceneContext';
import { AgentPresenceBadge } from './components/AgentPresenceBadge';
import { Avatar } from './components/Avatar';
import AgentStatusPanel from './AgentStatusPanel';
import ChatPanel from './ChatPanel';
import ProjectView from './ProjectView';
import { ProjectAgentsPanel } from './ProjectAgentsPanel';
import { ReferenceImages } from './components/ReferenceImages';
import { ProjectSideIconRail } from './projectView/ProjectSideIconRail';
import type { ProjectSideTab } from './projectView/ProjectSideTabs';
import { useActivityAttentionCount } from './projectView/useActivityAttentionCount';
import {
  persistProjectSideTab,
  readStoredProjectSideTab,
} from '@/integration/ui/projectSideTabStorage';

interface InspectorPanelProps {
  isFloating?: boolean;
  projectRailExpanded?: boolean;
  onProjectRailExpanded?: (next: boolean) => void;
}

const InspectorPanel: React.FC<InspectorPanelProps> = ({
  isFloating,
  projectRailExpanded = true,
  onProjectRailExpanded,
}) => {
  const { selectedNpcIndex, isChatting } = useInspectorNpcUi();
  const isThinking = useUiStore((s) => s.isThinking);
  const scene = useSceneManager();
  const { phase, setFinalOutputOpen, tasks, userBrief } = useCoreStore(
    useShallow((s) => ({
      phase: s.phase,
      setFinalOutputOpen: s.setFinalOutputOpen,
      tasks: s.tasks,
      userBrief: s.userBrief,
    })),
  );
  const {
    agentsOrchestrationPaused,
    isGeneratingAsset,
    agentHistories,
    chatReadVisibleLength,
    tasks: tasksList,
    taskExecution,
  } = useCoreStore(
    useShallow((s) => ({
      agentsOrchestrationPaused: s.agentsOrchestrationPaused,
      isGeneratingAsset: s.isGeneratingAsset,
      agentHistories: s.agentHistories,
      chatReadVisibleLength: s.chatReadVisibleLength,
      tasks: s.tasks,
      taskExecution: s.taskExecution,
    })),
  );
  const system = useActiveTeam();
  const agents = getAllCharacters(system);
  const rosterAgents = useMemo(() => getAllAgents(system), [system]);
  const { canChat, reason } = useChatAvailability(selectedNpcIndex);

  const agent = selectedNpcIndex !== null ? agents.find(a => a.index === selectedNpcIndex) ?? null : null;

  const { activityAttentionCount } = useActivityAttentionCount();
  const [projectSideTab, setProjectSideTab] = useState<ProjectSideTab>(() =>
    readStoredProjectSideTab(['overview', 'activity', 'agents'], 'overview'),
  );

  const orchestrationActive = useMemo(
    () =>
      hasActiveOrchestrationWork({
        phase,
        agentsOrchestrationPaused,
        tasks: tasksList,
        taskExecution,
        isGeneratingAsset,
        chatIsThinking: isThinking,
      }),
    [phase, agentsOrchestrationPaused, tasksList, taskExecution, isGeneratingAsset, isThinking],
  );

  const agentsChatAttentionCtx = useMemo(
    () => ({
      phase,
      isGeneratingAsset,
      orchestrationActive,
      leadAgentIndex: system.leadAgent.index,
      agentHistories,
      chatReadVisibleLength,
      isChatting,
      selectedNpcIndex,
    }),
    [
      phase,
      isGeneratingAsset,
      orchestrationActive,
      system.leadAgent.index,
      agentHistories,
      chatReadVisibleLength,
      isChatting,
      selectedNpcIndex,
    ],
  );

  const teamAgentIndices = useMemo(() => rosterAgents.map((a) => a.index), [rosterAgents]);
  const agentsAttentionCount = useMemo(
    () => countAgentsNeedingChatAttention(teamAgentIndices, agentsChatAttentionCtx),
    [teamAgentIndices, agentsChatAttentionCtx],
  );

  const persistProjectSideTab = useCallback(
    (tab: ProjectSideTab) => {
      if (isChatting) {
        useUiStore.getState().setChatting(false);
      }
      setProjectSideTab(tab);
      persistProjectSideTab(tab);
    },
    [isChatting],
  );

  const isProjectReady = phase === 'done' && selectedNpcIndex === system.leadAgent.index;

  const isLeadAgentIdle = selectedNpcIndex === system.leadAgent.index && phase === 'idle';
  const leadAwaitingBriefContent = isLeadAgentIdle && !String(userBrief || '').trim();
  const tasksOnHold = agent
    ? tasks.filter(
        t =>
          t.assignedAgentId === agent.index &&
          (t.status === 'review' || (t.status === 'scheduled' && t.requiresUserApproval)),
      )
    : [];
  const hasTaskOnHold = tasksOnHold.length > 0;

  const needsInput = leadAwaitingBriefContent || hasTaskOnHold;

  const handleEndChat = () => {
    useUiStore.getState().setChatting(false);
  };

  const handleStartChat = () => {
    if (canChat && selectedNpcIndex !== null) {
      scene?.startChat(selectedNpcIndex);
    }
  };

  const agentFocusChrome = agent && agent.index !== system.user.index ? (
    <>
      <div className={`shrink-0 px-4 py-3 border-b border-zinc-100 bg-white ${isFloating ? 'bg-zinc-50/50' : ''}`}>
        <div className="flex flex-col gap-4">
          <div className="flex items-center gap-4">
            <div className="shrink-0 rounded-2xl p-0.5 bg-zinc-50 border border-zinc-100/50">
              <Avatar
                type={agent.index === system.user.index ? 'user' : (agent.index === system.leadAgent.index ? 'lead' : 'sub')}
                color={agent.color}
                size={48}
              />
            </div>
            <div className="flex flex-col min-w-0">
              <h2 className="text-xl font-black text-darkDelegation leading-tight truncate">
                {agent.name}
              </h2>
              <div className="mt-1">
                <AgentPresenceBadge agentIndex={agent.index} size="sm" />
              </div>
              {agent.index !== system.user.index && (
                <div className="flex mt-1">
                  {agent.index === system.leadAgent.index ? (
                    <div
                      className="text-[9px] font-black px-1.5 py-0.5 rounded uppercase tracking-tighter border shadow-sm leading-none flex items-center h-4 shrink-0"
                      style={{
                        backgroundColor: `${agent.color}15`,
                        color: agent.color,
                        borderColor: `${agent.color}30`
                      }}
                    >
                      Lead Agent
                    </div>
                  ) : (
                    <div
                      className="text-[9px] font-black px-1.5 py-0.5 rounded uppercase tracking-tighter border shadow-sm leading-none flex items-center h-4 shrink-0"
                      style={{
                        backgroundColor: `${agent.color}15`,
                        color: agent.color,
                        borderColor: `${agent.color}30`
                      }}
                    >
                      Subagent
                    </div>
                  )}
                </div>
              )}
            </div>
          </div>

          {needsInput && !isChatting ? (
            <div className="flex flex-col gap-3 p-4 bg-zinc-50 border border-zinc-100 rounded-xl animate-in fade-in slide-in-from-top-1 shadow-sm">
              <div className="flex items-center gap-1.5 font-black uppercase tracking-widest text-[9px]">
                <div
                  className="flex items-center justify-center w-5 h-5 border rounded-lg"
                  style={{ backgroundColor: USER_COLOR_LIGHT, borderColor: USER_COLOR_SOFT, color: USER_COLOR }}
                >
                  <MessageSquareWarning size={12} strokeWidth={3} />
                </div>
                <span style={{ color: USER_COLOR }}>Review Requested</span>
              </div>
              <div className="flex flex-col gap-2">
                <p className="text-[12px] font-bold text-darkDelegation leading-tight">
                  {leadAwaitingBriefContent
                    ? "Review the user brief with the team."
                    : `I've finished the task "${tasksOnHold[0]?.title ?? 'Work'}". I've submitted my work for your review.`}
                </p>

                {leadAwaitingBriefContent && (system.outputType === 'image' || system.outputType === 'video') && (
                  <div className="mt-1 pt-3 border-t border-zinc-200/50">
                    <ReferenceImages />
                  </div>
                )}

                <Button
                  type="button"
                  onClick={leadAwaitingBriefContent ? handleStartChat : () => useUiStore.getState().setActiveAuditTaskId(tasksOnHold[0]?.id)}
                  disabled={leadAwaitingBriefContent ? !canChat : false}
                  className="mt-1 flex items-center justify-center gap-2 rounded-xl bg-darkDelegation px-4 py-3 text-[10px] font-black uppercase tracking-widest text-white shadow-sm hover:bg-black active:scale-95 disabled:opacity-50"
                >
                  {leadAwaitingBriefContent ? (
                    <>
                      <MessageSquare size={14} strokeWidth={3} />
                      Chat about the brief
                    </>
                  ) : (
                    <>
                      <GitPullRequest size={14} strokeWidth={3} />
                      Review Task
                    </>
                  )}
                </Button>
              </div>
            </div>
          ) : (
            <div className="w-full">
              {agent.index === system.user.index ? (
                null
              ) : isProjectReady ? (
                <div className="bg-yellow-50 border border-yellow-200 rounded-xl p-4 flex flex-col gap-3 shadow-sm">
                  <div className="flex items-center gap-2">
                    <div className="w-2 h-2 rounded-full bg-yellow-400 animate-pulse" />
                    <span className="text-[10px] font-black uppercase tracking-widest text-yellow-700">Project Ready</span>
                  </div>
                  <Button
                    type="button"
                    onClick={() => setFinalOutputOpen(true)}
                    className="flex w-full items-center justify-center gap-2 rounded-xl bg-yellow-400 px-4 py-2.5 text-[10px] font-black uppercase tracking-widest text-black shadow-sm hover:bg-yellow-300 active:scale-95"
                  >
                    <FolderOpen size={14} strokeWidth={3} />
                    View Final Output
                  </Button>
                </div>
              ) : isChatting ? (
                null
              ) : (
                <Button
                  type="button"
                  onClick={handleStartChat}
                  disabled={!canChat}
                  title={!canChat ? reason : undefined}
                  className={`flex h-10 w-full items-center justify-center gap-2 rounded-xl px-4 text-[10px] font-black uppercase tracking-widest transition-all active:scale-95 ${canChat
                      ? 'border-none bg-darkDelegation text-white shadow-md hover:bg-black'
                      : 'cursor-not-allowed border border-transparent bg-zinc-50 text-zinc-300 hover:bg-zinc-50'
                    }`}
                >
                  {canChat ? (
                    <>
                      <MessageSquare size={13} className="text-white" />
                      Open Chat
                    </>
                  ) : (
                    <>
                      <Lock size={12} className="opacity-40" />
                      {reason}
                    </>
                  )}
                </Button>
              )}
            </div>
          )}
        </div>
      </div>

      <div className={`min-h-0 flex-1 overflow-y-auto ${isFloating ? 'bg-white' : 'bg-zinc-50/30'}`}>
        <AgentStatusPanel agentIndex={selectedNpcIndex!} />
      </div>
    </>
  ) : null;

  const sidebarMain = isChatting ? (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
      <div className="min-h-0 flex-1 overflow-y-auto bg-white">
        <ChatPanel />
      </div>
      <div className="shrink-0 border-t border-zinc-100 bg-white p-3">
        <Button
          type="button"
          onClick={handleEndChat}
          className="flex h-10 w-full items-center justify-center gap-2 rounded-xl bg-darkDelegation px-4 text-[10px] font-black uppercase tracking-widest text-white shadow-md hover:bg-black active:scale-95"
        >
          <div className="size-1.5 animate-pulse rounded-full bg-white" />
          Close Chat
        </Button>
      </div>
    </div>
  ) : projectSideTab === 'agents' ? (
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto bg-white">
      <ProjectAgentsPanel />
      {agentFocusChrome}
    </div>
  ) : (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
      <ProjectView
        sideTab={projectSideTab}
        onSideTab={persistProjectSideTab}
        hideTabBar
      />
    </div>
  );

  return (
    <div
      className={`${
        isFloating
          ? 'w-full h-full max-h-[85vh] self-end rounded-2xl shadow-2xl border border-white/20'
          : 'h-full min-w-0 w-full border-l border-zinc-100'
      } relative z-30 flex shrink-0 flex-col overflow-hidden bg-white pointer-events-auto`}
    >
      <div className="flex h-full min-h-0 min-w-0 w-full flex-row">
        {projectRailExpanded && sidebarMain}
        {!isFloating && (
          <ProjectSideIconRail
            sideTab={projectSideTab}
            onSideTab={persistProjectSideTab}
            expanded={projectRailExpanded}
            onExpandedChange={onProjectRailExpanded ?? (() => {})}
            activityAttentionCount={activityAttentionCount}
            agentsAttentionCount={agentsAttentionCount}
            muteTabSelection={isChatting}
          />
        )}
      </div>
    </div>
  );
};

export default InspectorPanel;
