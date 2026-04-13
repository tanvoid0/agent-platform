import { ExternalLink, FileSearch, Play, RefreshCcw, Users } from 'lucide-react'
import React from 'react'
import { Button } from '@/components/ui/button'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import type { LLMMessage, PlanningFormAnswers, PlanningFormSpec } from '../../core/llm/types'
import type { AgenticSystem, AgentNode } from '../../data/agents'
import type { Task } from '../../integration/store/coreStore'
import { USER_COLOR, USER_COLOR_LIGHT, USER_COLOR_SOFT } from '../../theme/brand'
import { CopyButton } from '../actionLog/CopyButton'
import { Avatar } from '../components/Avatar'
import { PlanningFormBlock } from '../PlanningFormBlock'
import type { TeamProjectReviewDraft } from './chatReviewTypes'

export const ChatMessageRow: React.FC<{
  msg: LLMMessage
  historyIndex: number
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
  msg,
  historyIndex,
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
}) => {
  if (msg.metadata?.internal) return null
  const reviewTaskId =
    typeof msg.metadata?.reviewTaskId === 'string' && msg.metadata.reviewTaskId.length > 0
      ? msg.metadata.reviewTaskId
      : null
  const savedTeamTemplateId =
    typeof msg.metadata?.savedTeamTemplateId === 'string' &&
    msg.metadata.savedTeamTemplateId.length > 0
      ? msg.metadata.savedTeamTemplateId
      : null
  const createdProjectId =
    typeof msg.metadata?.createdProjectId === 'string' && msg.metadata.createdProjectId.length > 0
      ? msg.metadata.createdProjectId
      : null

  return (
    <div
      className={`flex w-full min-w-0 flex-col ${msg.role === 'user' ? 'items-end' : 'items-start'}`}
    >
      <div
        className={`flex w-full min-w-0 max-w-full items-start gap-3 sm:gap-4 ${
          msg.role === 'user' ? 'flex-row-reverse' : 'flex-row'
        }`}
      >
        <div className="mt-1 shrink-0">
          {msg.role === 'assistant' ? (
            <Avatar
              type={agent?.index === activeTeam.leadAgent.index ? 'lead' : 'sub'}
              color={agent?.color}
              size={32}
            />
          ) : (
            <Avatar type="user" color={USER_COLOR} size={32} />
          )}
        </div>

        <div
          className={`flex min-w-0 max-w-full flex-1 flex-col ${
            msg.role === 'user' ? 'items-end' : 'items-start'
          }`}
        >
          <div
            className={`max-w-full min-w-0 rounded-[20px] border px-3 py-2.5 text-[14px] leading-relaxed shadow-sm sm:px-4 ${
              msg.role === 'user' ? 'rounded-tr-none' : 'rounded-tl-none'
            }`}
            style={
              msg.role === 'user'
                ? {
                    backgroundColor: USER_COLOR_LIGHT,
                    borderColor: USER_COLOR_SOFT,
                    color: '#27272a',
                  }
                : {
                    backgroundColor: '#fafafa',
                    borderColor: '#f4f4f5',
                    color: '#27272a',
                  }
            }
          >
            <div className="markdown-content min-w-0 max-w-full">
              <ReactMarkdown remarkPlugins={[remarkGfm]}>{msg.content}</ReactMarkdown>

              {msg.role === 'assistant' && reviewTaskId && (
                <div className="mt-4 p-4 bg-white/50 rounded-2xl border border-zinc-200/50 flex flex-wrap items-center justify-between gap-3 animate-in fade-in slide-in-from-bottom-2 duration-500">
                  <div className="flex items-center gap-2 pr-2">
                    <div
                      className="p-2 rounded-xl flex-shrink-0"
                      style={{ backgroundColor: USER_COLOR_LIGHT, color: USER_COLOR }}
                    >
                      <FileSearch size={18} />
                    </div>
                    <span className="text-[10px] font-black uppercase tracking-widest text-zinc-600">
                      {tasks.find((t) => t.id === reviewTaskId)?.status === 'review'
                        ? 'Review Requested'
                        : 'Review Processed'}
                    </span>
                  </div>

                  {tasks.find((t) => t.id === reviewTaskId)?.status === 'review' && (
                    <Button
                      type="button"
                      onClick={() => setActiveAuditTaskId(reviewTaskId)}
                      className="min-w-[120px] flex-1 rounded-xl bg-darkDelegation px-4 py-2 text-[9px] font-black uppercase tracking-widest text-white whitespace-nowrap shadow-sm hover:bg-black active:scale-95"
                    >
                      Review Task
                    </Button>
                  )}
                </div>
              )}

              {msg.role === 'assistant' &&
                savedTeamTemplateId && (
                  <div className="mt-4 p-4 bg-indigo-50/80 rounded-2xl border border-indigo-100 flex flex-col gap-3 animate-in fade-in slide-in-from-bottom-2 duration-500">
                    <div className="flex items-start gap-3">
                      <div className="p-2 rounded-xl shrink-0 bg-white border border-indigo-100 text-indigo-600">
                        <Users size={18} />
                      </div>
                      <div className="min-w-0 flex-1">
                        <p className="text-[10px] font-black uppercase tracking-widest text-indigo-900">
                          New team template
                        </p>
                        <p className="text-[12px] font-semibold text-zinc-800 mt-0.5 truncate">
                          {typeof msg.metadata.savedTeamTemplateName === 'string'
                            ? msg.metadata.savedTeamTemplateName
                            : 'Saved template'}
                        </p>
                        <p className="text-[11px] text-zinc-500 mt-1 leading-snug">
                          Review or edit the flow, then switch the simulation to this team when you are ready.
                        </p>
                      </div>
                    </div>
                    <div className="flex flex-wrap gap-2">
                      <Button
                        type="button"
                        variant="outline"
                        onClick={() =>
                          setTemplateHandshake({
                            teamId: savedTeamTemplateId,
                            headlineName:
                              typeof msg.metadata.savedTeamTemplateName === 'string'
                                ? msg.metadata.savedTeamTemplateName
                                : undefined,
                          })
                        }
                        className="min-w-[140px] flex-1 rounded-xl border-indigo-200 bg-white px-4 py-2.5 text-[9px] font-black uppercase tracking-widest text-indigo-800 shadow-sm hover:bg-indigo-50 active:scale-[0.99]"
                      >
                        Review &amp; edit
                      </Button>
                      <Button
                        type="button"
                        onClick={() =>
                          switchSimulationToTeam(
                            savedTeamTemplateId,
                            typeof msg.metadata.savedTeamTemplateName === 'string'
                              ? msg.metadata.savedTeamTemplateName
                              : 'Team',
                          )
                        }
                        className="flex min-w-[140px] flex-1 items-center justify-center gap-2 rounded-xl bg-darkDelegation px-4 py-2.5 text-[9px] font-black uppercase tracking-widest text-white shadow-sm hover:bg-black active:scale-[0.99]"
                      >
                        <Play size={14} className="shrink-0" />
                        Use for simulation
                      </Button>
                      {createdProjectId && (
                          <Button
                            type="button"
                            onClick={() =>
                              openReviewFlow({
                                teamId: savedTeamTemplateId,
                                teamName:
                                  typeof msg.metadata.savedTeamTemplateName === 'string'
                                    ? msg.metadata.savedTeamTemplateName
                                    : 'Team',
                                projectId: createdProjectId,
                                projectTitle:
                                  typeof msg.metadata.createdProjectTitle === 'string'
                                    ? msg.metadata.createdProjectTitle
                                    : 'Project',
                              })
                            }
                            className="min-w-[140px] w-full rounded-xl bg-indigo-600 px-4 py-2.5 text-[9px] font-black uppercase tracking-widest text-white shadow-sm hover:bg-indigo-700 active:scale-[0.99]"
                          >
                            Review team + project
                          </Button>
                        )}
                    </div>
                  </div>
                )}

              {msg.role === 'assistant' &&
                !savedTeamTemplateId &&
                createdProjectId && (
                  <div className="mt-4 p-4 bg-emerald-50/80 rounded-2xl border border-emerald-100 flex flex-col gap-3 animate-in fade-in slide-in-from-bottom-2 duration-500">
                    <div className="flex items-start gap-3">
                      <div className="p-2 rounded-xl shrink-0 bg-white border border-emerald-100 text-emerald-600">
                        <ExternalLink size={18} />
                      </div>
                      <div className="min-w-0 flex-1">
                        <p className="text-[10px] font-black uppercase tracking-widest text-emerald-900">
                          New project created
                        </p>
                        <p className="text-[12px] font-semibold text-zinc-800 mt-0.5 truncate">
                          {typeof msg.metadata.createdProjectTitle === 'string'
                            ? msg.metadata.createdProjectTitle
                            : 'New project'}
                        </p>
                        <p className="text-[11px] text-zinc-500 mt-1 leading-snug">
                          Open it in Projects or switch the simulation there now.
                        </p>
                      </div>
                    </div>
                    <div className="flex flex-wrap gap-2">
                      <Button
                        type="button"
                        variant="outline"
                        onClick={() =>
                          window.open(
                            `/projects?focusProject=${encodeURIComponent(createdProjectId)}`,
                            '_blank',
                            'noopener,noreferrer',
                          )
                        }
                        className="min-w-[140px] flex-1 rounded-xl border-emerald-200 bg-white px-4 py-2.5 text-[9px] font-black uppercase tracking-widest text-emerald-800 shadow-sm hover:bg-emerald-50 active:scale-[0.99]"
                      >
                        Review project
                      </Button>
                      <Button
                        type="button"
                        onClick={() =>
                          void switchSimulationToProject(
                            createdProjectId,
                            typeof msg.metadata.createdProjectTitle === 'string'
                              ? msg.metadata.createdProjectTitle
                              : 'Project',
                          )
                        }
                        className="flex min-w-[140px] flex-1 items-center justify-center gap-2 rounded-xl bg-darkDelegation px-4 py-2.5 text-[9px] font-black uppercase tracking-widest text-white shadow-sm hover:bg-black active:scale-[0.99]"
                      >
                        <Play size={14} className="shrink-0" />
                        Use for simulation
                      </Button>
                    </div>
                  </div>
                )}

              {msg.role === 'assistant' &&
                msg.metadata?.planningForm &&
                (msg.metadata.planningForm.fields?.length ?? 0) > 0 && (
                  <PlanningFormBlock
                    spec={msg.metadata.planningForm}
                    status={msg.metadata.planningFormStatus}
                    savedAnswers={msg.metadata.planningFormAnswers}
                    historyIndex={historyIndex}
                    disabled={isThinking}
                    onSubmit={onPlanningFormSubmit}
                  />
                )}
            </div>
          </div>

          <div
            className={`mt-2 flex flex-wrap items-center gap-1 px-1 ${
              msg.role === 'user' ? 'justify-end' : 'justify-start'
            }`}
          >
            <span className="text-[10px] font-black text-zinc-400 uppercase tracking-widest">
              {msg.role === 'user' ? 'You' : agent?.name?.split(' ')[0] || 'AI'}
            </span>
            <CopyButton text={msg.content} title="Copy this message" />
            {msg.role === 'user' &&
              historyIndex === lastVisibleHistoryIndex &&
              pendingRetryText &&
              !isThinking && (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={!canChat}
                  title={!canChat ? chatBlockedReason ?? undefined : 'Resend this message to the model'}
                  className="h-7 gap-1 rounded-lg border-zinc-200 bg-white px-2 text-[9px] font-black uppercase tracking-wider text-zinc-600 shadow-sm hover:bg-zinc-50"
                  onClick={() => void onRetryLastChat()}
                >
                  <RefreshCcw size={11} strokeWidth={2.5} />
                  Resend
                </Button>
              )}
          </div>
        </div>
      </div>
    </div>
  )
}
