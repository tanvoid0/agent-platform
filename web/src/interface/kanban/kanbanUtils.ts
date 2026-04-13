import type { Task, TaskStatus } from '../../integration/store/coreStore'

export type BoardFilter = 'all' | 'needs_you'

export function formatTaskAge(updatedAt: number): string {
  const sec = Math.floor((Date.now() - updatedAt) / 1000)
  if (sec < 45) return 'just now'
  const min = Math.floor(sec / 60)
  if (min < 60) return `${min}m ago`
  const hr = Math.floor(min / 60)
  if (hr < 48) return `${hr}h ago`
  const day = Math.floor(hr / 24)
  return `${day}d ago`
}

export function taskNeedsUserAttention(task: Task): boolean {
  return (
    (task.status === 'scheduled' && task.requiresUserApproval) || task.status === 'review'
  )
}

/** Maps tasks to board columns; proposed tasks stay `scheduled` but appear under Review until approved. */
export function taskBelongsInKanbanColumn(task: Task, column: TaskStatus): boolean {
  if (column === 'scheduled') {
    return task.status === 'scheduled' && !task.requiresUserApproval
  }
  if (column === 'review') {
    return task.status === 'review' || (task.status === 'scheduled' && task.requiresUserApproval)
  }
  return task.status === column
}

export function taskMatchesQuery(task: Task, q: string): boolean {
  if (!q.trim()) return true
  const s = q.trim().toLowerCase()
  return (
    (task.title || '').toLowerCase().includes(s) ||
    (task.description || '').toLowerCase().includes(s)
  )
}
