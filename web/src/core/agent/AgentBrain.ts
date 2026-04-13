import type { AgentSimulation } from '../../simulation/core/AgentSimulation';
import type { OutputGenerationParams } from '../llm/outputGenerationParams';
import {
  LLMMessage,
  type LLMToolCall,
  type LLMToolDefinition,
  type LLMTokenUsage,
  type PlanningFormSpec,
} from '../llm/types';
import { resolveEffectiveGenerationModel } from '../llm/resolveGenerationModel';
import { assertBudgetAllowsCloudSpend } from '../finance/budgetPolicy';
import {
  analyzeAgentThinkFailure,
  analyzeCloudMediaFailure,
  chatCompletionUsesCloudBilling,
  createChatLlmProvider,
  createCloudMediaClient,
  getMediaReadiness,
  resolveChatModelForSession,
  resolveMediaBackend,
} from '../llm/llmFacade';
import { useLlmSessionStore } from '../../integration/store/llmSessionStore';
import { useUiStore } from '../../integration/store/uiStore';
import { useCoreStore, type MultimodalOutputKind } from '../../integration/store/coreStore';
import { useTeamStore } from '../../integration/store/teamStore';
import { ToolRegistry, type AgentActionContext, type ToolCall } from './ToolRegistry';
import { PromptBuilder } from './PromptBuilder';
import { AGENTIC_SETS, AgentNode, CONSULTANT_WORKSHOP_TEAM_ID } from '../../data/agents';

/** Simulation-backed brain host: full `AgentNode` for model routing and system prompts. */
export interface BrainHost extends AgentActionContext {
  data: AgentNode;
  simulation: AgentSimulation;
  getCurrentTaskId: () => string | null;
}

export interface ThinkOptions {
  isChat?: boolean;
  tools?: LLMToolDefinition[];
  silent?: boolean;
}

/** Passed through to cloud media providers — same shape as `pendingOutputParams` / Output review UI. */
export type FinalAssetOptions = OutputGenerationParams;

export class AgentBrain {
  private history: LLMMessage[] = [];
  /** Last seen `agentHistoryClearGeneration` for this agent; must match to write history to the store. */
  private syncGenBaseline = 0;
  public isThinking: boolean = false;

  constructor(private readonly host: BrainHost) {
    this.refreshFromStore();
  }

