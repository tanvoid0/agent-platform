import { AgentActionContext } from '../ToolRegistry';
import { useCoreStore } from '../../../integration/store/coreStore';

export function proposeTask(
  agent: AgentActionContext,
  args: { title: string; description: string; agentId: number; requiresApproval?: boolean; toBacklog?: boolean },
): boolean {
  const store = useCoreStore.getState();
  const { title, description, agentId, requiresApproval, toBacklog } = args;

  if (store.phase === 'done') {
    store.setPhase('working');
  }

  // Simplified: Trust the agentId provided by the LLM, fallback to self if invalid or 0
  const finalAgentId = agentId > 0 ? agentId : agent.data.index;

  const wantsApproval = requiresApproval || false;
  const useBacklog = Boolean(toBacklog) && !wantsApproval;

  const newTask = store.addTask({
    title,
    description,
    assignedAgentId: finalAgentId,
    status: useBacklog ? 'backlog' : 'scheduled',
    requiresUserApproval: wantsApproval,
  });

  store.addLogEntry({ agentIndex: agent.data.index, action: `proposed task: "${title}"`, taskId: newTask.id });

  return true;
}
