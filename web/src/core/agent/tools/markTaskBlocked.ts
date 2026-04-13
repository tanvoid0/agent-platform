import { AgentActionContext } from '../ToolRegistry';
import { useCoreStore } from '../../../integration/store/coreStore';

/** Block a task waiting on a dependency or external factor — not for human output review (use review flow). */
export function markTaskBlocked(
  agent: AgentActionContext,
  args: { taskId: string; reason?: string },
): boolean {
  const store = useCoreStore.getState();
  const task = store.tasks.find((t) => t.id === args.taskId);
  if (!task) return false;
  if (
    task.status === 'done' ||
    task.status === 'on_hold' ||
    task.status === 'review' ||
    (task.status === 'scheduled' && task.requiresUserApproval)
  ) {
    return false;
  }

  store.setTaskBlocked(args.taskId, args.reason);
  store.addLogEntry({
    agentIndex: agent.data.index,
    action: `marked task on hold (blocked): "${task.title}"`,
    taskId: args.taskId,
  });
  return true;
}