  public async think(prompt: string, options: ThinkOptions = {}): Promise<{ text: string; toolCalls: LLMToolCall[] }> {
    if (this.isThinking) return { text: '', toolCalls: [] };
    this.isThinking = true;

    try {
      this.refreshFromStore();
      const core = useCoreStore.getState();
      const llmConfig = useLlmSessionStore.getState().llmConfig;
      const provider = createChatLlmProvider(llmConfig.apiKey);
      const model = resolveChatModelForSession(llmConfig, this.host.data.model);
      const teamId = useTeamStore.getState().selectedAgentSetId;
      const activeTeam = useTeamStore.getState().customSystems.find(s => s.id === teamId)
        || AGENTIC_SETS.find(s => s.id === teamId);

      const hasVisionSupport = activeTeam?.outputType === 'image' || activeTeam?.outputType === 'video';

      // 1. Manage Message History
      if (!options.isChat) {
        if (useCoreStore.getState().agentsOrchestrationPaused) {
          return { text: '', toolCalls: [] };
        }
        const userMsg: LLMMessage = {
          role: 'user',
          content: prompt,
          metadata: options.silent ? { internal: true } : undefined
        };
        
        // Attach reference images if VISION is supported for this project type
        if (hasVisionSupport && core.referenceImages.length > 0) {
          userMsg.images = core.referenceImages;
        }

        this.history.push(userMsg);
        this.syncToStore();
      }

      // 2. Prepare context
      let messages: LLMMessage[] = this.history.slice(-10);

      if (!options.isChat && useCoreStore.getState().agentsOrchestrationPaused) {
        const last = this.history[this.history.length - 1];
        if (last?.role === 'user') {
          this.history.pop();
          this.syncToStore();
        }
        return { text: '', toolCalls: [] };
      }

      // In chat mode, ensure the latest user message also carries images if it's the brief phase
      if (options.isChat && hasVisionSupport && core.referenceImages.length > 0) {
        messages = messages.map((m, idx) => {
          if (idx === messages.length - 1 && m.role === 'user') {
            return { ...m, images: core.referenceImages };
          }
          return m;
        });
      }
      const allAgents = this.host.simulation.getAllAgents();
      const systemPrompt = PromptBuilder.buildSystemPrompt(this.host.data, core.phase, core.userBrief, allAgents);
      const toolDefs = options.tools || ToolRegistry.getDefinitions(this.host.data.index, core.phase, this.host.data.subagents?.length || 0);

      // 3. Log and Execute LLM Call
      core.addRequestLog({
        agentIndex: this.host.data.index,
        agentName: this.host.data.name,
        systemInstruction: systemPrompt,
        contents: messages,
        systemTools: toolDefs,
        taskId: this.host.getCurrentTaskId() || undefined
      });

      if (chatCompletionUsesCloudBilling()) {
        assertBudgetAllowsCloudSpend();
      }

      const response = await provider.generateCompletion(
        messages,
        toolDefs,
        systemPrompt,
        model
      );

      core.addResponseLog({
        agentIndex: this.host.data.index,
        agentName: this.host.data.name,
        content: response.content || '',
        tool_calls: response.tool_calls,
        usage: response.usage,
        raw: response.raw,
        taskId: this.host.getCurrentTaskId() || undefined
      });

      if (!options.isChat && useCoreStore.getState().agentsOrchestrationPaused) {
        this.history.push({
          role: 'assistant',
          content:
            '[Orchestration paused — this model reply was not applied. Press Resume to continue; you may repeat the last step if needed.]',
          metadata: { internal: true },
        });
        this.syncToStore();
        return { text: response.content || '', toolCalls: [] };
      }

      // 5. Parse Tool Calls
      const text = response.content || '';
      const parsedToolCalls: ToolCall[] =
        response.tool_calls?.flatMap((tc) => {
          try {
            return [{ name: tc.function.name, args: JSON.parse(tc.function.arguments) as unknown }];
          } catch (_err) {
            console.error('[AgentBrain] Failed to parse tool arguments', tc.function.arguments);
            return [];
          }
        }) ?? [];

      // 6. Final Message Construction
      const isInternalTrigger = options.silent;
      const hasToolCallsOnly = !text && parsedToolCalls.length > 0;
      const isBrief = parsedToolCalls.some(tc => tc.name === 'set_user_brief');
      const isPlanningForm = parsedToolCalls.some(tc => tc.name === 'present_planning_form');
      const isProjectCreate = parsedToolCalls.some(tc => tc.name === 'create_project');
      const isTeamTemplateSave = parsedToolCalls.some(tc => tc.name === 'save_team_template');
      const isWorkspaceTool = parsedToolCalls.some(tc =>
        ['workspace_list', 'workspace_read', 'workspace_write'].includes(tc.name),
      );
      const isResolution = false;
      let finalContent = text;
      const isMalformed = response.finishReason === 'MALFORMED_FUNCTION_CALL';

      if (isMalformed) {
        finalContent = 'ERROR: Malformed function call. Please try again.';
        console.warn(`[AgentBrain:${this.host.data.name}] Malformed function call detected.`);
      } else if (hasToolCallsOnly && !isInternalTrigger) {
        finalContent = isBrief
          ? "Project brief set. Let's begin!"
          : isPlanningForm
            ? 'A few quick questions to shape the brief:'
            : isProjectCreate
              ? 'New project workspace created and switched. Continue from here (e.g. set the brief when ready).'
              : isTeamTemplateSave
                ? 'Team template saved. Open Teams and Use team on it when you are ready, then continue with the handoff brief if needed.'
                : isWorkspaceTool
                  ? 'Workspace updated. Check **Deliverables** in the action log panel or the activity log for file details.'
                  : 'Working on it...';
      } else if (!text && parsedToolCalls.length === 0 && !isInternalTrigger) {
        finalContent = '...';
      }

      // UI/UX handling for chat auto-closing
      if (options.isChat && (isBrief || isResolution)) {
        setTimeout(() => {
          if (useUiStore.getState().isChatting) useUiStore.getState().setChatting(false);
          useUiStore.getState().setSelectedNpc(null);
        }, 3000);
      }

      const isInternalMessage = isInternalTrigger || (hasToolCallsOnly && isInternalTrigger);
      this.history.push({
        role: 'assistant',
        content: finalContent,
        tool_calls: response.tool_calls,
        metadata: isInternalMessage ? { internal: true } : undefined
      });
      this.syncToStore();

      // 7. Process Actions (Tools)
      let lastSavedTeamTemplate: { teamId: string; teamName: string } | undefined;
      let lastCreatedProject: { projectId: string; projectTitle: string } | undefined;
      let lastPlanningForm: PlanningFormSpec | undefined;
      for (const tc of parsedToolCalls) {
        const { handled, savedTeamTemplate, createdProject, planningForm } =
          await ToolRegistry.process(this.host, tc);
        if (savedTeamTemplate) lastSavedTeamTemplate = savedTeamTemplate;
        if (createdProject) lastCreatedProject = createdProject;
        if (planningForm) lastPlanningForm = planningForm;
        if (tc.name === 'deliver_project' && handled) {
          const deliverArgs = tc.args as { output?: unknown };
          const out = deliverArgs.output;
          if (typeof out === 'string') void this.handleFinalAssetGeneration(out);
        }
      }

      if (lastSavedTeamTemplate || lastCreatedProject || lastPlanningForm) {
        const last = this.history[this.history.length - 1];
        if (last?.role === 'assistant') {
          last.metadata = {
            ...last.metadata,
            ...(lastSavedTeamTemplate
              ? {
                savedTeamTemplateId: lastSavedTeamTemplate.teamId,
                savedTeamTemplateName: lastSavedTeamTemplate.teamName,
              }
              : {}),
            ...(lastCreatedProject
              ? {
                createdProjectId: lastCreatedProject.projectId,
                createdProjectTitle: lastCreatedProject.projectTitle,
              }
              : {}),
            ...(lastPlanningForm
              ? {
                  planningForm: lastPlanningForm,
                  planningFormStatus: 'open' as const,
                }
              : {}),
          };
          this.syncToStore();
        }
      }

      if (!lastPlanningForm && parsedToolCalls.some((tc) => tc.name === 'present_planning_form')) {
        const last = this.history[this.history.length - 1];
        const soloInvalidPlanning =
          !text.trim() &&
          parsedToolCalls.length > 0 &&
          parsedToolCalls.every((tc) => tc.name === 'present_planning_form');
        if (last?.role === 'assistant' && soloInvalidPlanning) {
          last.content =
            'I could not show the planning form. Please describe what you want to build in your own words, and I will set the brief.';
          this.syncToStore();
        }
      }

      // Silent lead spark often proposes tasks with requiresUserApproval; those were invisible in chat (empty internal turn).
      if (options.silent && parsedToolCalls.some((tc) => tc.name === 'propose_task')) {
        const pending = useCoreStore
          .getState()
          .tasks.filter((t) => t.status === 'scheduled' && t.requiresUserApproval);
        if (pending.length > 0) {
          const last = this.history[this.history.length - 1];
          if (
            last?.role === 'assistant' &&
            last.metadata?.internal &&
            !(typeof last.content === 'string' && last.content.trim())
          ) {
            last.metadata = undefined;
            last.content = `I proposed **${pending.length}** task(s) on the board. Use **Needs your input** in the project sidebar to approve each one (work starts after approval), or review them in the Kanban.`;
            this.syncToStore();
          }
        }
      }

      return { text, toolCalls: response.tool_calls ?? [] };
    } catch (error) {
      const fail = analyzeAgentThinkFailure(error);
      if (fail.kind === 'budget') {
        const c = useCoreStore.getState();
        c.openBudgetExceeded(fail.message);
        this.history.push({
          role: 'assistant',
          content:
            'Stopped: this project’s estimated spend has reached its budget limit. Open Finance to raise the cap or reset usage.',
        });
        this.syncToStore();
        c.addLogEntry({
          agentIndex: this.host.data.index,
          action: 'Blocked: project budget limit reached (cloud LLM call skipped).',
          taskId: this.host.getCurrentTaskId() || undefined,
        });
        return { text: '', toolCalls: [] };
      }
      console.error(`[AgentBrain:${this.host.data.name}] Logic error:`, error);
      if (fail.openByok) {
        useUiStore.getState().setBYOKOpen(true, fail.message);
      }
      if (options.isChat) {
        this.history.push({
          role: 'assistant',
          content: `I could not get a reply from the model (${fail.message}). Check your LLM connection or settings, then try again.`,
        });
        this.syncToStore();
        return { text: '', toolCalls: [] };
      }
      throw error;
    } finally {
      this.isThinking = false;
      this.host.simulation.processScheduledTasks();
    }
  }

