import { AgentActionContext } from '../ToolRegistry';
import { useCoreStore } from '../../../integration/store/coreStore';
import { useTeamStore } from '../../../integration/store/teamStore';
import { AGENTIC_SETS } from '../../../data/agents';
import { getMediaReadiness } from '../../llm/llmFacade';
import { useLlmSessionStore } from '../../../integration/store/llmSessionStore';

export async function deliverProject(agent: AgentActionContext, args: { output: string }): Promise<boolean> {
  const store = useCoreStore.getState();
  const { output } = args;

  if (store.phase !== 'working' && store.phase !== 'done') return false;

  const pendingTasks = store.tasks.filter(
    (t) => t.status === 'in_progress' || t.status === 'review' || t.status === 'on_hold',
  );
  if (pendingTasks.length > 0) {
    const names = pendingTasks.map((t) => t.title).join(', ');
    agent.appendHistory({
      role: 'user',
      content: `[SYSTEM] You cannot deliver the project yet. There are pending tasks: ${names}. All subagents must finish their work first.`,
      metadata: { internal: true },
    });
    return false;
  }

  const teamId = useTeamStore.getState().selectedAgentSetId;
  const activeTeam =
    useTeamStore.getState().customSystems.find((s) => s.id === teamId) || AGENTIC_SETS.find((s) => s.id === teamId);
  const isMultimodal = activeTeam?.outputType && activeTeam.outputType !== 'text';

  if (isMultimodal) {
    const media = getMediaReadiness(
      activeTeam.outputType as 'image' | 'music' | 'video',
      useLlmSessionStore.getState().llmConfig.apiKey,
    );
    if (media.ready) {
      store.setIsGeneratingAsset(true);
    }
  } else {
    store.setFinalOutput(output);
    store.setPhase('done');
    store.setFinalOutputOpen(true);
  }

  store.tasks
    .filter((t) => t.assignedAgentId === agent.data.index && t.status === 'in_progress')
    .forEach((t) => store.updateTaskStatus(t.id, 'done'));

  store.addLogEntry({ agentIndex: agent.data.index, action: 'delivered final project results', taskId: undefined });

  return true;
}
