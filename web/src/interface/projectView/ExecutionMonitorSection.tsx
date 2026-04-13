import { Activity, AlertTriangle, RotateCcw } from 'lucide-react'
import React from 'react'
import { Button } from '@/components/ui/button'
import type { Task, TaskExecutionState } from '../../integration/store/coreStore'
import { TASK_EXECUTION_STALE_MS } from '../../integration/store/coreStoreTypes'

export const ExecutionMonitorSection: React.FC<{
  executionRows: Array<{ task: Task; run: TaskExecutionState }>
  nowTs: number
  onRetryTask: (taskId: string) => void
  onRetryAllFailedStalled?: () => void
}> = ({ executionRows, nowTs, onRetryTask, onRetryAllFailedStalled }) => {
  const bulkRetryCount = executionRows.filter(({ run }) => {
    const staleMs = nowTs - run.lastHeartbeatAt
    const stalled = run.status === 'running' && staleMs > TASK_EXECUTION_STALE_MS
    return run.status === 'failed' || stalled
  }).length

  return (
  <div className="mb-6">
    <div className="flex items-center gap-2 mb-4">
      <p className="text-[10px] font-black uppercase tracking-widest text-zinc-400">Execution Monitor</p>
      <div className="h-px flex-1 bg-zinc-100" />
      {bulkRetryCount >= 1 && typeof onRetryAllFailedStalled === 'function' && (
        <Button
          type="button"
          variant="outline"
          onClick={() => onRetryAllFailedStalled()}
          className="h-7 rounded-lg border-zinc-200 px-2 text-[8px] font-black uppercase tracking-widest text-zinc-700 hover:bg-zinc-50"
        >
          Requeue all ({bulkRetryCount})
        </Button>
      )}
      <Activity size={12} className="text-zinc-400" />
    </div>
    <div className="space-y-1.5">
      {executionRows.map(({ task, run }) => {
        const staleMs = nowTs - run.lastHeartbeatAt
        const isStuck = run.status === 'running' && staleMs > TASK_EXECUTION_STALE_MS
        const canRetry = run.status === 'failed' || isStuck
        const badgeClass =
          run.status === 'failed'
            ? 'bg-red-50 text-red-600 border-red-100'
            : run.status === 'retry_queued'
              ? 'bg-amber-50 text-amber-600 border-amber-100'
              : isStuck
                ? 'bg-orange-50 text-orange-700 border-orange-100'
                : run.status === 'succeeded'
                  ? 'bg-emerald-50 text-emerald-600 border-emerald-100'
                  : 'bg-blue-50 text-blue-600 border-blue-100'
        const badgeLabel =
          run.status === 'failed'
            ? 'Failed'
            : run.status === 'retry_queued'
              ? 'Retry queued'
              : isStuck
                ? 'Stalled'
                : run.status === 'succeeded'
                  ? 'Done'
                  : 'Running'
        return (
          <div key={task.id} className="rounded-lg border border-zinc-100 bg-white/90 p-2.5">
            <div className="flex items-start justify-between gap-3">
              <div className="min-w-0">
                <p className="text-[11px] font-black text-darkDelegation uppercase tracking-tight truncate">
                  {task.title}
                </p>
                <p className="text-[10px] text-zinc-500 mt-0.5">{run.currentStep}</p>
                {run.status !== 'succeeded' && (
                  <p className="text-[9px] text-zinc-400 mt-0.5">
                    HB {Math.max(0, Math.floor(staleMs / 1000))}s • Attempt {run.attempts}
                  </p>
                )}
                {run.lastError && (
                  <p className="text-[10px] text-red-500 mt-2 line-clamp-2 flex items-start gap-1">
                    <AlertTriangle size={12} className="mt-0.5 shrink-0" />
                    <span>{run.lastError}</span>
                  </p>
                )}
              </div>
              <div className="flex flex-col items-end gap-2 shrink-0">
                <span
                  className={`px-2 py-1 rounded-lg border text-[9px] font-black uppercase tracking-widest ${badgeClass}`}
                >
                  {badgeLabel}
                </span>
                {canRetry && (
                  <Button
                    type="button"
                    onClick={() => onRetryTask(task.id)}
                    className="flex items-center gap-1 rounded-lg bg-darkDelegation px-2 py-1.5 text-[8px] font-black uppercase tracking-widest text-white hover:bg-black"
                  >
                    <RotateCcw size={10} strokeWidth={3} />
                    Retry
                  </Button>
                )}
              </div>
            </div>
          </div>
        )
      })}
    </div>
  </div>
  )
}
