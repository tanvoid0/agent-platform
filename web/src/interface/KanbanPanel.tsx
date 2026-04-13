import { ClipboardList, Search } from 'lucide-react'
import React, { useMemo, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useShallow } from 'zustand/react/shallow'
import { useCoreStore } from '../integration/store/coreStore'
import { KanbanTaskCard } from './kanban/KanbanTaskCard'
import { COLUMN_META, COLUMN_ORDER } from './kanban/kanbanConstants'
import {
  taskBelongsInKanbanColumn,
  taskMatchesQuery,
  taskNeedsUserAttention,
  type BoardFilter,
} from './kanban/kanbanUtils'

interface KanbanPanelProps {
  height?: number
  expanded?: boolean
}

export function KanbanPanel({ height = 320, expanded = false }: KanbanPanelProps) {
  const { tasks, phase, agentsOrchestrationPaused, approveAllAwaitingUserInput } = useCoreStore(
    useShallow((s) => ({
      tasks: s.tasks,
      phase: s.phase,
      agentsOrchestrationPaused: s.agentsOrchestrationPaused,
      approveAllAwaitingUserInput: s.approveAllAwaitingUserInput,
    })),
  )

  const [query, setQuery] = useState('')
  const [boardFilter, setBoardFilter] = useState<BoardFilter>('all')

  const filteredTasks = useMemo(() => {
    return tasks.filter((t) => {
      if (!taskMatchesQuery(t, query)) return false
      if (boardFilter === 'needs_you' && !taskNeedsUserAttention(t)) return false
      return true
    })
  }, [tasks, query, boardFilter])

  const needsYouCount = useMemo(
    () => tasks.reduce((n, t) => n + (taskNeedsUserAttention(t) ? 1 : 0), 0),
    [tasks],
  )

  const visibleTotal = filteredTasks.length

  return (
    <div
      className={`w-full bg-white border-t border-black/8 flex flex-col pointer-events-auto relative min-h-0 ${
        expanded ? 'flex-1 min-h-0' : 'shrink-0'
      }`}
      style={expanded ? undefined : { height }}
    >
      <div className="shrink-0 px-4 py-2.5 border-b border-zinc-100/90 bg-white flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between sm:gap-3">
        <div className="flex items-center gap-2 min-w-0">
          <div className="flex items-center justify-center w-8 h-8 rounded-xl bg-zinc-100 text-zinc-500 shrink-0">
            <ClipboardList size={16} strokeWidth={2.25} />
          </div>
          <div className="min-w-0">
            <div className="flex items-center gap-2 flex-wrap">
              <h2 className="text-[11px] font-black uppercase tracking-widest text-darkDelegation">
                Todo board
              </h2>
              {phase === 'working' && agentsOrchestrationPaused && (
                <span
                  className="text-[8px] font-black uppercase tracking-widest text-amber-700 bg-amber-50 border border-amber-100 px-1.5 py-0.5 rounded-md"
                  title="Agents will not start new work until you resume"
                >
                  Paused
                </span>
              )}
            </div>
            <p className="text-[10px] text-zinc-400 font-medium truncate">
              {visibleTotal === tasks.length
                ? `${tasks.length} task${tasks.length === 1 ? '' : 's'}`
                : `${visibleTotal} of ${tasks.length} shown`}
            </p>
          </div>
        </div>

        <div className="flex flex-col sm:flex-row gap-2 sm:items-center sm:justify-end flex-1 min-w-0">
          <div className="relative flex-1 min-w-0 max-w-md">
            <Search
              size={14}
              className="absolute left-2.5 top-1/2 -translate-y-1/2 text-zinc-300 pointer-events-none"
            />
            <Input
              type="search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Filter by title or description…"
              className="h-auto w-full rounded-lg border-zinc-200/90 bg-zinc-50/50 py-1.5 pr-3 pl-8 text-[11px] text-zinc-800 focus-visible:ring-darkDelegation/15"
              aria-label="Filter tasks"
            />
          </div>
          <div className="flex items-center gap-1 shrink-0">
            <Button
              type="button"
              variant="ghost"
              onClick={() => setBoardFilter('all')}
              className={`rounded-lg px-2.5 py-1.5 text-[9px] font-black uppercase tracking-widest transition-all ${
                boardFilter === 'all'
                  ? 'bg-darkDelegation text-white shadow-sm hover:bg-darkDelegation hover:text-white'
                  : 'bg-zinc-100 text-zinc-500 hover:bg-zinc-200/80'
              }`}
            >
              All
            </Button>
            <Button
              type="button"
              variant="ghost"
              onClick={() => setBoardFilter('needs_you')}
              className={`flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[9px] font-black uppercase tracking-widest transition-all ${
                boardFilter === 'needs_you'
                  ? 'bg-violet-600 text-white shadow-sm hover:bg-violet-600 hover:text-white'
                  : 'bg-zinc-100 text-zinc-500 hover:bg-zinc-200/80'
              }`}
              title="Tasks waiting on approval or review"
            >
              Needs you
              {needsYouCount > 0 && (
                <span
                  className={`tabular-nums px-1 py-px rounded text-[8px] font-black ${
                    boardFilter === 'needs_you' ? 'bg-white/20' : 'bg-violet-100 text-violet-700'
                  }`}
                >
                  {needsYouCount}
                </span>
              )}
            </Button>
            {needsYouCount > 0 && (
              <Button
                type="button"
                variant="outline"
                onClick={() => approveAllAwaitingUserInput()}
                className="rounded-lg px-2.5 py-1.5 text-[9px] font-black uppercase tracking-widest border-emerald-200/90 text-emerald-800 bg-emerald-50/80 hover:bg-emerald-100/90"
                title="Approve every proposed plan and every submission in review that is waiting on you"
              >
                Approve all
              </Button>
            )}
          </div>
        </div>
      </div>

      <div className="flex-1 min-h-0 overflow-x-auto overflow-y-hidden bg-zinc-50/30">
        <div className="flex h-full min-w-max px-4 py-3 gap-3">
          {COLUMN_ORDER.map((status) => {
            const meta = COLUMN_META[status]
            const colTasks = filteredTasks.filter((t) => taskBelongsInKanbanColumn(t, status))
            return (
              <div
                key={status}
                className={`w-56 sm:w-60 flex flex-col gap-2 rounded-2xl border p-2 min-h-0 ${meta.tint}`}
              >
                <div className="shrink-0 px-1 pt-1 pb-0.5 select-none">
                  <div className="flex items-center gap-2">
                    <span className="text-[10px] font-black uppercase tracking-widest text-zinc-500 leading-none">
                      {meta.label}
                    </span>
                    <span className="px-1.5 py-0.5 bg-white/80 text-zinc-500 text-[9px] font-bold rounded-md min-w-[1.25rem] text-center border border-black/5">
                      {colTasks.length}
                    </span>
                  </div>
                  <p className="text-[9px] text-zinc-400 font-medium leading-snug mt-1 line-clamp-2">
                    {meta.hint}
                  </p>
                </div>

                <div className="flex-1 min-h-0 flex flex-col gap-2 overflow-y-auto overflow-x-hidden pr-0.5 custom-scrollbar">
                  {colTasks.map((t) => (
                    <React.Fragment key={t.id}>
                      <KanbanTaskCard task={t} />
                    </React.Fragment>
                  ))}
                  {colTasks.length === 0 && (
                    <div className="border border-dashed border-zinc-200/80 rounded-xl p-4 flex flex-col items-center justify-center gap-1 select-none bg-white/40">
                      <span className="text-[10px] font-bold text-zinc-300 uppercase tracking-widest">
                        Nothing here
                      </span>
                      <span className="text-[9px] text-zinc-400 text-center leading-snug">
                        {query.trim() || boardFilter === 'needs_you'
                          ? 'Try clearing filters or check other columns.'
                          : meta.hint}
                      </span>
                    </div>
                  )}
                </div>
              </div>
            )
          })}
        </div>
      </div>
    </div>
  )
}
