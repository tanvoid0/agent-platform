import { useCoreStore } from '../../../integration/store/coreStore';
import type { AgentActionContext } from '../ToolRegistry';

export interface ReviewTaskSubmissionArgs {
  taskId: string;
  decision: 'approve' | 'request_changes';
  feedback?: string;
}

/**
 * Agent-to-agent review gate for tasks in `review`.
 * Allows lead/manager agents to approve a teammate's draft or request revisions.
 */
export function reviewTaskSubmission(
  reviewer: AgentActionContext,
  args: ReviewTaskSubmissionArgs,
): boolean {
  const store = useCoreStore.getState();
  const task = store.tasks.find((t) => t.id === args.taskId);
  if (!task) return false;
  if (task.status !== 'review') return false;

  // Keep peer review semantics: the assignee should not self-approve.
  if (task.assignedAgentId === reviewer.data.index) return false;

  if (args.decision === 'approve') {
    store.approveTask(task.id);
    store.addLogEntry({
      agentIndex: reviewer.data.index,
      action: `approved task submission`,
      taskId: task.id,
    });
    return true;
  }

  const feedback = typeof args.feedback === 'string' ? args.feedback.trim() : '';
  if (!feedback) return false;
  store.rejectTask(task.id, feedback);
  store.addLogEntry({
    agentIndex: reviewer.data.index,
    action: `requested changes on task submission`,
    taskId: task.id,
  });
  return true;
}
