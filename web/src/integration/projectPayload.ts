import { LLMMessage, LLMTokenUsage } from '../core/llm/types';
import { GEMINI_CATALOG } from '../core/llm/providerModelCatalog';
import {
  applyProjectSessionToStores,
  readProjectSessionFromStores,
  type ProjectSessionWire,
} from './projectSession';
import type { OutputGenerationParams } from '../core/llm/outputGenerationParams';
import {
  reconcileTaskExecutionRecord,
  useCoreStore,
  type ActionLogEntry,
  type DebugLogEntry,
  type ProjectPhase,
  type Task,
  type TaskExecutionState,
  type TaskExecutionStatus,
  type TaskStatus,
  type UsageLedgerEntry,
} from './store/coreStore';
import { setSkipProjectResetOnce, useTeamStore } from './store/teamStore';
import { backfillChatReadWatermarks } from './chat/chatReadWatermark';
import { clearProjectScopedUi } from './store/uiStore';

export interface PersistedProjectPayload {
  selectedAgentSetId: string;
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
  budgetLimitUsd: number | null;
  finalAssetType: 'text' | 'image' | 'audio' | 'video';
  finalAssetContent: string | null;
  isGeneratingAsset: boolean;
  isReviewingOutput: boolean;
  pendingOutputPrompt: string;
  pendingOutputParams: OutputGenerationParams;
  tasks: Task[];
  actionLog: ActionLogEntry[];
  debugLog: DebugLogEntry[];
  agentHistories: Record<number, LLMMessage[]>;
  chatReadVisibleLength?: Record<number, number>;
  agentSummaries: Record<number, string>;
  boardroomHistories: Record<string, LLMMessage[]>;
  isKanbanOpen: boolean;
  isLogOpen: boolean;
  isFinalOutputOpen: boolean;
  logFilterAgentIndex: number | null;
  agentsOrchestrationPaused: boolean;
  /** In-flight / failed task runs (Kanban execution). Reconciled on load with `tasks`. */
  taskExecution: Record<string, TaskExecutionState>;
  /** Versioned simulation presence (poses + orchestration); extend `ProjectSessionWire` for new fields. */
  session: ProjectSessionWire;
}

function numRecord<T>(obj: unknown, parseVal: (v: unknown) => T): Record<number, T> {
  const out: Record<number, T> = {};
  if (!obj || typeof obj !== 'object') return out;
  for (const [k, v] of Object.entries(obj as Record<string, unknown>)) {
    const n = Number(k);
    if (!Number.isNaN(n)) out[n] = parseVal(v);
  }
  return out;
}

function parseUsageLedger(raw: unknown): UsageLedgerEntry[] {
  if (!Array.isArray(raw)) return [];
  const out: UsageLedgerEntry[] = [];
  for (const item of raw) {
    if (!item || typeof item !== 'object') continue;
    const o = item as Record<string, unknown>;
    const kind = o.kind === 'final_media' || o.kind === 'chat' ? o.kind : 'chat';
    const id = typeof o.id === 'string' ? o.id : `fin_${Date.now()}_${out.length}`;
    out.push({
      id,
      timestamp: Number(o.timestamp) || 0,
      agentIndex: Number(o.agentIndex) || 0,
      agentName: typeof o.agentName === 'string' ? o.agentName : 'Agent',
      taskId: typeof o.taskId === 'string' ? o.taskId : undefined,
      kind,
      model: typeof o.model === 'string' ? o.model : '',
      promptTokens: Number(o.promptTokens) || 0,
      completionTokens: Number(o.completionTokens) || 0,
      totalTokens: Number(o.totalTokens) || 0,
      estimatedCostUsd: Number(o.estimatedCostUsd) || 0,
    });
  }
  return out;
}

function strRecord(obj: unknown): Record<string, LLMMessage[]> {
  const out: Record<string, LLMMessage[]> = {};
  if (!obj || typeof obj !== 'object') return out;
  for (const [k, v] of Object.entries(obj as Record<string, unknown>)) {
    if (Array.isArray(v)) out[k] = v as LLMMessage[];
  }
  return out;
}

