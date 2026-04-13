import { AgentNode } from '../../data/agents';
import type { LLMToolCall } from '../../core/llm/types';
import { LLMMessage } from '../../core/llm/types';
import { AgentState } from '../../types';
import { useUiStore } from '../../integration/store/uiStore';
import { AgentBrain, type BrainHost, type ThinkOptions } from '../../core/agent/AgentBrain';
import type { AgentSimulation } from './AgentSimulation';

export class AgentHost implements BrainHost {
  public state: AgentState = 'idle';
  private currentTaskId: string | null = null;
  public readonly brain: AgentBrain;

  constructor(
    public readonly data: AgentNode,
    public readonly simulation: AgentSimulation,
  ) {
    this.brain = new AgentBrain(this);
  }

  /** Determines if the agent is currently available to respond to user messages. */
  public canChat(): boolean {
    // Allow chat while an agent is working so users can provide context mid-execution.
    // We only block when the agent is explicitly paused for review.
    return this.state !== 'on_hold';
  }

  public async think(
    prompt: string,
    options: ThinkOptions = {},
  ): Promise<{ text: string; toolCalls: LLMToolCall[] }> {
    return this.brain.think(prompt, options);
  }

  public async spark() {
    return this.brain.spark();
  }

  public async executeTask(taskId: string) {
    return this.brain.executeTask(taskId);
  }

  public async concludeProject() {
    return this.brain.concludeProject();
  }

  public getStatus(): string {
    return `${this.data.name} is ${this.state}${this.currentTaskId ? ` working on ${this.currentTaskId}` : ''}`;
  }

  public getCurrentTaskId(): string | null {
    return this.currentTaskId;
  }

  public setTask(taskId: string | null) {
    this.currentTaskId = taskId;
    this.setState(taskId ? 'working' : 'idle');
  }

  public setState(state: AgentState) {
    this.state = state;
    useUiStore.getState().setAgentStatus(this.data.index, state);
  }

  public appendHistory(message: LLMMessage) {
    this.brain.appendHistory(message);
  }

  public get isThinking(): boolean {
    return this.brain.isThinking;
  }

  public dispose() {
    // No-op for now
  }
}
