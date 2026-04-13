import { CONSULTANT_WORKSHOP_TEAM_ID, getAgentSet } from '../data/agents';
import { useTeamStore } from './store/teamStore';
import { useUiStore } from './store/uiStore';

/** Placeholder `userTitle` for delegation when starting from consultant chat; refine via the Consultant or rename in the list. */
export function consultantFirstProjectPlaceholderTitle(): string {
  const now = new Date();
  return `New project (${now.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' })})`;
}

/**
 * After creating a new project, switch to Consultant Workshop, select the lead, show the chat tab,
 * and bump `consultantChatKick` so {@link SimulationView} can run `startChat` when the scene exists.
 */
export function armConsultantWorkshopAfterNewProject(): void {
  const { customSystems } = useTeamStore.getState();
  const system = getAgentSet(CONSULTANT_WORKSHOP_TEAM_ID, customSystems);
  useTeamStore.getState().setActiveTeam(CONSULTANT_WORKSHOP_TEAM_ID);
  useUiStore.getState().setSelectedNpc(system.leadAgent.index);
  useUiStore.getState().setInspectorTab('chat');
  useUiStore.getState().bumpConsultantChatKick();
}
