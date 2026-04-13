import { AgenticSystem, CONSULTANT_WORKSHOP_TEAM_ID, getAllAgents } from '../../data/agents';
import type { ProposedPlanAmendmentPayload } from '../../integration/orchestration/proposedPlanAmendment';
import { useCoreStore } from '../../integration/store/coreStore';
import { TASK_EXECUTION_STALE_MS } from '../../integration/store/coreStoreTypes';
import { useTeamStore } from '../../integration/store/teamStore';
import { AgentHost } from './AgentHost';
import { useUiStore } from '../../integration/store/uiStore';
import { notifyExecuteTaskQueued, notifySparkQueued } from '../../integration/orchestration/enqueueOrchestrationJob';

/**
 * AgentSimulation — Autonomous orchestration (currently in-browser).
 *
 * Target architecture (multiplayer-style): a single authority holds tasks, LLM turns, and `payload.session`;
 * the UI renders store/server state and sends user intents. Today the runner is this tab’s JS; persisted
 * `PersistedProjectPayload` (including `session` poses + orchestration) syncs via Agent Platform projects. A worker would keep
 * the same `projectSession` wire shape.
 *
 * Concurrency: **each agent** runs at most one board task at a time (serial per agent). **Different agents**
 * can execute tasks **in parallel** — `processScheduledTasks` starts `startTaskExecution` for every idle agent
 * without waiting for others; overlapping `await` chains interleave on the event loop.
 *
 * Principles:
 * 1. Monitors the store to trigger autonomous loops.
 * 2. 3D and panels reflect `coreStore` / `useUiStore` (including persisted world snapshot).
 * 3. Re-checks scheduled tasks when agents become idle.
 */
const PROPOSED_PLAN_AMEND_DESC_MAX = 2500;

export class AgentSimulation {
  private agents: Map<number, AgentHost> = new Map();
  private system: AgenticSystem;
  private unsubs: (() => void)[] = [];
  private heartbeatInterval: ReturnType<typeof setInterval> | null = null;
  private lastSparkTriggerTime: number = 0;
  private lastLeadCoordinationTriggerTime: number = 0;
  private lastLeadCoordinationSignature: string = '';

  constructor(system: AgenticSystem) {
    this.system = system;
    this.initializeAgents();
    this.startStateMonitoring();
  }

  private startStateMonitoring() {
    // 1. Heartbeat safety net (Periodically check for scheduled tasks and empty boards)
    this.heartbeatInterval = setInterval(() => {
      const state = useCoreStore.getState();
      if (state.agentsOrchestrationPaused) return;
      if (
        state.phase === 'working' &&
        state.tasks.length === 0 &&
        useTeamStore.getState().selectedAgentSetId !== CONSULTANT_WORKSHOP_TEAM_ID
      ) {
        this.triggerAutonomousStrategy();
      } else if (state.phase === 'working') {
        this.processScheduledTasks();
        this.triggerLeadCoordinationIfNeeded();
      }
    }, 5000);

    // 2. Core Store Monitoring
    this.unsubs.push(
      useCoreStore.subscribe((state, prevState) => {
        if (prevState.agentsOrchestrationPaused && !state.agentsOrchestrationPaused && state.phase === 'working') {
          if (state.tasks.length === 0 && useTeamStore.getState().selectedAgentSetId !== CONSULTANT_WORKSHOP_TEAM_ID) {
            void this.triggerAutonomousStrategy();
          } else if (state.tasks.length > 0) {
            this.processScheduledTasks();
          }
          void this.checkProjectCompletion();
        }

        // A. Initial Strategy (Spark)
        if (
          state.phase === 'working' &&
          prevState.phase === 'idle' &&
          state.tasks.length === 0 &&
          !state.agentsOrchestrationPaused &&
          useTeamStore.getState().selectedAgentSetId !== CONSULTANT_WORKSHOP_TEAM_ID
        ) {
          this.triggerAutonomousStrategy();
        }

        // B. Task Lifecycle: Process SCHEDULED tasks
        if (state.phase === 'working' && !state.agentsOrchestrationPaused) {
          this.processScheduledTasks();
          this.triggerLeadCoordinationIfNeeded();
        }

        // C. Project Completion
        if (!state.agentsOrchestrationPaused) {
          this.checkProjectCompletion();
        }
      })
    );

    // 3. UI Store Monitoring (Cleanup)
    this.unsubs.push(
      useUiStore.subscribe((state, prevState) => {
        if (!state.isChatting && prevState.isChatting) {
          const core = useCoreStore.getState();
          if (core.agentsOrchestrationPaused) return;
          if (
            core.phase === 'working' &&
            core.tasks.length === 0 &&
            useTeamStore.getState().selectedAgentSetId !== CONSULTANT_WORKSHOP_TEAM_ID
          ) {
            this.triggerAutonomousStrategy();
          }
        }
      })
    );
  }

