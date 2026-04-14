import type { OutputGenerationParams } from '../../core/llm/outputGenerationParams';
import type { LLMMessage, LLMTokenUsage, LLMToolCall, LLMToolDefinition } from '../../core/llm/types';
import type { OfficeVisualStyle } from '../../types';
import type { AgentPose } from '../projectSession';
import type { UsageLedgerEntry } from '../finance/usageLedger';

export type TaskStatus =
  | 'backlog'
  | 'scheduled'
  | 'on_hold'
  | 'review'
  | 'in_progress'
  | 'done';

export interface TaskRevision {
  output: string;
  feedback?: string;
  timestamp: number;
}

export interface Task {
  id: string;
  title: string;
  description: string;
  assignedAgentId: number;
  status: TaskStatus;
  parentTaskId?: string;
  requiresUserApproval: boolean;
  draftOutput?: string;
  reviewComments?: string;
  output?: string;
  revisions: TaskRevision[];
  createdAt: number;
  updatedAt: number;
}

export interface ActionLogEntry {
  id: string;
  timestamp: number;
  agentIndex: number;
  action: string;
  taskId?: string;
}

export type TaskExecutionStatus = 'running' | 'failed' | 'succeeded' | 'retry_queued';

/** Running row with no heartbeat longer than this is "stalled" (Execution Monitor + bulk retry). */
export const TASK_EXECUTION_STALE_MS = 25_000;

export interface TaskExecutionState {
  taskId: string;
  agentIndex: number;
  status: TaskExecutionStatus;
  currentStep: string;
  startedAt: number;
  updatedAt: number;
  lastHeartbeatAt: number;
  attempts: number;
  retryable: boolean;
  lastError?: string;
}

export interface DebugLogEntryBase {
  id: string;
  timestamp: number;
  agentIndex: number;
  agentName: string;
  status: 'pending' | 'completed' | 'error';
  taskId?: string;
}

export interface RequestDebugLogEntry extends DebugLogEntryBase {
  phase: 'request';
  systemInstruction?: string;
  /** Messages sent to the chat completion provider (same as `AgentBrain` request). */
  contents: LLMMessage[];
  /** Tool definitions attached to the request (`LLMToolDefinition[]` / OpenAI-style function schema). */
  systemTools?: LLMToolDefinition[];
}

export interface ResponseDebugLogEntry extends DebugLogEntryBase {
  phase: 'response';
  content: string | null;
  tool_calls?: LLMToolCall[];
  usage?: LLMTokenUsage;
  raw?: unknown;
}

export type DebugLogEntry = RequestDebugLogEntry | ResponseDebugLogEntry;

export type ProjectPhase = 'idle' | 'working' | 'done';

export type FinalAssetOutputType = 'text' | 'image' | 'audio' | 'video';

export type MultimodalOutputKind = 'image' | 'music' | 'video';

export type { UsageLedgerEntry, UsageLedgerKind } from '../finance/usageLedger';

export interface MultimodalAssetBlockedPayload {
  summaryPrompt: string;
  outputType: MultimodalOutputKind;
  backend?: 'gemini' | 'ollama' | 'lm_studio' | 'aimlapi' | 'disabled';
  reason?: string;
}

/** Defaults for the output-review modal when a team requires manual approval before generating media. */
export interface AssetGenerationDefaults {
  image: { aspectRatio: string; imageSize: string };
  video: { resolution: string; aspectRatio: string; durationSeconds: number };
}

export const DEFAULT_ASSET_GENERATION_DEFAULTS: AssetGenerationDefaults = {
  image: { aspectRatio: '16:9', imageSize: '1K' },
  video: { resolution: '720p', aspectRatio: '16:9', durationSeconds: 4 },
};

