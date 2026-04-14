import type { Task, TaskStatus } from './coreStoreTypes';

export function completeTaskForApproval(task: Task, now: number): Task {
  return {
    ...task,
    status: 'done',
    output: task.draftOutput || task.output,
    revisions: task.draftOutput
      ? [...task.revisions, { output: task.draftOutput, timestamp: now }]
      : task.revisions,
    draftOutput: undefined,
    updatedAt: now,
  };
}

export function rejectTaskForRevision(task: Task, comments: string, now: number): Task {
  return {
    ...task,
    status: 'scheduled',
    reviewComments: comments,
    revisions: task.draftOutput
      ? [...task.revisions, { output: task.draftOutput, feedback: comments, timestamp: now }]
      : task.revisions,
    draftOutput: undefined,
    updatedAt: now,
  };
}

export function updateTaskStatusWithDoneGuard(
  task: Task,
  status: TaskStatus,
  now: number,
): Task | null {
  if (
    task.status === 'done' &&
    (status === 'in_progress' ||
      status === 'review' ||
      status === 'on_hold' ||
      status === 'backlog' ||
      status === 'scheduled')
  ) {
    return null;
  }
  return { ...task, status, updatedAt: now };
}