const TASK_EXEC_STATUSES: TaskExecutionStatus[] = ['running', 'failed', 'succeeded', 'retry_queued'];

function normalizeTasksFromPersistence(tasks: Task[]): Task[] {
  return tasks
}

function parseTaskExecution(raw: unknown): Record<string, TaskExecutionState> {
  if (!raw || typeof raw !== 'object') return {};
  const out: Record<string, TaskExecutionState> = {};
  for (const [taskId, v] of Object.entries(raw as Record<string, unknown>)) {
    if (!taskId || typeof v !== 'object' || !v) continue;
    const o = v as Record<string, unknown>;
    const status = o.status;
    if (typeof status !== 'string' || !TASK_EXEC_STATUSES.includes(status as TaskExecutionStatus)) continue;
    out[taskId] = {
      taskId: typeof o.taskId === 'string' ? o.taskId : taskId,
      agentIndex: Number(o.agentIndex) || 0,
      status: status as TaskExecutionStatus,
      currentStep: typeof o.currentStep === 'string' ? o.currentStep : '',
      startedAt: Number(o.startedAt) || 0,
      updatedAt: Number(o.updatedAt) || 0,
      lastHeartbeatAt: Number(o.lastHeartbeatAt) || 0,
      attempts: Number(o.attempts) || 0,
      retryable: Boolean(o.retryable),
      lastError: typeof o.lastError === 'string' ? o.lastError : undefined,
    };
  }
  return out;
}

export function serializeProjectPayload(): PersistedProjectPayload {
  const s = useCoreStore.getState();
  return {
    selectedAgentSetId: useTeamStore.getState().selectedAgentSetId,
    userBrief: s.userBrief,
    referenceImages: s.referenceImages,
    phase: s.phase,
    finalOutput: s.finalOutput,
    availableModels: [...s.availableModels],
    totalTokenUsage: { ...s.totalTokenUsage },
    agentTokenUsage: { ...s.agentTokenUsage },
    totalEstimatedCost: s.totalEstimatedCost,
    agentEstimatedCost: { ...s.agentEstimatedCost },
    usageLedger: structuredClone(s.usageLedger),
    budgetLimitUsd: s.budgetLimitUsd,
    finalAssetType: s.finalAssetType,
    finalAssetContent: s.finalAssetContent,
    isGeneratingAsset: s.isGeneratingAsset,
    isReviewingOutput: s.isReviewingOutput,
    pendingOutputPrompt: s.pendingOutputPrompt,
    pendingOutputParams: s.pendingOutputParams,
    tasks: structuredClone(s.tasks),
    actionLog: structuredClone(s.actionLog),
    debugLog: structuredClone(s.debugLog) as DebugLogEntry[],
    agentHistories: structuredClone(s.agentHistories),
    chatReadVisibleLength: { ...s.chatReadVisibleLength },
    agentSummaries: { ...s.agentSummaries },
    boardroomHistories: structuredClone(s.boardroomHistories),
    isKanbanOpen: s.isKanbanOpen,
    isLogOpen: s.isLogOpen,
    isFinalOutputOpen: s.isFinalOutputOpen,
    logFilterAgentIndex: s.logFilterAgentIndex,
    agentsOrchestrationPaused: s.agentsOrchestrationPaused,
    taskExecution: structuredClone(s.taskExecution),
    session: readProjectSessionFromStores(),
  };
}

