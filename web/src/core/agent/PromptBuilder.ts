import { AgentNode, AGENTIC_SETS, CONSULTANT_WORKSHOP_TEAM_ID } from '../../data/agents';
import { useCoreStore, type Task } from '../../integration/store/coreStore';
import type { AgentHost } from '../../simulation/core/AgentHost';
import { useLlmSessionStore } from '../../integration/store/llmSessionStore';
import { useTeamStore } from '../../integration/store/teamStore';
import {
  maxReferenceImagesForVideoModelId,
  resolveEffectiveGenerationModel,
} from '../llm/resolveGenerationModel';

/** Keeps system prompt size bounded — full task bodies live in store/UI, not every LLM turn. */
function clipPromptText(s: string | undefined, max: number, kind: string): string {
  if (!s) return '';
  if (s.length <= max) return s;
  return `${s.slice(0, max)}\n[${kind} truncated; ${s.length} chars total — see task panel for full text]`;
}

const BRIEF_MAX = 2800;
const TASK_OUTPUT_IN_PROMPT_MAX = 700;
const TASK_FEEDBACK_IN_PROMPT_MAX = 450;

export class PromptBuilder {
  /**
   * Builds the system prompt for an agent based on their role and current project context.
   */
  public static buildSystemPrompt(agent: AgentNode, phase: string, brief: string, allAgents: AgentHost[]): string {
    const isLead = agent.index === 1;
    const team = allAgents
      .map((a) => `[${a.data.index}] ${a.data.name}`)
      .join(', ');

    const selectedTeamId = useTeamStore.getState().selectedAgentSetId;
    const consultantWorkshopLead =
      isLead && selectedTeamId === CONSULTANT_WORKSHOP_TEAM_ID;

    const objectives = {
      idle: consultantWorkshopLead
        ? 'Same cadence as other leads: chat with [0], learn goals. When the goal is genuinely vague, prefer the present_planning_form tool (short intro in chat + structured fields) instead of long numbered question lists in plain text; keep forms small (about 3–7 fields). After [0] submits the form, synthesize their answers and call set_user_brief in the same or next turn. When [0] already gave enough detail (what to build plus rough shape), skip the form and call set_user_brief directly with assumptions in the brief text. Brief gate: once actionable, set_user_brief moves to WORKING. After the brief exists, propose execution-team ideas; call save_team_template only after [0] clearly approves. If [0] wants a brand-new empty workspace, call create_project after they agree on a title. Remind [0] to use Teams → Use team when switching to a saved execution template.'
        : isLead
          ? 'Chat with [0] to define brief. If the goal is vague, use present_planning_form for interactive questions; when answers exist or the goal is clear, set_user_brief.'
          : 'Wait for Lead to start.',
      working: consultantWorkshopLead
        ? 'Consultant Workshop = planning handoff, not the build team. Roster is only [0] + you; real implementation belongs on a saved execution team after handoff. In chat: propose a concrete execution team (role names, what each owns, how it maps to the Brief). After [0] clearly approves, call save_team_template. Remind [0] to use Teams → Use team on that template—then they continue there. Do NOT propose_task for implementation work (HTML/CSS/JS files, coding milestones) unless [0] explicitly asks you to plan tasks on this workshop without switching teams. An empty task board while you agree on a team is normal.'
        : isLead
          ? 'Manage board. As lead, when you are idle, proactively check for idle teammates, reassign effort by proposing concrete follow-up tasks, and create unblocker tasks for stalled/on-hold/failed work so delivery keeps moving. Review teammate submissions with review_task_submission so human approval is not required by default. Escalate to the human only for high-risk/security/compliance/product-direction decisions. deliver_project when all Done.'
          : 'Complete tasks.',
      done:
        'Delivered. The user may still message you for tweaks, questions, or follow-on work. If they want new execution, use propose_task (that moves the workspace back to active work). Answer conversationally when no board changes are needed.',
    };

    const tasks: Task[] = useCoreStore.getState().tasks;
    const board = tasks.length > 0
      ? tasks.map((t: Task) => {
          const agentName =
            allAgents.find((a) => a.data.index === t.assignedAgentId)?.data?.name || `Agent ${t.assignedAgentId}`;
          
          const feedbackStr = t.reviewComments
            ? `\n   >> USER FEEDBACK / REVISION REQUESTED: "${clipPromptText(t.reviewComments, TASK_FEEDBACK_IN_PROMPT_MAX, 'Feedback')}"`
            : '';
            
          const outputStr = (t.status === 'done' && t.output)
            ? `\n   >> FINAL APPROVED WORK (excerpt):\n   """\n   ${clipPromptText(t.output, TASK_OUTPUT_IN_PROMPT_MAX, 'Task output')}\n   """`
            : '';

          return `* [${t.status.toUpperCase()}] ${t.title} (Owner: ${agentName})${feedbackStr}${outputStr}`;
        }).join('\n\n')
      : 'Empty';

    const activeTeam = useTeamStore.getState().customSystems.find(s => s.id === selectedTeamId) 
      || AGENTIC_SETS.find(s => s.id === selectedTeamId);
      
    const referenceImages = useCoreStore.getState().referenceImages;
    const hasImages = referenceImages.length > 0 && (activeTeam?.outputType === 'image' || activeTeam?.outputType === 'video');
    
    let modelLimitInfo = '';
    if (activeTeam?.outputType === 'video') {
      const effectiveVideoModel = resolveEffectiveGenerationModel(
        useLlmSessionStore.getState().llmConfig,
        activeTeam
      );
      const maxRef = maxReferenceImagesForVideoModelId(effectiveVideoModel);
      modelLimitInfo =
        maxRef === 1
          ? ` Note: The current model (${effectiveVideoModel}) supports only 1 reference image for animation.`
          : ` Note: The current model (${effectiveVideoModel}) supports up to 3 reference images for style and content guidance.`;
    }

    const imageInstruction = hasImages
      ? `\n6. REFERENCE IMAGES: The user has provided ${referenceImages.length} reference image(s). You MUST use these as a visual guide for the project's style, mood, and content. Your team should analyze these to ensure the final ${activeTeam?.outputType} aligns with the inspiration.${modelLimitInfo}`
      : '';

    const effectiveGenerationModel = activeTeam
      ? resolveEffectiveGenerationModel(useLlmSessionStore.getState().llmConfig, activeTeam)
      : '';
    const outputInstruction = activeTeam?.outputType !== 'text'
      ? `\n4. TEAM OUTPUT: ${activeTeam?.outputType?.toUpperCase()}. Your 'deliver_project' output MUST be a highly detailed PROMPT for a ${activeTeam?.outputType} generator model (${effectiveGenerationModel}).
CRITICAL: You MUST synthesize all subagent findings, research results, and any user feedback into this final prompt. DO NOT just repeat your initial brief.
The generation model expects a SINGLE prompt to produce a SINGLE ${activeTeam?.outputType}. Be precise.`
      : '';

    const pendingReviews = tasks.filter(t => t.assignedAgentId === agent.index && t.reviewComments);
    const reviewContext = pendingReviews.length > 0
      ? `\nREVISION REQUESTED:\n${pendingReviews.map(t => `- [${t.title}] Feedback: ${clipPromptText(t.reviewComments, TASK_FEEDBACK_IN_PROMPT_MAX, 'Feedback')}`).join('\n')}`
      : '';

    const briefBlock = brief
      ? `Brief: ${clipPromptText(brief, BRIEF_MAX, 'Brief')}`
      : '';

    return `Agent: ${agent.name}. Role: ${agent.description}. Phase: ${phase}.
${briefBlock ? `${briefBlock}\n` : ''}${reviewContext}
Team: User (0), ${team}
Task board:
${board}
RULES:
1. CHAT (when talking to the user): Write so a non-expert can follow you. Use plain language, short sentences, and explain any necessary jargon. Aim for about 2–5 sentences unless the user asks for depth. Sound human and helpful, not like a compressed spec. For vague scope in IDLE, prefer present_planning_form over a long prose questionnaire. Exception: when scope is clear enough to call set_user_brief, give at most one short sentence of acknowledgment and call the tool—do not add more interview questions in that same turn.
2. STRUCTURED OUTPUTS (task titles/descriptions, 'complete_task', 'deliver_project'): Keep each under ~100 words unless the brief clearly needs more. No filler, long intros/outros, or self-attribution ("I have completed..."). Put substance in the tool payload, not meta-commentary about the work.
3. Phase tools: use normal WORKING tools on the board. IDLE (lead): present_planning_form when you need structured clarifiers; set_user_brief when the brief is ready. Consultant Workshop lead: save_team_template only after [0] approves a proposed structure (IDLE or WORKING); create_project in IDLE when [0] agreed to a new server project. Consultant Workshop in WORKING: treat heavy implementation as out of scope unless [0] explicitly wants you to build here—your default is team design + save_team_template, not shipping the product on this board. In autonomous teams, managers/leads should use review_task_submission for tasks in Review (output ready); use mark_task_blocked for dependency blocks (On hold). involve [0] only when there is significant risk, ambiguity, or a decision that changes scope/policy.
4. QUALITY: If your node has 'Human-in-the-loop' enabled, your 'complete_task' result will be reviewed by the user before completion.
5. In chat, avoid empty phrases like "Here is the result" when the answer is obvious; still be clear about what you need from the user next.${outputInstruction}${imageInstruction}
6. LANGUAGE: Systemic outputs (tasks, 'complete_task', 'deliver_project') must match the language of the Brief or the user's messages (e.g. Spanish brief → Spanish task text and deliverables).
Goal: ${objectives[phase as keyof typeof objectives] || ''}`;
  }
}
