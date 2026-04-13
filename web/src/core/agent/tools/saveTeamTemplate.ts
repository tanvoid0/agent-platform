import type { AgentNode, AgenticSystem, OutputType } from '../../../data/agents';
import { CONSULTANT_WORKSHOP_TEAM_ID, DEFAULT_AGENT_CHAT_MODEL, MAX_AGENTS } from '../../../data/agents';
import { DEFAULT_MODELS } from '../../llm/constants';
import { useCoreStore } from '../../../integration/store/coreStore';
import { useTeamStore } from '../../../integration/store/teamStore';
import type { AgentActionContext } from '../ToolRegistry';

export type SavedTeamTemplateRef = { teamId: string; teamName: string };

export interface TeamTemplateSubagent {
  name: string;
  description: string;
  subagents?: TeamTemplateSubagent[];
}

export interface SaveTeamTemplateArgs {
  teamName: string;
  teamType: string;
  teamDescription: string;
  outputType: OutputType;
  outputModel?: string;
  color?: string;
  leadName: string;
  leadDescription: string;
  subagents?: TeamTemplateSubagent[];
}

function countSubtreeAgents(subs: TeamTemplateSubagent[] | undefined): number {
  if (!subs?.length) return 0;
  let n = 0;
  for (const s of subs) {
    n += 1 + countSubtreeAgents(s.subagents);
  }
  return n;
}

function isOutputType(x: string): x is OutputType {
  return x === 'text' || x === 'image' || x === 'music' || x === 'video';
}

function buildSubagent(
  spec: TeamTemplateSubagent,
  indexRef: { n: number },
  siblingIndex: number,
  parentPos: { x: number; y: number },
  color: string,
): AgentNode {
  const idx = indexRef.n;
  indexRef.n += 1;
  const offsets = [0, -280, 280, -560, 560];
  const xOffset = offsets[siblingIndex % offsets.length];
  const position = { x: parentPos.x + xOffset, y: parentPos.y + 160 };
  const id = `agent-${Date.now()}_${idx}_${Math.random().toString(36).slice(2, 9)}`;
  const subagents =
    spec.subagents?.map((c, i) => buildSubagent(c, indexRef, i, position, color)) ?? undefined;
  return {
    id,
    index: idx,
    name: spec.name.trim(),
    description: spec.description.trim(),
    color,
    model: DEFAULT_AGENT_CHAT_MODEL,
    position,
    subagents,
  };
}

export function saveTeamTemplate(
  agent: AgentActionContext,
  raw: SaveTeamTemplateArgs,
): false | SavedTeamTemplateRef {
  const store = useCoreStore.getState();
  if (agent.data.index !== 1) {
    console.warn('[ToolRegistry] save_team_template: only the lead agent may save templates.');
    return false;
  }
  if (store.phase !== 'idle' && store.phase !== 'working') return false;
  if (useTeamStore.getState().selectedAgentSetId !== CONSULTANT_WORKSHOP_TEAM_ID) return false;

  const args = raw;
  const teamName = typeof args.teamName === 'string' ? args.teamName.trim() : '';
  const teamType = typeof args.teamType === 'string' ? args.teamType.trim() : '';
  const teamDescription = typeof args.teamDescription === 'string' ? args.teamDescription.trim() : '';
  const outputTypeRaw = typeof args.outputType === 'string' ? args.outputType : '';
  const leadName = typeof args.leadName === 'string' ? args.leadName.trim() : '';
  const leadDescription = typeof args.leadDescription === 'string' ? args.leadDescription.trim() : '';

  if (!teamName || !teamType || !teamDescription || !leadName || !leadDescription || !isOutputType(outputTypeRaw)) {
    return false;
  }

  const totalAgents = 1 + countSubtreeAgents(args.subagents);
  if (totalAgents > MAX_AGENTS) {
    console.warn(`[ToolRegistry] save_team_template: at most ${MAX_AGENTS} agents (including lead).`);
    return false;
  }

  const outputType = outputTypeRaw;
  const defaultModel = DEFAULT_MODELS[outputType];
  const outputModel =
    typeof args.outputModel === 'string' && args.outputModel.trim() ? args.outputModel.trim() : defaultModel;
  const color = typeof args.color === 'string' && /^#[0-9A-Fa-f]{6}$/.test(args.color) ? args.color : '#6366F1';

  const indexRef = { n: 2 };
  const leadPos = { x: 0, y: 150 };
  const leadId = `agent-${Date.now()}_lead_${Math.random().toString(36).slice(2, 9)}`;

  const leadAgent: AgentNode = {
    id: leadId,
    index: 1,
    name: leadName,
    description: leadDescription,
    color,
    model: DEFAULT_AGENT_CHAT_MODEL,
    humanInTheLoop: true,
    position: leadPos,
    subagents: args.subagents?.map((c, i) => buildSubagent(c, indexRef, i, leadPos, color)) ?? [],
  };

  const newSystem: AgenticSystem = {
    id: `team-${Date.now()}`,
    teamName,
    teamType,
    teamDescription,
    color,
    outputType,
    outputModel,
    outputAutoApprove: outputType === 'text',
    user: { index: 0, model: 'Human', position: { x: 0, y: 0 } },
    leadAgent,
  };

  useTeamStore.getState().saveCustomSystem(newSystem);
  store.addLogEntry({
    agentIndex: agent.data.index,
    action: `saved team template "${teamName}"`,
    taskId: undefined,
  });
  return { teamId: newSystem.id, teamName };
}
