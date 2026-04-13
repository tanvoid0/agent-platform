import type { AgentNode } from '../../data/agents';
import type { AgentState } from '../../types';
import { type LLMMessage, type LLMToolDefinition, type PlanningFormSpec } from '../llm/types';
import { CONSULTANT_WORKSHOP_TEAM_ID, MAX_AGENTS } from '../../data/agents';
import { useTeamStore } from '../../integration/store/teamStore';
import { setUserBrief } from './tools/setUserBrief';
import { createRemoteProjectFromTool } from './tools/createRemoteProject';
import { saveTeamTemplate, type SaveTeamTemplateArgs } from './tools/saveTeamTemplate';
import { proposeTask } from './tools/proposeTask';
import { markTaskBlocked } from './tools/markTaskBlocked';
import { promoteTaskFromBacklog } from './tools/promoteTaskFromBacklog';
import { completeTask } from './tools/completeTask';
import { deliverProject } from './tools/deliverProject';
import { presentPlanningForm } from './tools/presentPlanningForm';
import { reviewTaskSubmission } from './tools/reviewTaskSubmission';
import {
  workspaceListTool,
  workspaceReadTool,
  workspaceWriteTool,
} from './tools/workspaceFiles';
import { hasRemoteProjectBackend } from '../../integration/api/projectRemoteApi';

export interface ToolCall {
  name: string;
  /** JSON-shaped tool arguments from the model (validated per handler). */
  args: unknown;
}

export type ToolProcessResult = {
  handled: boolean;
  savedTeamTemplate?: { teamId: string; teamName: string };
  createdProject?: { projectId: string; projectTitle: string };
  planningForm?: PlanningFormSpec;
};

/**
 * Interface that decouples the ToolRegistry from the 3D Simulation (AgentHost).
 * This allows the tool logic to be tested and used independently of the simulation.
 */
export interface AgentActionContext {
  data: Pick<AgentNode, 'index' | 'name' | 'humanInTheLoop'> & { subagents?: AgentNode[] };
  setState: (state: AgentState) => void;
  appendHistory: (message: LLMMessage) => void;
}

function saveTeamTemplateToolDefinition(): LLMToolDefinition {
  return {
    type: 'function',
    function: {
      name: 'save_team_template',
      description: `Persist a custom execution team after the user has approved the proposed structure (clear yes / go ahead). Same agentic flow as other teams: use in IDLE while planning, or in WORKING if you refine the template mid-flight. At most ${MAX_AGENTS} agents including the lead.`,
      parameters: {
        type: 'object',
        properties: {
          teamName: { type: 'string' },
          teamType: { type: 'string' },
          teamDescription: { type: 'string' },
          outputType: { type: 'string', enum: ['text', 'image', 'music', 'video'] },
          outputModel: { type: 'string', description: 'Optional; defaults to the app default for outputType.' },
          color: { type: 'string', description: 'Optional #RRGGBB for team accent.' },
          leadName: { type: 'string' },
          leadDescription: { type: 'string' },
          subagents: {
            type: 'array',
            description: 'Direct reports of the lead; nest further for deeper hierarchy.',
            items: {
              type: 'object',
              properties: {
                name: { type: 'string' },
                description: { type: 'string' },
                subagents: {
                  type: 'array',
                  items: {
                    type: 'object',
                    properties: {
                      name: { type: 'string' },
                      description: { type: 'string' },
                      subagents: {
                        type: 'array',
                        items: {
                          type: 'object',
                          properties: {
                            name: { type: 'string' },
                            description: { type: 'string' },
                          },
                          required: ['name', 'description'],
                        },
                      },
                    },
                    required: ['name', 'description'],
                  },
                },
              },
              required: ['name', 'description'],
            },
          },
        },
        required: ['teamName', 'teamType', 'teamDescription', 'outputType', 'leadName', 'leadDescription'],
      },
    },
  } as LLMToolDefinition;
}