  /** Autonomous Intent: Start the project strategy. */
  public async spark() {
    const teamId = useTeamStore.getState().selectedAgentSetId;
    const prompt =
      teamId === CONSULTANT_WORKSHOP_TEAM_ID
        ? 'Brief is set. You are on Consultant Workshop (planning handoff only). In your next visible chat with [0], propose a concrete execution team for this brief (roles, names, responsibilities)—not a task board of code milestones. After they approve, save_team_template. Do not propose implementation tasks or write code unless they explicitly want you to build on this workshop without switching teams.'
        : 'Start the project by proposing initial tasks.';
    return this.think(prompt, { silent: true });
  }

  /** Autonomous Intent: Work on a specific task. */
  public async executeTask(taskId: string) {
    return this.think(`Proceed with task: ${taskId}`, { silent: true });
  }

  /** Autonomous Intent: Finalize and deliver the project results. */
  public async concludeProject() {
    return this.think('All tasks are complete! Use the deliver_project tool to fulfill the final delivery with the project result.', { silent: true });
  }

  private async handleFinalAssetGeneration(prompt: string) {
    const core = useCoreStore.getState();
    const teamId = useTeamStore.getState().selectedAgentSetId;
    const activeTeam = useTeamStore.getState().customSystems.find(s => s.id === teamId)
      || AGENTIC_SETS.find(s => s.id === teamId);

    if (!activeTeam) return;

    const out = activeTeam.outputType;
    const needsCloudMedia = out === 'image' || out === 'music' || out === 'video';
    const apiKey = useLlmSessionStore.getState().llmConfig.apiKey;
    if (needsCloudMedia) {
      const media = getMediaReadiness(out as 'image' | 'music' | 'video', apiKey);
      if (!media.ready) {
      core.openMultimodalAssetBlocked({
        summaryPrompt: prompt,
        outputType: out === 'music' ? 'music' : out,
        backend: media.backend,
        reason: media.reason,
      });
      if (this.host.data.index === 1) {
        this.appendHistory({
          role: 'assistant',
          content:
            `I submitted the delivery, but **I cannot generate the final ${out}** from here: ${media.reason || 'the selected media backend is unavailable'}. Please confirm in the dialog—skip, mock placeholder, or defer.`,
        });
      }
      return;
      }
    }

    // Check if we need manual approval
    if (activeTeam.outputAutoApprove === false) {
      core.setPendingOutputPrompt(prompt);

      const pref = core.assetGenerationDefaults;
      const llm = useLlmSessionStore.getState().llmConfig;

      const defaultParams: FinalAssetOptions = {
        model: resolveEffectiveGenerationModel(llm, activeTeam),
      };
      if (activeTeam.outputType === 'image') {
        defaultParams.aspectRatio = pref.image.aspectRatio;
        defaultParams.imageSize = pref.image.imageSize;
      } else if (activeTeam.outputType === 'video') {
        defaultParams.resolution = pref.video.resolution;
        defaultParams.aspectRatio = pref.video.aspectRatio;
        defaultParams.durationSeconds = pref.video.durationSeconds;
      }

      core.setPendingOutputParams(defaultParams);
      core.setReviewingOutput(true);
      return;
    }

    // Standard auto-approve flow
    await this.processFinalAsset(prompt, { model: activeTeam.outputModel });
  }