export function applyProjectPayload(raw: unknown): void {
  if (!raw || typeof raw !== 'object') return;
  const p = raw as Partial<PersistedProjectPayload>;
  const prePause = useCoreStore.getState().agentsOrchestrationPaused;

  if (typeof p.selectedAgentSetId === 'string' && p.selectedAgentSetId) {
    const current = useTeamStore.getState().selectedAgentSetId;
    if (current !== p.selectedAgentSetId) {
      setSkipProjectResetOnce(true);
      useTeamStore.getState().setActiveTeam(p.selectedAgentSetId);
    }
  }

  const tokenUsage = (v: unknown): LLMTokenUsage => {
    if (!v || typeof v !== 'object') {
      return { promptTokens: 0, completionTokens: 0, totalTokens: 0 };
    }
    const o = v as Record<string, number>;
    return {
      promptTokens: Number(o.promptTokens) || 0,
      completionTokens: Number(o.completionTokens) || 0,
      totalTokens: Number(o.totalTokens) || 0,
    };
  };

  const agentHistories = numRecord(p.agentHistories, (v) =>
    Array.isArray(v) ? (v as LLMMessage[]) : [],
  );
  const chatReadPartial =
    p.chatReadVisibleLength && typeof p.chatReadVisibleLength === 'object'
      ? numRecord(p.chatReadVisibleLength, (x) =>
          Number.isFinite(Number(x)) ? Number(x) : 0,
        )
      : undefined;

  const tasks = normalizeTasksFromPersistence(
    Array.isArray(p.tasks) ? (structuredClone(p.tasks) as Task[]) : [],
  );
  const taskExecution = reconcileTaskExecutionRecord(tasks, parseTaskExecution(p.taskExecution));

  useCoreStore.setState({
    userBrief: typeof p.userBrief === 'string' ? p.userBrief : '',
    referenceImages: Array.isArray(p.referenceImages) ? (p.referenceImages as string[]) : [],
    phase: (p.phase as ProjectPhase) || 'idle',
    finalOutput: typeof p.finalOutput === 'string' || p.finalOutput === null ? p.finalOutput : null,
    availableModels: Array.isArray(p.availableModels) ? [...p.availableModels] as string[] : [...GEMINI_CATALOG.chat.options],
    totalTokenUsage: tokenUsage(p.totalTokenUsage),
    agentTokenUsage: numRecord(p.agentTokenUsage, tokenUsage),
    totalEstimatedCost: typeof p.totalEstimatedCost === 'number' ? p.totalEstimatedCost : 0,
    agentEstimatedCost: numRecord(p.agentEstimatedCost, (x) => Number(x) || 0),
    usageLedger: parseUsageLedger(p.usageLedger),
    budgetLimitUsd:
      typeof p.budgetLimitUsd === 'number' && Number.isFinite(p.budgetLimitUsd) ? p.budgetLimitUsd : null,
    finalAssetType:
      p.finalAssetType === 'text' ||
      p.finalAssetType === 'image' ||
      p.finalAssetType === 'audio' ||
      p.finalAssetType === 'video'
        ? p.finalAssetType
        : 'text',
    finalAssetContent: typeof p.finalAssetContent === 'string' || p.finalAssetContent === null
      ? p.finalAssetContent
      : null,
    isGeneratingAsset: Boolean(p.isGeneratingAsset),
    isReviewingOutput: Boolean(p.isReviewingOutput),
    pendingOutputPrompt: typeof p.pendingOutputPrompt === 'string' ? p.pendingOutputPrompt : '',
    pendingOutputParams:
      p.pendingOutputParams && typeof p.pendingOutputParams === 'object'
        ? (p.pendingOutputParams as OutputGenerationParams)
        : {},
    tasks,
    actionLog: Array.isArray(p.actionLog) ? structuredClone(p.actionLog) : [],
    debugLog: Array.isArray(p.debugLog) ? structuredClone(p.debugLog) as DebugLogEntry[] : [],
    agentHistories,
    chatReadVisibleLength: backfillChatReadWatermarks(agentHistories, chatReadPartial),
    agentHistoryClearGeneration: {},
    agentSummaries: numRecord(p.agentSummaries, (x) => String(x ?? '')),
    boardroomHistories: strRecord(p.boardroomHistories),
    isKanbanOpen: p.isKanbanOpen !== false,
    isLogOpen: p.isLogOpen !== false,
    isFinalOutputOpen: Boolean(p.isFinalOutputOpen),
    logFilterAgentIndex:
      typeof p.logFilterAgentIndex === 'number' || p.logFilterAgentIndex === null
        ? p.logFilterAgentIndex
        : null,
    agentsOrchestrationPaused:
      typeof p.agentsOrchestrationPaused === 'boolean' ? p.agentsOrchestrationPaused : prePause,
    taskExecution,
  });

  applyProjectSessionToStores(p.session);
  clearProjectScopedUi();
}
