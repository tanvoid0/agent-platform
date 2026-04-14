import React, { useMemo } from 'react'
import type { ActionLogEntry, Task, TaskExecutionState } from '../../integration/store/coreStore'

type StatsItem = {
  label: string
  value: string
  hint?: string
}

function formatDuration(ms: number): string {
  if (ms < 60_000) return `${Math.max(1, Math.round(ms / 1000))}s`
  const minutes = Math.floor(ms / 60_000)
  const hours = Math.floor(minutes / 60)
  const remMinutes = minutes % 60
  if (hours <= 0) return `${minutes}m`
  if (remMinutes <= 0) return `${hours}h`
  return `${hours}h ${remMinutes}m`
}

export const ProjectStatsSection: React.FC<{
  tasks: Task[]
  actionLog: ActionLogEntry[]
  taskExecution: Record<string, TaskExecutionState>
  className?: string
}> = ({ tasks, actionLog, taskExecution, className = 'mb-8' }) => {
  const stats = useMemo(() => {
    const totalTasks = tasks.length
    const doneTasks = tasks.filter((task) => task.status === 'done').length
    const inFlightTasks = tasks.filter(
      (task) => task.status === 'in_progress' || task.status === 'review',
    ).length
    const blockedTasks = tasks.filter((task) => task.status === 'on_hold').length
    const completionRate = totalTasks > 0 ? Math.round((doneTasks / totalTasks) * 100) : 0
    const revisionCount = tasks.reduce((sum, task) => sum + task.revisions.length, 0)
    const totalRetries = Object.values(taskExecution).reduce(
      (sum, row) => sum + Math.max(0, row.attempts - 1),
      0,
    )

    const timelinePoints = [
      ...tasks.flatMap((task) => [task.createdAt, task.updatedAt]),
      ...actionLog.map((entry) => entry.timestamp),
    ]
      .filter((ts) => Number.isFinite(ts) && ts > 0)
      .sort((a, b) => a - b)

    const activeSpanMs =
      timelinePoints.length >= 2 ? timelinePoints[timelinePoints.length - 1] - timelinePoints[0] : 0
    const activityLabel =
      timelinePoints.length >= 2 ? formatDuration(activeSpanMs) : timelinePoints.length === 1 ? '<1m' : 'n/a'

    const items: StatsItem[] = [
      { label: 'Tasks done', value: `${doneTasks}/${totalTasks}`, hint: `${completionRate}% complete` },
      { label: 'In progress', value: String(inFlightTasks), hint: `${blockedTasks} blocked` },
      { label: 'Activity events', value: String(actionLog.length), hint: 'Action log entries' },
      { label: 'Revisions', value: String(revisionCount), hint: `${totalRetries} retries across runs` },
      { label: 'Active span', value: activityLabel, hint: 'From first to latest activity' },
    ]
    return items
  }, [tasks, actionLog, taskExecution])

  return (
    <section className={className}>
      <div className="mb-3 flex items-center gap-2">
        <p className="text-[10px] font-black uppercase tracking-widest text-zinc-400">Project stats</p>
        <div className="h-px flex-1 bg-zinc-100" />
      </div>
      <div className="grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-5">
        {stats.map((item) => (
          <article key={item.label} className="rounded-xl border border-zinc-100 bg-zinc-50/80 px-3 py-2.5">
            <p className="text-[9px] font-black uppercase tracking-widest text-zinc-400">{item.label}</p>
            <p className="mt-1 text-base font-black text-darkDelegation tabular-nums">{item.value}</p>
            {item.hint ? (
              <p className="mt-0.5 truncate text-[9px] font-medium text-zinc-500" title={item.hint}>
                {item.hint}
              </p>
            ) : null}
          </article>
        ))}
      </div>
    </section>
  )
}