  public async processFinalAsset(prompt: string, options: FinalAssetOptions) {
    const core = useCoreStore.getState();
    const teamId = useTeamStore.getState().selectedAgentSetId;
    const activeTeam = useTeamStore.getState().customSystems.find(s => s.id === teamId)
      || AGENTIC_SETS.find(s => s.id === teamId);

    if (!activeTeam) return;

    const out = activeTeam.outputType;
    const needsCloudMedia = out === 'image' || out === 'music' || out === 'video';
    const llmConfig = useLlmSessionStore.getState().llmConfig;
    if (needsCloudMedia) {
      const media = getMediaReadiness(out as 'image' | 'music' | 'video', llmConfig.apiKey);
      if (!media.ready) {
        core.openMultimodalAssetBlocked({
          summaryPrompt: prompt,
          outputType: out === 'music' ? 'music' : out,
          backend: media.backend,
          reason: media.reason,
        });
        core.setReviewingOutput(false);
        return;
      }
    }

    // Text deliverables do not use the Gemini media client; never gate them on `apiKey`.
    if (activeTeam.outputType === 'text') {
      core.setReviewingOutput(false);
      core.setFinalOutput(prompt);
      core.setPhase('done');
      core.setFinalOutputOpen(true);
      return;
    }

    core.setIsGeneratingAsset(true);
    core.setReviewingOutput(false);

    try {
      const apiKeyTrimmed = llmConfig.apiKey?.trim();
      if (!apiKeyTrimmed && needsCloudMedia) {
        // Defensive: `getMediaReadiness` should already block; avoid throwing into BYOK for cloud media.
        const mediaBackend = resolveMediaBackend(out as 'image' | 'music' | 'video');
        core.setIsGeneratingAsset(false);
        core.openMultimodalAssetBlocked({
          summaryPrompt: prompt,
          outputType: out === 'music' ? 'music' : out === 'video' ? 'video' : 'image',
          backend: mediaBackend,
          reason:
            mediaBackend === 'gemini'
              ? 'Gemini backend selected but no API key is configured.'
              : 'Media generation is not available for the selected backend.',
        });
        return;
      }
      if (needsCloudMedia) {
        assertBudgetAllowsCloudSpend();
      }
      const provider = createCloudMediaClient(apiKeyTrimmed) as any;
      const model =
        options.model || resolveEffectiveGenerationModel(llmConfig, activeTeam);

      core.addLogEntry({
        agentIndex: -1,
        action: `Generating final ${activeTeam.outputType} using ${model}...`,
        taskId: undefined
      });

      let assetContent: string = '';
      let usage: LLMTokenUsage | undefined;

      if (activeTeam.outputType === 'image') {
        const result = await provider.generateImage(prompt, model, (msg: string) => {
          console.log(`[System:Image] ${msg}`);
        }, options, core.referenceImages);
        assetContent = result.data || '';
        usage = result.usage;
      } else if (activeTeam.outputType === 'music') {
        const result = await provider.generateAudio(prompt, model, (msg: string) => {
          console.log(`[System:Audio] ${msg}`);
        });
        assetContent = result.data || '';
        usage = result.usage;
      } else if (activeTeam.outputType === 'video') {
        const result = await provider.generateVideo(prompt, model, (msg: string) => {
          console.log(`[System:Video] ${msg}`);
        }, options, core.referenceImages);
        assetContent = result.videoUrl || '';
        usage = result.usage;
      }

      core.addResponseLog({
        agentIndex: -1,
        agentName: 'System',
        content: `Final ${activeTeam.outputType} generated successfully.`,
        usage: usage,
        raw: { model, ...usage },
        taskId: undefined
      });

      core.setFinalOutput(prompt);
      const assetChannel: 'image' | 'audio' | 'video' =
        activeTeam.outputType === 'music' ? 'audio' : activeTeam.outputType;
      core.setFinalAsset(assetChannel, assetContent);
      core.setPhase('done');
      core.setFinalOutputOpen(true);
    } catch (error) {
      console.error('[AgentBrain] Final asset generation failed:', error);
      core.setIsGeneratingAsset(false);
      const mediaFail = analyzeCloudMediaFailure(error, { multimodalOutput: needsCloudMedia });
      if (mediaFail.kind === 'budget') {
        core.openBudgetExceeded(mediaFail.message);
        core.addLogEntry({
          agentIndex: -1,
          action: 'Blocked: project budget limit reached (final media generation skipped).',
          taskId: undefined,
        });
        return;
      }
      if (mediaFail.kind === 'credentials_gap' && needsCloudMedia) {
        const outputType: MultimodalOutputKind =
          out === 'music' ? 'music' : out === 'video' ? 'video' : 'image';
        core.openMultimodalAssetBlocked({
          summaryPrompt: prompt,
          outputType,
        });
      } else {
        useUiStore.getState().setBYOKOpen(true, mediaFail.message);
      }
      core.addLogEntry({
        agentIndex: 0,
        action: `Error generating final ${activeTeam.outputType}: ${mediaFail.message}`,
        taskId: undefined
      });
    }
  }

  public appendHistory(message: LLMMessage) {
    this.refreshFromStore();
    this.history.push(message);
    this.syncToStore();
  }

  private refreshFromStore() {
    const idx = this.host.data.index;
    const history = useCoreStore.getState().agentHistories[idx];
    if (history !== undefined) {
      this.history = [...history];
    }
    this.syncGenBaseline = useCoreStore.getState().agentHistoryClearGeneration[idx] ?? 0;
  }

  private syncToStore() {
    const idx = this.host.data.index;
    const gen = useCoreStore.getState().agentHistoryClearGeneration[idx] ?? 0;
    if (gen !== this.syncGenBaseline) {
      this.refreshFromStore();
      return;
    }
    useCoreStore.getState().setAgentHistory(idx, this.history);
  }
}
