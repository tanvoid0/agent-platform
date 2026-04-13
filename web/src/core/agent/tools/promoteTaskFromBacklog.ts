import { AgentActionContext } from '../ToolRegistry';
import { useCoreStore } from '../../../integration/store/coreStore';

/** Move a backlog card into the scheduled queue so runners can pick it up. */
export function promoteTaskFromBacklog(agent: AgentActionContext, args: { taskId: string }): boolean {
  const store = useCoreStore.getState();
  const task = store.tasks.find((t) => t.id === args.taskId);
  if (!task || task.status !== 'backlog') return false;

  store.updateTaskStatus(args.taskId, 'scheduled');
  store.addLogEntry({
    agentIndex: agent.data.index,
    action: `promoted from backlog: "${task.title}"`,
    taskId: args.taskId,
  });
  return true;
}