/** Full Zustand core store shape: project, tasks, logs, histories, UI flags, session sync. */
export interface CoreState {
  // ── Project ──────────────────────────────────────────────────
  userBrief: string;
  referenceImages: string[];
  phase: ProjectPhase;
  finalOutput: string | null;
  availableModels: string[];
  totalTokenUsage: LLMTokenUsage;
  agentTokenUsage: Record<number, LLMTokenUsage>;
  totalEstimatedCost: number;
  agentEstimatedCost: Record<number, number>;
  usageLedger: UsageLedgerEntry[];
  /** USD cap for estimated cloud spend; null or ≤0 disables enforcement */
  budgetLimitUsd: number | null;
  budgetExceededOpen: boolean;
  budgetExceededMessage: string;
  finalAssetType: FinalAssetOutputType;
  finalAssetContent: string | null;
  isGeneratingAsset: boolean;

  // ── Output Review ────────────────────────────────────────────
  isReviewingOutput: boolean;
  pendingOutputPrompt: string;
  pendingOutputParams: OutputGenerationParams;

  /** Lead delivered a multimodal project but the selected backend is unavailable — user must confirm skip / mock / defer. */
  multimodalAssetBlocked: MultimodalAssetBlockedPayload | null;
  /** User chose “decide later”; suppress auto-conclude until they resolve or reset. */
  multimodalDeliveryDeferred: boolean;

  // ── Tasks ────────────────────────────────────────────────────
  tasks: Task[];

  // ── Log ──────────────────────────────────────────────────────
  actionLog: ActionLogEntry[];
  debugLog: DebugLogEntry[];
  taskExecution: Record<string, TaskExecutionState>;

  // ── Conversation histories (Agnostic standard) ───────────────
  agentHistories: Record<number, LLMMessage[]>;
  /** Visible (non-internal) message count last seen open-chat for each agent; drives Activity unread. */
  chatReadVisibleLength: Record<number, number>;
  /**
   * Bumped when the user clears an agent chat so in-flight `AgentBrain.syncToStore` does not
   * overwrite the store with stale in-memory history. Not persisted (session-only).
   */
  agentHistoryClearGeneration: Record<number, number>;
  agentSummaries: Record<number, string>;
  boardroomHistories: Record<string, LLMMessage[]>;

  // ── UI ───────────────────────────────────────────────────────
  isKanbanOpen: boolean;
  officeVisualStyle: OfficeVisualStyle;
  assetGenerationDefaults: AssetGenerationDefaults;
  isLogOpen: boolean;
  isFinalOutputOpen: boolean;
  logFilterAgentIndex: number | null;
  isResizing: boolean;
  /** Milliseconds: last server `updatedAt` we merged from the projects API (avoids stale server overwriting fresher local state). */
  lastSyncedServerUpdatedAt: number;
  /** When true, autonomous work (scheduling tasks, lead spark, auto-conclude) is frozen until resumed. */
  agentsOrchestrationPaused: boolean;

  /**
   * Project session: last known 3D poses per agent. Persisted with the project payload; the renderer merges
   * live captures here and replaces from remote when `sessionSceneRevision` bumps.
   */
  sessionPoseByAgent: Record<number, AgentPose>;
  /** Incremented when poses from authority (saved project) must be reapplied in the 3D scene. */
  sessionSceneRevision: number;
  /** Bumped after server project create/switch so the simulation can reset avatars (not persisted). */
  simSceneResetNonce: number;

  // ── Actions — Project —————————————————————————————————────────
  setUserBrief: (brief: string) => void;
  addReferenceImage: (base64: string) => void;
  removeReferenceImage: (index: number) => void;
  clearReferenceImages: () => void;
  setPhase: (phase: ProjectPhase) => void;
  startProject: (brief: string) => void;
  setFinalOutput: (output: string) => void;
  setFinalAsset: (type: 'image' | 'audio' | 'video', content: string) => void;
  setIsGeneratingAsset: (isGenerating: boolean) => void;
  setReviewingOutput: (val: boolean) => void;
  setPendingOutputPrompt: (prompt: string) => void;
  setPendingOutputParams: (params: OutputGenerationParams) => void;
  openMultimodalAssetBlocked: (payload: MultimodalAssetBlockedPayload) => void;
  clearMultimodalAssetBlocked: () => void;
  resolveMultimodalAssetSkipped: () => void;
  resolveMultimodalAssetMocked: () => void;
  resolveMultimodalAssetDeferred: () => void;
  setBudgetLimitUsd: (limitUsd: number | null) => void;
  openBudgetExceeded: (message?: string) => void;
  closeBudgetExceeded: () => void;
  setAgentsOrchestrationPaused: (paused: boolean) => void;
  /** Merge live sim poses into the session (throttled); does not bump `sessionSceneRevision`. */
  mergeSessionPosesFromSimCapture: (entries: Record<number, AgentPose>) => void;
  /** Replace poses from saved/remote project and bump revision so the scene teleports avatars. */
  replaceSessionPosesFromAuthority: (poses: Record<number, AgentPose>) => void;
  bumpSimSceneReset: () => void;

