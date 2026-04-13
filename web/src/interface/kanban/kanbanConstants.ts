import type { TaskStatus } from '../../integration/store/coreStore'

export const COLUMN_META: Record<
  TaskStatus,
  { label: string; hint: string; tint: string }
> = {
  backlog: {
    label: 'Backlog',
    hint: 'Captured work not yet pulled into the run queue.',
    tint: 'bg-zinc-50/80 border-zinc-100/80',
  },
  scheduled: {
    label: 'Scheduled',
    hint: 'Ready to run — no approval gate pending.',
    tint: 'bg-slate-50/90 border-slate-100/80',
  },
  on_hold: {
    label: 'On hold',
    hint: 'Blocked by a dependency, upstream task, or external reason.',
    tint: 'bg-amber-50/45 border-amber-100/80',
  },
  review: {
    label: 'Review',
    hint: 'Human gate: approve a proposed plan or sign off on output before Done.',
    tint: 'bg-violet-50/50 border-violet-100/70',
  },
  in_progress: {
    label: 'In progress',
    hint: 'Agents are executing these tasks.',
    tint: 'bg-sky-50/40 border-sky-100/70',
  },
  done: {
    label: 'Done',
    hint: 'Completed deliverables.',
    tint: 'bg-emerald-50/35 border-emerald-100/70',
  },
}

export const COLUMN_ORDER: TaskStatus[] = [
  'backlog',
  'scheduled',
  'on_hold',
  'review',
  'in_progress',
  'done',
]