export class ToolRegistry {
  /**
   * Processes a tool call by dispatching it to the appropriate tool handler.
   */
  public static async process(agent: AgentActionContext, toolCall: ToolCall): Promise<ToolProcessResult> {
    const { name, args } = toolCall;

    switch (name) {
      case 'set_user_brief':
        return { handled: setUserBrief(agent, args as { brief: string }) };
      case 'present_planning_form': {
        const r = presentPlanningForm(agent, args);
        if (!r.ok) return { handled: false };
        return { handled: true, planningForm: r.spec };
      }
      case 'save_team_template': {
        const r = saveTeamTemplate(agent, args as SaveTeamTemplateArgs);
        if (r && typeof r === 'object')
          return { handled: true, savedTeamTemplate: { teamId: r.teamId, teamName: r.teamName } };
        return { handled: false };
      }
      case 'create_project': {
        const project = await createRemoteProjectFromTool(agent, args as { userTitle: string });
        if (project && typeof project === 'object') {
          return { handled: true, createdProject: project };
        }
        return { handled: false };
      }
      case 'propose_task':
        return {
          handled: proposeTask(
            agent,
            args as {
              title: string;
              description: string;
              agentId: number;
              requiresApproval?: boolean;
              toBacklog?: boolean;
            },
          ),
        };
      case 'mark_task_blocked':
        return {
          handled: markTaskBlocked(agent, args as { taskId: string; reason?: string }),
        };
      case 'promote_task_from_backlog':
        return {
          handled: promoteTaskFromBacklog(agent, args as { taskId: string }),
        };
      case 'complete_task':
        return { handled: completeTask(agent, args as { taskId: string; output: string }) };
      case 'review_task_submission':
        return {
          handled: reviewTaskSubmission(agent, args as {
            taskId: string;
            decision: 'approve' | 'request_changes';
            feedback?: string;
          }),
        };
      case 'deliver_project':
        return { handled: await deliverProject(agent, args as { output: string }) };
      case 'workspace_list':
        return { handled: await workspaceListTool(agent, args as { path?: string }) };
      case 'workspace_read':
        return { handled: await workspaceReadTool(agent, args as { path?: string }) };
      case 'workspace_write':
        return {
          handled: await workspaceWriteTool(agent, args as { path?: string; content?: string }),
        };
      default:
        console.warn(`[ToolRegistry] Unknown tool: ${name}`);
        return { handled: false };
    }
  }