  /**
   * Browser refresh tears down JS; the project snapshot is loaded from the projects API (or localStorage). Re-run scheduling from restored store state.
   */
  public resumePersistedWorkflow() {
    const core = useCoreStore.getState();
    if (core.isGeneratingAsset) {
      core.setIsGeneratingAsset(false);
    }
    if (core.agentsOrchestrationPaused) return;
    if (core.phase !== 'working') return;
    if (core.tasks.length === 0) {
      if (useTeamStore.getState().selectedAgentSetId !== CONSULTANT_WORKSHOP_TEAM_ID) {
        void this.triggerAutonomousStrategy();
      }
    } else {
      this.processScheduledTasks();
      this.triggerLeadCoordinationIfNeeded();
    }
    void this.checkProjectCompletion();
  }

  /** Central method to check for and start available tasks. */
  public processScheduledTasks() {
    const state = useCoreStore.getState();
    if (state.phase !== 'working' || state.agentsOrchestrationPaused) return;

    state.tasks
      .filter(
        (t) =>
          ((t.status === 'scheduled' && !t.requiresUserApproval) || t.status === 'in_progress') &&
          state.taskExecution[t.id]?.status !== 'failed',
      )
      .forEach((task) => {
      const agent = this.getAgent(task.assignedAgentId);
      const uiStatus = useUiStore.getState().agentStatuses[task.assignedAgentId];
      
      // Resilience check: only start if agent is truly idle and not currently thinking.
      // We check both internal state and UI status as safety.
      if (agent && (agent.state === 'idle' || uiStatus === 'idle') && !agent.isThinking) {
        this.startTaskExecution(task.assignedAgentId, task.id);
      }
    });
  }

  /**
   * Lead-level autonomous triage loop:
   * - If lead is idle and some teammates are idle while work remains,
   *   ask lead to rebalance work, unblock stalled items, and push backlog.
   * - Uses cooldown + signature to avoid repetitive prompts.
   */
  private triggerLeadCoordinationIfNeeded() {
    const state = useCoreStore.getState();
    if (state.phase !== 'working' || state.agentsOrchestrationPaused) return;
    if (useTeamStore.getState().selectedAgentSetId === CONSULTANT_WORKSHOP_TEAM_ID) return;

    const lead = this.getAgent(1);
    if (!lead || lead.isThinking) return;
    if (lead.state !== 'idle' && useUiStore.getState().agentStatuses[1] !== 'idle') return;

    const nonLeadAgents = this.getAllAgents().filter((a) => a.data.index !== 1);
    const idleNonLead = nonLeadAgents.filter((a) => {
      const uiStatus = useUiStore.getState().agentStatuses[a.data.index];
      return !a.isThinking && (a.state === 'idle' || uiStatus === 'idle');
    });
    if (idleNonLead.length === 0) return;

    const openTasks = state.tasks.filter((t) => t.status !== 'done');
    if (openTasks.length === 0) return;

    const backlog = openTasks.filter((t) => t.status === 'backlog');
    const scheduled = openTasks.filter((t) => t.status === 'scheduled');
    const blocked = openTasks.filter((t) => t.status === 'on_hold');
    const inProgress = openTasks.filter((t) => t.status === 'in_progress');
    const review = openTasks.filter((t) => t.status === 'review');
    const failedExec = Object.values(state.taskExecution).filter((x) => x.status === 'failed');

    const needsCoordination =
      backlog.length > 0 ||
      scheduled.length > 0 ||
      blocked.length > 0 ||
      review.length > 0 ||
      failedExec.length > 0 ||
      inProgress.length > 0;
    if (!needsCoordination) return;

    const signature = [
      idleNonLead.map((a) => a.data.index).sort((a, b) => a - b).join(','),
      backlog.map((t) => t.id).sort().join(','),
      scheduled.map((t) => t.id).sort().join(','),
      blocked.map((t) => t.id).sort().join(','),
      review.map((t) => t.id).sort().join(','),
      failedExec.map((x) => x.taskId).sort().join(','),
    ].join('|');
    const now = Date.now();
    if (signature === this.lastLeadCoordinationSignature && now - this.lastLeadCoordinationTriggerTime < 30_000) {
      return;
    }
    if (now - this.lastLeadCoordinationTriggerTime < 12_000) return;

    this.lastLeadCoordinationSignature = signature;
    this.lastLeadCoordinationTriggerTime = now;

    const prompt = `You are the lead and currently idle. Run a quick team coordination sweep now:
- Identify idle agents and decide where they can help immediately.
- Check backlog/scheduled/on-hold/in-progress/review/failed work and remove blockers.
- Rebalance ownership by creating follow-up tasks for idle agents where applicable.
- Promote backlog items with promote_task_from_backlog when they should enter the run queue.
- Mark dependency or external waits with mark_task_blocked (not for human output review).
- If a task is stalled, create explicit unblocker tasks (diagnostics, dependency prep, review handoff, or workaround).
- For tasks in Review (output ready), use review_task_submission (approve or request changes with concrete feedback).
- Escalate to user review only for high-risk/security/compliance/product-direction decisions.
- Keep the board moving toward delivery; avoid duplicate or vague tasks.

If no concrete action is needed, do nothing. If action is needed, use concise propose_task calls with clear owners and outcomes.`;
    void lead.think(prompt, { silent: true });
  }

