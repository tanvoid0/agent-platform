import { ChevronDown, ChevronRight, GitPullRequest, Trash2 } from 'lucide-react'
import React, { useState } from 'react'
import { Button } from '@/components/ui/button'
import { useShallow } from 'zustand/react/shallow'
import { getAllAgents, USER_NAME } from '../../data/agents'
import { USER_COLOR, USER_COLOR_LIGHT, USER_COLOR_SOFT } from '../../theme/brand'
import { useCoreStore, type Task } from '../../integration/store/coreStore'
import { getActiveAgentSet } from '../../integration/store/teamStore'
import { useUiKanbanTaskActions } from '../../integration/store/uiSelectors'
import DeleteTaskModal from '../DeleteTaskModal'
import { Avatar } from '../components/Avatar'
import { AgentPresenceBadge } from '../components/AgentPresenceBadge'
import { formatTaskAge } from './kanbanUtils'

export function KanbanTaskCard({ task }: { task: Task }) {
  const [isExpanded, setIsExpanded] = useState(false)
  const [isDeleteModalOpen, setIsDeleteModalOpen] = useState(false)
  const { removeTask, agentsOrchestrationPaused, taskExecutionRow } = useCoreStore(
    useShallow((s) => ({
      removeTask: s.removeTask,
      agentsOrchestrationPaused: s.agentsOrchestrationPaused,
      taskExecutionRow: s.taskExecution[task.id],
    })),
  )
  const { setSelectedNpc, setActiveAuditTaskId } = useUiKanbanTaskActions()
  const system = getActiveAgentSet()

  const effectiveAgentIds = [task.assignedAgentId]

  const handleSelectAgent = (e: React.MouseEvent, agentIndex: number) => {
    e.stopPropagation()
    setSelectedNpc(agentIndex)
  }

  const borderAccent =
    task.status === 'on_hold'
      ? 'border-l-2 border-l-amber-400'
      : task.status === 'backlog'
        ? 'border-l-2 border-l-zinc-400'
        : task.status === 'review'
          ? 'border-l-2 border-l-violet-400'
          : task.status === 'scheduled' && task.requiresUserApproval
            ? 'border-l-2 border-l-indigo-400'
            : task.status === 'in_progress'
              ? 'border-l-2 border-l-sky-400'
              : task.status === 'done'
                ? 'border-l-2 border-l-emerald-400/70'
                : ''

  const canOpenReview =
    (task.status === 'scheduled' && task.requiresUserApproval) ||
    task.status === 'done' ||
    !!task.draftOutput ||
    (task.revisions && task.revisions.length > 0) ||
    task.status === 'review' ||
    task.status === 'on_hold' ||
    task.status === 'backlog'

  const stepLine =
    task.status === 'in_progress' && taskExecutionRow?.currentStep
      ? taskExecutionRow.currentStep
      : null

  return (
    <div
      className={`bg-white rounded-lg border border-black/5 shadow-sm p-3 space-y-2 group relative ${borderAccent} pl-2.5`}
    >
      <div
        className="flex items-start justify-between gap-1 cursor-pointer rounded-md -m-0.5 p-0.5 hover:bg-zinc-50/80 transition-colors"
        onClick={() => setIsExpanded(!isExpanded)}
      >
        <div className="flex-1 min-w-0 flex flex-col gap-1">
          <h3
            className="text-xs text-darkDelegation leading-snug font-bold line-clamp-2"
            title={task.title || 'Untitled task'}
          >
            {task.title || 'Untitled Task'}
          </h3>
          <div className="flex flex-wrap items-center gap-1.5">
            {task.status === 'backlog' && (
              <span className="text-[8px] font-black uppercase tracking-widest text-zinc-600 bg-zinc-100 border border-zinc-200 px-1.5 py-0.5 rounded-md">
                Backlog
              </span>
            )}
            {task.status === 'on_hold' && (
              <span className="text-[8px] font-black uppercase tracking-widest text-amber-800 bg-amber-50 border border-amber-100 px-1.5 py-0.5 rounded-md">
                Blocked
              </span>
            )}
            {task.status === 'review' && (
              <span className="text-[8px] font-black uppercase tracking-widest text-violet-700 bg-violet-50 border border-violet-100 px-1.5 py-0.5 rounded-md">
                Output review
              </span>
            )}
            {task.status === 'scheduled' && task.requiresUserApproval && (
              <span className="text-[8px] font-black uppercase tracking-widest text-indigo-600 bg-indigo-50 border border-indigo-100 px-1.5 py-0.5 rounded-md">
                Approve first
              </span>
            )}
            <span
              className="text-[9px] font-medium text-zinc-400 tabular-nums"
              title={new Date(task.updatedAt).toLocaleString()}
            >
              {formatTaskAge(task.updatedAt)}
            </span>
          </div>
          {stepLine && (
            <p className="text-[10px] text-zinc-500 leading-snug line-clamp-2" title={stepLine}>
              {stepLine}
            </p>
          )}
        </div>
        <div className="flex items-center gap-1 shrink-0">
          {task.status !== 'done' && (
            <>
              <Button
                type="button"
                variant="ghost"
                size="icon-xs"
                onClick={(e) => {
                  e.stopPropagation()
                  setIsDeleteModalOpen(true)
                }}
                className="text-zinc-300 hover:bg-red-50 hover:text-red-500"
                title="Remove task"
              >
                <Trash2 size={12} />
              </Button>
              <DeleteTaskModal
                isOpen={isDeleteModalOpen}
                onClose={() => setIsDeleteModalOpen(false)}
                onConfirm={() => removeTask(task.id)}
                taskTitle={task.title}
              />
            </>
          )}
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            className="text-zinc-300 group-hover:text-zinc-500"
            title={isExpanded ? 'Collapse details' : 'Expand description'}
            onClick={(e) => {
              e.stopPropagation()
              setIsExpanded(!isExpanded)
            }}
          >
            {isExpanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
          </Button>
        </div>
      </div>

      {isExpanded && (
        <p className="text-[11px] text-zinc-500 leading-relaxed bg-zinc-50/50 p-2 rounded border border-black/5 animate-in fade-in slide-in-from-top-1 duration-200">
          {task.description?.trim() ? task.description : 'No description for this task.'}
        </p>
      )}

      <div className="flex items-center justify-between gap-x-2 gap-y-1 pt-1 border-t border-zinc-100/80">
        <div className="flex flex-wrap gap-x-2 gap-y-1 min-w-0">
          {effectiveAgentIds.map((agentIndex) => {
            if (agentIndex === 0) {
              return (
                <span
                  key={agentIndex}
                  className="inline-flex flex-col items-start gap-0.5 text-[10px] font-bold"
                  style={{ color: USER_COLOR }}
                >
                  <span className="inline-flex items-center gap-1.5 min-w-0">
                    <Avatar type="user" color={USER_COLOR} size={18} className="shrink-0" />
                    <span className="truncate">{USER_NAME}</span>
                  </span>
                  <AgentPresenceBadge agentIndex={system.user.index} size="sm" />
                </span>
              )
            }
            const agent = getAllAgents(system).find((a) => a.index === agentIndex)
            if (!agent) return null
            const avatarType = agentIndex === system.leadAgent.index ? 'lead' : 'sub'
            return (
              <Button
                key={agentIndex}
                type="button"
                variant="ghost"
                onClick={(e) => handleSelectAgent(e, agentIndex)}
                className="-mx-1 inline-flex h-auto max-w-[11rem] flex-row items-start gap-1.5 rounded-md px-1 py-1 text-[10px] font-normal text-zinc-500 hover:bg-zinc-100 hover:text-darkDelegation"
                title={`Focus ${agent.name} in workspace`}
              >
                <Avatar type={avatarType} color={agent.color} size={18} className="shrink-0 mt-0.5" />
                <span className="flex min-w-0 flex-1 flex-col items-stretch gap-0.5">
                  <span className="truncate text-left">{agent.name}</span>
                  <AgentPresenceBadge agentIndex={agentIndex} size="sm" className="self-start" />
                </span>
              </Button>
            )
          })}
        </div>

        <div className="flex items-center gap-2 shrink-0">
          {task.status === 'in_progress' && (
            <span
              className={`inline-block text-[10px] font-black uppercase tracking-widest rounded-full px-2 py-0.5 shadow-sm border whitespace-nowrap ${
                agentsOrchestrationPaused ? 'text-amber-800 bg-amber-50 border-amber-200' : ''
              }`}
              style={
                agentsOrchestrationPaused
                  ? undefined
                  : {
                      color: USER_COLOR,
                      backgroundColor: USER_COLOR_LIGHT,
                      borderColor: USER_COLOR_SOFT,
                    }
              }
            >
              {agentsOrchestrationPaused ? 'paused' : 'working'}
            </span>
          )}

          {canOpenReview ? (
            <Button
              type="button"
              variant="ghost"
              onClick={(e) => {
                e.stopPropagation()
                setActiveAuditTaskId(task.id)
              }}
              className="group/audit flex items-center gap-1.5 rounded-lg px-2 py-1 text-zinc-400 hover:bg-emerald-50 hover:text-emerald-500"
              title={
                task.status === 'scheduled' && task.requiresUserApproval
                  ? 'Approve or edit proposed task'
                  : task.status === 'review'
                    ? 'Review output'
                    : task.status === 'on_hold'
                      ? 'View blocked task'
                      : task.status === 'backlog'
                        ? 'View task'
                        : 'View work details'
              }
            >
              {task.revisions && task.revisions.length > 0 && (
                <span className="text-[10px] font-black text-zinc-300 transition-colors group-hover/audit:text-emerald-400">
                  {task.revisions.length}
                </span>
              )}
              <GitPullRequest size={13} strokeWidth={2.5} />
            </Button>
          ) : null}
        </div>
      </div>
    </div>
  )
}