  public static getDefinitions(agentIndex: number, phase: string, subagentsCount: number = 0): LLMToolDefinition[] {
    const isLead = agentIndex === 1;
    const isManager = subagentsCount > 0;
    const tools: LLMToolDefinition[] = [];

    // 1. Idle Phase: Only Lead can set the brief
    if (phase === 'idle') {
      if (isLead) {
        tools.push({
          type: 'function',
          function: {
            name: 'set_user_brief',
            description:
              'Start this workspace’s project with the brief (moves to WORKING). Call as soon as the goal is actionable: what to build plus rough shape (e.g. simple browser notepad, HTML + vanilla JS). Encode unstated details as explicit assumptions in the brief instead of blocking on more questions. No fixed “start” phrase required.',
            parameters: {
              type: 'object',
              properties: { brief: { type: 'string' } },
              required: ['brief']
            }
          }
        });

        tools.push({
          type: 'function',
          function: {
            name: 'present_planning_form',
            description:
              'Show an interactive planning form in chat (IDLE only). Use when the user’s goal is vague—prefer this over long numbered question lists in plain text. Supply 2–8 fields: boolean (yes/no), single_select, multi_select (options required), text, or textarea. After the user submits the form, read their structured answers and then call set_user_brief with a concise brief that reflects those answers.',
            parameters: {
              type: 'object',
              properties: {
                title: { type: 'string', description: 'Short heading above the form.' },
                description: { type: 'string', description: 'Optional intro shown above fields.' },
                fields: {
                  type: 'array',
                  maxItems: 12,
                  description: 'Form fields; each needs unique id, label, and kind.',
                  items: {
                    type: 'object',
                    properties: {
                      id: { type: 'string', description: 'Stable key for this answer (e.g. platform).' },
                      label: { type: 'string', description: 'Question shown to the user.' },
                      kind: {
                        type: 'string',
                        enum: ['boolean', 'single_select', 'multi_select', 'text', 'textarea'],
                      },
                      options: {
                        type: 'array',
                        items: { type: 'string' },
                        description: 'Required for single_select and multi_select (at least 2 options).',
                      },
                      required: { type: 'boolean' },
                      helpText: { type: 'string' },
                    },
                    required: ['id', 'label', 'kind'],
                  },
                },
              },
              required: ['fields'],
            },
          },
        });

        if (useTeamStore.getState().selectedAgentSetId === CONSULTANT_WORKSHOP_TEAM_ID) {
          tools.push(saveTeamTemplateToolDefinition());
          tools.push({
            type: 'function',
            function: {
              name: 'create_project',
              description:
                'Create a new empty server-backed project workspace and switch to it. Use when the user wants a fresh project or has agreed on a title. Clears the current board; you can then set_user_brief on this new workspace.',
              parameters: {
                type: 'object',
                properties: {
                  userTitle: { type: 'string', description: 'Display name for the new project' },
                },
                required: ['userTitle'],
              },
            },
          });
        }
      }
      return tools;
    }

    // 2. Working Phase: Common tools for everyone
    // After deliver_project (phase === 'done'), the user may still chat for follow-ups or new scope.
    // Offer the same tool surface as WORKING so the lead can propose tasks and re-deliver.
    const phaseForTools = phase === 'done' ? 'working' : phase;
    if (phaseForTools === 'working') {
      if (isLead || isManager) {
        tools.push({
          type: 'function',
          function: {
            name: 'propose_task',
            description:
              'Assign task to an agent. Use toBacklog for ideas not yet ready to run; use requiresApproval for user sign-off before execution.',
            parameters: {
              type: 'object',
              properties: {
                title: { type: 'string' },
                description: { type: 'string' },
                agentId: { type: 'integer', description: 'Agent index' },
                requiresApproval: {
                  type: 'boolean',
                  description:
                    'If true, the user must approve this task in the UI (Needs your input / Kanban) before the agent starts work.',
                },
                toBacklog: {
                  type: 'boolean',
                  description:
                    'If true, place the card in Backlog (not the run queue). Ignored when requiresApproval is true.',
                },
              },
              required: ['title', 'description', 'agentId']
            }
          }
        });

        tools.push({
          type: 'function',
          function: {
            name: 'mark_task_blocked',
            description:
              'Put a task On hold when it is blocked by a dependency, another task, or an external factor. Not for human output review — that uses complete_task / review flow.',
            parameters: {
              type: 'object',
              properties: {
                taskId: { type: 'string' },
                reason: { type: 'string', description: 'Short note appended to the task description.' },
              },
              required: ['taskId'],
            },
          },
        });

        tools.push({
          type: 'function',
          function: {
            name: 'promote_task_from_backlog',
            description: 'Move a task from Backlog to Scheduled so execution can pick it up.',
            parameters: {
              type: 'object',
              properties: { taskId: { type: 'string' } },
              required: ['taskId'],
            },
          },
        });
      }

      tools.push(
        {
          type: 'function',
          function: {
            name: 'complete_task',
            description: 'Finish task. Output must be raw content, no introductions or credit for the work.',
            parameters: {
              type: 'object',
              properties: {
                taskId: { type: 'string' },
                output: { type: 'string', description: 'Task result in Markdown (e.g. code blocks, text, or research).' }
              },
              required: ['taskId', 'output']
            }
          }
        },
      );

      if (isLead || isManager) {
        tools.push({
          type: 'function',
          function: {
            name: 'review_task_submission',
            description:
              'Review another agent submission in Review status (output ready). Approve to mark done, or request changes with concise feedback.',
            parameters: {
              type: 'object',
              properties: {
                taskId: { type: 'string', description: 'Task id currently in review status.' },
                decision: {
                  type: 'string',
                  enum: ['approve', 'request_changes'],
                },
                feedback: {
                  type: 'string',
                  description:
                    'Required when decision=request_changes. Clear, concrete revision guidance for the assignee.',
                },
              },
              required: ['taskId', 'decision'],
            },
          },
        });
      }

      if (isLead) {
        tools.push({
          type: 'function',
          function: {
            name: 'deliver_project',
            description:
              'Final delivery of the full project results as Markdown (handoff document for the user).',
            parameters: {
              type: 'object',
              properties: { 
                output: { 
                  type: 'string', 
                  description: 'Full project document in Markdown (handoff). NO attribution needed.' 
                } 
              },
              required: ['output']
            }
          }
        });
      }

      if (
        isLead &&
        useTeamStore.getState().selectedAgentSetId === CONSULTANT_WORKSHOP_TEAM_ID
      ) {
        tools.push(saveTeamTemplateToolDefinition());
      }

      if (hasRemoteProjectBackend()) {
        tools.push(
          {
            type: 'function',
            function: {
              name: 'workspace_list',
              description:
                'List files and folders in the active project server sandbox (Deliverables). Use relative paths; omit path for the project root.',
              parameters: {
                type: 'object',
                properties: {
                  path: {
                    type: 'string',
                    description: 'Directory path relative to project root, or empty for root.',
                  },
                },
              },
            },
          },
          {
            type: 'function',
            function: {
              name: 'workspace_read',
              description: 'Read a UTF-8 text file from the active project sandbox.',
              parameters: {
                type: 'object',
                properties: {
                  path: { type: 'string', description: 'File path relative to project root.' },
                },
                required: ['path'],
              },
            },
          },
          {
            type: 'function',
            function: {
              name: 'workspace_write',
              description:
                'Create or overwrite a UTF-8 text file in the active project sandbox. Parent directories are created automatically.',
              parameters: {
                type: 'object',
                properties: {
                  path: { type: 'string', description: 'File path relative to project root.' },
                  content: { type: 'string', description: 'Full file contents.' },
                },
                required: ['path', 'content'],
              },
            },
          },
        );
      }
    }

    return tools;
  }
}