  private async triggerAutonomousStrategy() {
    const lead = this.getAgent(1);
    const ui = useUiStore.getState();
    const core = useCoreStore.getState();
    if (core.agentsOrchestrationPaused) return;
    if (useTeamStore.getState().selectedAgentSetId === CONSULTANT_WORKSHOP_TEAM_ID) return;

    // GUARD: Prevent duplication
    if (!lead || lead.isThinking || core.tasks.length > 0) return;
    if (ui.isChatting && ui.selectedNpcIndex === lead.data.index) return;
    
    if (Date.now() - this.lastSparkTriggerTime < 1000) return;
    this.lastSparkTriggerTime = Date.now();

    notifySparkQueued();
    await lead.spark();
  }

  private async startTaskExecution(agentIndex: number, taskId: string) {
    const agent = this.getAgent(agentIndex);
    if (!agent) return;
    const core = useCoreStore.getState();
    const previousStatus = core.taskExecution[taskId]?.status;
    const isRetry = previousStatus === 'failed' || previousStatus === 'retry_queued';

    agent.setTask(taskId); 
    core.updateTaskStatus(taskId, 'in_progress');
    core.markTaskExecutionRunning({
      taskId,
      agentIndex,
      step: isRetry ? 'Retrying failed step' : 'Starting task execution',
      retrying: isRetry,
    });

    const taskMeta = core.tasks.find((t) => t.id === taskId);
    notifyExecuteTaskQueued({
      taskId,
      assignedAgentId: agentIndex,
      title: taskMeta?.title,
      isRetry,
    });

    await new Promise(resolve => setTimeout(resolve, Math.random() * 2000 + 1000));

    if (useCoreStore.getState().agentsOrchestrationPaused) {
      useCoreStore.getState().updateTaskStatus(taskId, 'scheduled');
      agent.setTask(null);
      agent.setState('idle');
      useCoreStore.getState().queueTaskExecutionRetry(taskId, 'Paused before execution');
      return;
    }

    const heartbeat = setInterval(() => {
      useCoreStore.getState().heartbeatTaskExecution(taskId, 'Waiting on model/tool response');
    }, 4000);

    try {
      if (!agent.isThinking) {
        useCoreStore.getState().heartbeatTaskExecution(taskId, 'Agent is thinking');
        await agent.executeTask(taskId);
      }
      useCoreStore.getState().markTaskExecutionSucceeded(taskId);
    } catch (err) {
      console.error(`[AgentSimulation] Agent ${agentIndex} failed:`, err);
      const message = err instanceof Error ? err.message : String(err);
      const lowered = message.toLowerCase();
      const retryable =
        lowered.includes('timeout') ||
        lowered.includes('network') ||
        lowered.includes('failed to fetch') ||
        lowered.includes('temporar') ||
        lowered.includes('connect') ||
        lowered.includes('503') ||
        lowered.includes('429');
      useCoreStore.getState().markTaskExecutionFailed(taskId, message, retryable);
      useCoreStore.getState().addLogEntry({
        agentIndex,
        action: `Task failed: ${message}. Use Retry to continue from this step.`,
        taskId,
      });
    } finally {
      clearInterval(heartbeat);
      // Resilience check: only clear task if not waiting for review or meeting
      if (agent.state !== 'on_hold' && agent.state !== 'talking') {
        agent.setTask(null);
        agent.setState('idle');
      }
      
      const failed = useCoreStore.getState().taskExecution[taskId]?.status === 'failed';
      if (!failed) {
        // KEY: When finished, check if there are other scheduled tasks waiting
        this.processScheduledTasks();
        
        // AND check if the project is now ready for delivery 
        // (Resilience for 1-agent teams where lead is thinking when the last task finishes)
        this.checkProjectCompletion();
      }
    }
  }