  // ── Actions — Tasks ───────────────────────────────────────────
  addTask: (task: Omit<Task, 'id' | 'revisions' | 'createdAt' | 'updatedAt'>) => Task;
  removeTask: (taskId: string) => void;
  updateTaskStatus: (taskId: string, status: TaskStatus) => void;
  /** Dependency / external blocker — not the same as review (human sign-off). Clears any task execution row. */
  setTaskBlocked: (taskId: string, reason?: string) => void;
  submitTaskForReview: (taskId: string, draftOutput?: string) => void;
  setTaskOutput: (taskId: string, output: string) => void;
  approveTask: (taskId: string) => void;
  rejectTask: (taskId: string, comments: string) => void;
  /** Clears requiresUserApproval on a scheduled task so the simulation can start it. */
  approveProposedTask: (taskId: string) => void;
  /** Approve every proposed plan and every task in review that currently needs the user (same as Kanban "Needs you"). */
  approveAllAwaitingUserInput: () => void;
  /**
   * User feedback on a proposed (approval-gated) task: removes that card and asks the lead to
   * propose_task again with the new context (one or more replacements).
   */
  amendProposedTaskPlan: (taskId: string, feedback: string) => void;

  // ── Actions — Log ─────────────────────────────────────────────
  addLogEntry: (entry: Omit<ActionLogEntry, 'id' | 'timestamp'>) => void;
  addRequestLog: (entry: Omit<RequestDebugLogEntry, 'id' | 'timestamp' | 'phase' | 'status'>) => void;
  addResponseLog: (entry: Omit<ResponseDebugLogEntry, 'id' | 'timestamp' | 'phase' | 'status'>) => void;
  markTaskExecutionRunning: (params: {
    taskId: string;
    agentIndex: number;
    step: string;
    retrying?: boolean;
  }) => void;
  heartbeatTaskExecution: (taskId: string, step?: string) => void;
  markTaskExecutionFailed: (taskId: string, errorMessage: string, retryable?: boolean) => void;
  markTaskExecutionSucceeded: (taskId: string, step?: string) => void;
  queueTaskExecutionRetry: (taskId: string, step?: string) => void;

  // ── Actions — History ───────────────────────────────────────
  appendAgentHistory: (agentIndex: number, role: 'user' | 'assistant', parts: readonly unknown[]) => void;
  setAgentSummary: (agentIndex: number, summary: string) => void;
  appendBoardroomHistory: (taskId: string, role: 'user' | 'assistant', parts: readonly unknown[]) => void;
  clearAllHistories: () => void;
  bumpAgentHistoryClearGeneration: (agentIndex: number) => void;
  /** Mark all visible turns through the latest message as read for an agent (e.g. chat open / focused). */
  markAgentChatRead: (agentIndex: number) => void;

  // ── Actions — UI ──────────────────────────────────────────────
  setKanbanOpen: (open: boolean) => void;
  setLogOpen: (open: boolean, filterAgent?: number | null) => void;
  setFinalOutputOpen: (open: boolean) => void;
  setIsResizing: (isResizing: boolean) => void;
  resetProject: () => void;
  setOfficeVisualStyle: (style: OfficeVisualStyle) => void;
  setAssetGenerationDefaults: (patch: {
    image?: Partial<AssetGenerationDefaults['image']>;
    video?: Partial<AssetGenerationDefaults['video']>;
  }) => void;

  // ── Simulation Sync ──────────────────────────────────────────
  setAgentHistory: (agentIndex: number, history: LLMMessage[]) => void;
}
