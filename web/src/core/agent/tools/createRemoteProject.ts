import { CONSULTANT_WORKSHOP_TEAM_ID } from '../../../data/agents';
import { hasRemoteProjectBackend } from '../../../integration/api/projectRemoteApi';
import { createAndSwitchToNewProject } from '../../../integration/projectPersistence';
import { useCoreStore } from '../../../integration/store/coreStore';
import { useTeamStore } from '../../../integration/store/teamStore';
import type { AgentActionContext } from '../ToolRegistry';

function normalizeTitle(raw: string): string {
  const t = raw.trim();
  return t.length > 120 ? `${t.slice(0, 117)}...` : t;
}

/**
 * Consultant-only: create a new server-backed project and switch to it (idle phase only).
 */
export async function createRemoteProjectFromTool(
  agent: AgentActionContext,
  args: { userTitle: string },
): Promise<false | { projectId: string; projectTitle: string }> {
  const store = useCoreStore.getState();
  if (agent.data.index !== 1) {
    console.warn('[ToolRegistry] create_project: only the lead agent may call this.');
    return false;
  }
  if (store.phase !== 'idle') return false;
  if (useTeamStore.getState().selectedAgentSetId !== CONSULTANT_WORKSHOP_TEAM_ID) return false;
  if (!hasRemoteProjectBackend()) return false;

  const userTitle = typeof args.userTitle === 'string' ? normalizeTitle(args.userTitle) : '';
  if (!userTitle) return false;

  try {
    const projectId = await createAndSwitchToNewProject(userTitle);
    store.addLogEntry({
      agentIndex: agent.data.index,
      action: `created project "${userTitle}"`,
      taskId: undefined,
    });
    return { projectId, projectTitle: userTitle };
  } catch (e) {
    console.warn('[ToolRegistry] create_project', e);
    return false;
  }
}