  public retryTaskExecution(taskId: string): boolean {
    const state = useCoreStore.getState();
    const task = state.tasks.find((t) => t.id === taskId);
    if (!task) return false;

    state.queueTaskExecutionRetry(taskId, 'Retry requested by user');
    const agent = this.getAgent(task.assignedAgentId);
    if (!agent) return false;

    if ((agent.state === 'idle' || useUiStore.getState().agentStatuses[task.assignedAgentId] === 'idle') && !agent.isThinking) {
      void this.startTaskExecution(task.assignedAgentId, task.id);
    } else {
      state.addLogEntry({
        agentIndex: task.assignedAgentId,
        action: 'Retry queued; waiting for agent to become idle.',
        taskId: task.id,
      });
    }
    return true;
  }

  /**
   * Requeue every failed or stalled execution in **Kanban / `tasks` array order** (not object-key order).
   * Each row uses the same path as the per-row Retry button; agents that are busy get queued behind.
   */
  public retryAllFailedOrStalledTaskExecutions(): number {
    const state = useCoreStore.getState();
    if (state.phase !== 'working' || state.agentsOrchestrationPaused) return 0;

    const now = Date.now();
    const ids: string[] = [];
    for (const task of state.tasks) {
      const run = state.taskExecution[task.id];
      if (!run) continue;
      if (run.status === 'failed') {
        ids.push(task.id);
        continue;
      }
      if (run.status === 'running' && now - run.lastHeartbeatAt > TASK_EXECUTION_STALE_MS) {
        ids.push(task.id);
      }
    }
    if (ids.length === 0) return 0;

    useCoreStore.getState().addLogEntry({
      agentIndex: this.system.leadAgent.index,
      action: `Requeue all: ${ids.length} task(s) in board order.`,
    });

    let n = 0;
    for (const taskId of ids) {
      if (this.retryTaskExecution(taskId)) n += 1;
    }
    return n;
  }

  private async checkProjectCompletion() {
    const state = useCoreStore.getState();
    if (state.agentsOrchestrationPaused) return;
    if (state.multimodalDeliveryDeferred || state.multimodalAssetBlocked) return;

    const allTasksFinished = state.tasks.length > 0 && state.tasks.every(t => t.status === 'done');
    
    if (state.phase === 'working' && allTasksFinished && !state.isGeneratingAsset) {
      const lead = this.getAgent(this.system.leadAgent.index);
      if (lead && !lead.isThinking) {
        await lead.concludeProject();
      }
    }
  }

  private initializeAgents() {
    const allAgents = getAllAgents(this.system);
    for (const agentData of allAgents) {
      this.agents.set(agentData.index, new AgentHost(agentData, this));
    }
  }

  public getAgent(index: number): AgentHost | undefined {
    return this.agents.get(index);
  }

  public getAllAgents(): AgentHost[] {
    return Array.from(this.agents.values());
  }



  public async handleUserMessage(agentIndex: number, text: string) {
    const agent = this.getAgent(agentIndex);
    if (!agent || !agent.canChat()) return null;
    const response = await agent.think(text, { isChat: true });
    return response.text;
  }

  /**
   * User chose "Amend & replan" on a scheduled task that required approval; the task was already
   * removed from the store. Lead re-plans via propose_task (possibly multiple tasks).
   */
  public requestProposedPlanAmendment(payload: ProposedPlanAmendmentPayload) {
    const lead = this.getAgent(this.system.leadAgent.index);
    if (!lead) return;
    const desc =
      payload.description.length > PROPOSED_PLAN_AMEND_DESC_MAX
        ? `${payload.description.slice(0, PROPOSED_PLAN_AMEND_DESC_MAX)}…`
        : payload.description;
    const prompt = `The user chose "Amend & replan" on a proposed task that was waiting for their approval. That card has been removed from the board so you can replace it cleanly.

Previous title: "${payload.title.replace(/"/g, "'")}"
Previous description (context):
"""
${desc}
"""
Planned assignee agent index: ${payload.assignedAgentId}

User's amendment / direction:
"""
${payload.feedback}
"""

Use propose_task to put one or more revised tasks on the board that reflect their feedback. Use requiresApproval: true when the task should wait for user approval again. You may split work into multiple tasks or consolidate into a single clearer task.`;
    void lead.think(prompt, { silent: true });
  }

  public dispose() {
    if (this.heartbeatInterval) clearInterval(this.heartbeatInterval);
    this.unsubs.forEach(unsub => unsub());
    this.unsubs = [];
    this.agents.forEach(a => a.dispose());
  }
}
