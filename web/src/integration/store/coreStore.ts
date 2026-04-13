import { create } from 'zustand';
import {
  createJSONStorage,
  persist,
  type PersistStorage,
} from 'zustand/middleware';
import type { OfficeVisualStyle } from '../../types';
import {
  MOCK_DELIVERABLE_IMAGE_BASE64,
  MOCK_MEDIA_DELIVERABLE_SENTINEL,
} from '../../core/llm/mockDeliverables';
import { getActiveChatCompletionSlice } from '../../core/llm/providerModelCatalog';
import { resolveChatModelForSession } from '../../core/llm/llmFacade';
import { accumulateUsageAfterResponse } from '../finance/usageLedger';
import { consumeSkipProjectResetOnce, getActiveAgentSet, useTeamStore } from './teamStore';
import { dispatchProposedPlanAmendment } from '../orchestration/proposedPlanAmendment';
import { useUiStore } from './uiStore';
import { useLlmSessionStore } from './llmSessionStore';
import {
  DEFAULT_ASSET_GENERATION_DEFAULTS,
  type CoreState,
  type DebugLogEntry,
  type MultimodalAssetBlockedPayload,
  type ProjectPhase,
  type Task,
  type TaskExecutionState,
} from './coreStoreTypes';
import {
  backfillChatReadWatermarks,
  visibleChatTurnCount,
} from '../chat/chatReadWatermark';
import { createProjectScopedCoreStorage } from './projectScopedStorage';
import { clearProjectScopedUi } from './uiStore';

export type {
  ActionLogEntry,
  AssetGenerationDefaults,
  DebugLogEntry,
  DebugLogEntryBase,
  MultimodalAssetBlockedPayload,
  MultimodalOutputKind,
  ProjectPhase,
  RequestDebugLogEntry,
  ResponseDebugLogEntry,
  Task,
  TaskExecutionState,
  TaskExecutionStatus,
  TaskRevision,
  TaskStatus,
} from './coreStoreTypes';
export type { UsageLedgerEntry, UsageLedgerKind } from './coreStoreTypes';
export {
  DEFAULT_ASSET_GENERATION_DEFAULTS,
  TASK_EXECUTION_STALE_MS,
} from './coreStoreTypes';
export type { OutputGenerationParams } from '../../core/llm/outputGenerationParams';

const uid = () => `${Date.now()}_${Math.random().toString(36).slice(2, 7)}`;

export const useCoreStore = create<CoreState>()(
  persist(
    (set, get) => ({
      userBrief: '',
      referenceImages: [],
      phase: 'idle',
      finalOutput: null,
      availableModels: [...getActiveChatCompletionSlice().options],
      totalTokenUsage: { promptTokens: 0, completionTokens: 0, totalTokens: 0 },
      agentTokenUsage: {},
      totalEstimatedCost: 0,
      agentEstimatedCost: {},
      usageLedger: [],
      budgetLimitUsd: null,
      budgetExceededOpen: false,
      budgetExceededMessage: '',
      finalAssetType: 'text',
      finalAssetContent: null,
      isGeneratingAsset: false,
      isReviewingOutput: false,
      pendingOutputPrompt: '',
      pendingOutputParams: {},
      multimodalAssetBlocked: null,
      multimodalDeliveryDeferred: false,
      tasks: [],
      actionLog: [],
      debugLog: [],
      taskExecution: {},
      agentHistories: {},
      chatReadVisibleLength: {},
      agentHistoryClearGeneration: {},
      agentSummaries: {},
      boardroomHistories: {},
      isKanbanOpen: true,
      isLogOpen: true,
      isFinalOutputOpen: false,
      logFilterAgentIndex: null,
      isResizing: false,
      officeVisualStyle: 'color',
      assetGenerationDefaults: { ...DEFAULT_ASSET_GENERATION_DEFAULTS },
      lastSyncedServerUpdatedAt: 0,
      agentsOrchestrationPaused: false,
      sessionPoseByAgent: {},
      sessionSceneRevision: 0,
      simSceneResetNonce: 0,

      bumpSimSceneReset: () => set((s) => ({ simSceneResetNonce: s.simSceneResetNonce + 1 })),

      setOfficeVisualStyle: (officeVisualStyle) => set({ officeVisualStyle }),
      setAssetGenerationDefaults: (patch) => {
        set((s) => ({
          assetGenerationDefaults: {
            image: { ...s.assetGenerationDefaults.image, ...patch.image },
            video: { ...s.assetGenerationDefaults.video, ...patch.video },
          },
        }));
      },

      resetProject: () => {
        clearProjectScopedUi();
        return set({
          userBrief: '',
          phase: 'idle',
          agentsOrchestrationPaused: false,
          finalOutput: null,
          tasks: [],
          actionLog: [],
          debugLog: [],
          taskExecution: {},
          agentHistories: {},
          chatReadVisibleLength: {},
          agentHistoryClearGeneration: {},
          agentSummaries: {},
          boardroomHistories: {},
          isFinalOutputOpen: false,
          totalTokenUsage: { promptTokens: 0, completionTokens: 0, totalTokens: 0 },
          agentTokenUsage: {},
          totalEstimatedCost: 0,
          agentEstimatedCost: {},
          finalAssetType: 'text',
          finalAssetContent: null,
          isGeneratingAsset: false,
          isReviewingOutput: false,
          pendingOutputPrompt: '',
          pendingOutputParams: {},
          multimodalAssetBlocked: null,
          multimodalDeliveryDeferred: false,
          referenceImages: [],
          lastSyncedServerUpdatedAt: 0,
          usageLedger: [],
          sessionPoseByAgent: {},
          sessionSceneRevision: 0,
        });
      },

      mergeSessionPosesFromSimCapture: (entries) =>
        set((s) => ({
          sessionPoseByAgent: { ...s.sessionPoseByAgent, ...entries },
        })),

      replaceSessionPosesFromAuthority: (poses) =>
        set((s) => ({
          sessionPoseByAgent: { ...poses },
          sessionSceneRevision: s.sessionSceneRevision + 1,
        })),

      setUserBrief: (brief) => set({ userBrief: brief }),
      addReferenceImage: (base64) => set((s) => ({ 
        referenceImages: [...s.referenceImages, base64].slice(0, 3) 
      })),
      removeReferenceImage: (index) => set((s) => ({ 
        referenceImages: s.referenceImages.filter((_, i) => i !== index) 
      })),
      clearReferenceImages: () => set({ referenceImages: [] }),
      setPhase: (phase) => set({ phase }),
      startProject: (brief) =>
        set({
          userBrief: brief,
          phase: 'working',
          agentsOrchestrationPaused: false,
          finalAssetType: 'text',
          finalAssetContent: null,
          multimodalDeliveryDeferred: false,
          multimodalAssetBlocked: null,
        }),
      setFinalOutput: (output) => set({ finalOutput: output }),
      setFinalAsset: (type, content) => set({ finalAssetType: type, finalAssetContent: content, isGeneratingAsset: false }),
      setIsGeneratingAsset: (isGenerating) => set({ isGeneratingAsset: isGenerating }),
      setReviewingOutput: (val) => set({ isReviewingOutput: val }),
      setPendingOutputPrompt: (prompt) => set({ pendingOutputPrompt: prompt }),
      setPendingOutputParams: (params) => set({ pendingOutputParams: params }),

      openMultimodalAssetBlocked: (payload) =>
        set({
          multimodalAssetBlocked: payload,
          isGeneratingAsset: false,
          isReviewingOutput: false,
        }),

      clearMultimodalAssetBlocked: () => set({ multimodalAssetBlocked: null }),

      resolveMultimodalAssetSkipped: () => {
        const b = get().multimodalAssetBlocked;
        if (!b) return;
        const label = b.outputType === 'music' ? 'audio' : b.outputType;
        const reason = b.reason ? ` ${b.reason}` : '';
        const footer = `\n\n---\n*Final **${label}** deliverable was **skipped**: selected generation backend is unavailable.${reason}*`;
        set({
          multimodalAssetBlocked: null,
          multimodalDeliveryDeferred: false,
          finalOutput: b.summaryPrompt + footer,
          finalAssetType: 'text',
          finalAssetContent: null,
          phase: 'done',
          isGeneratingAsset: false,
          isFinalOutputOpen: true,
        });
        get().addLogEntry({
          agentIndex: 1,
          action: `User skipped final ${b.outputType} (${b.backend || 'unknown'} backend unavailable).`,
          taskId: undefined,
        });
      },

      resolveMultimodalAssetMocked: () => {
        const b = get().multimodalAssetBlocked;
        if (!b) return;
        const reason = b.reason ? ` ${b.reason}` : '';
        const footer = `\n\n---\n*Below is a **mock** ${b.outputType} placeholder — selected generation backend is currently unavailable.${reason}*`;
        const shared = {
          multimodalAssetBlocked: null as MultimodalAssetBlockedPayload | null,
          multimodalDeliveryDeferred: false,
          finalOutput: b.summaryPrompt + footer,
          isGeneratingAsset: false,
          phase: 'done' as ProjectPhase,
          isFinalOutputOpen: true,
        };
        if (b.outputType === 'image') {
          set({
            ...shared,
            finalAssetType: 'image',
            finalAssetContent: MOCK_DELIVERABLE_IMAGE_BASE64,
          });
        } else if (b.outputType === 'music') {
          set({
            ...shared,
            finalAssetType: 'audio',
            finalAssetContent: MOCK_MEDIA_DELIVERABLE_SENTINEL,
          });
        } else {
          set({
            ...shared,
            finalAssetType: 'video',
            finalAssetContent: MOCK_MEDIA_DELIVERABLE_SENTINEL,
          });
        }
        get().addLogEntry({
          agentIndex: 1,
          action: `User accepted mock ${b.outputType} deliverable (${b.backend || 'unknown'} backend unavailable).`,
          taskId: undefined,
        });
      },

      resolveMultimodalAssetDeferred: () => {
        const b = get().multimodalAssetBlocked;
        set({
          multimodalAssetBlocked: null,
          multimodalDeliveryDeferred: true,
          isGeneratingAsset: false,
        });
        get().addLogEntry({
          agentIndex: 1,
          action: b
            ? `Delivery paused: final ${b.outputType} backend (${b.backend || 'unknown'}) unavailable. Auto-conclude suppressed until resolved or reset.`
            : 'Delivery paused pending cloud API configuration.',
          taskId: undefined,
        });
      },

      setBudgetLimitUsd: (budgetLimitUsd) => set({ budgetLimitUsd }),
      openBudgetExceeded: (message) =>
        set({
          budgetExceededOpen: true,
          budgetExceededMessage: message?.trim() || 'Project budget limit reached.',
        }),
      closeBudgetExceeded: () => set({ budgetExceededOpen: false, budgetExceededMessage: '' }),
      setAgentsOrchestrationPaused: (paused) => set({ agentsOrchestrationPaused: paused }),

      addTask: (task) => {
        const newTask: Task = {
          ...task,
          id: `task_${uid()}`,
          revisions: [],
          createdAt: Date.now(),
          updatedAt: Date.now(),
        }
        set((s) => ({ tasks: [...s.tasks, newTask] }))
        return newTask
      },

      removeTask: (taskId) =>
        set((s) => {
          const newTasks = s.tasks.filter((t) => t.id !== taskId);

          // Logic to check if removing this task finishes the project
          const hasRemainingTasks = newTasks.some(t => t.status !== 'done');
          const isWorking = s.phase === 'working';

          let nextPhase = s.phase;
          if (isWorking && !hasRemainingTasks) {
            nextPhase = 'done';
          }

          return {
            tasks: newTasks,
            phase: nextPhase,
          };
        }),

      updateTaskStatus: (taskId, status) =>
        set((s) => {
          const task = s.tasks.find((t) => t.id === taskId);
          if (!task) return {};

          // Safety check: Cannot move back into active columns if already 'done'
          if (
            task.status === 'done' &&
            (status === 'in_progress' ||
              status === 'review' ||
              status === 'on_hold' ||
              status === 'backlog' ||
              status === 'scheduled')
          ) {
            return {};
          }

          const newTasks = s.tasks.map((t) =>
            t.id === taskId ? { ...t, status, updatedAt: Date.now() } : t
          );

          return {
            tasks: newTasks,
          };
        }),

      setTaskBlocked: (taskId, reason) =>
        set((s) => {
          const task = s.tasks.find((t) => t.id === taskId);
          if (!task || task.status === 'done') return {};
          const note = reason?.trim();
          const description =
            note && !task.description.includes(note)
              ? `${task.description}\n\n[Blocked] ${note}`
              : task.description;
          const { [taskId]: _removed, ...taskExecution } = s.taskExecution;
          return {
            tasks: s.tasks.map((t) =>
              t.id === taskId ? { ...t, status: 'on_hold' as const, description, updatedAt: Date.now() } : t
            ),
            taskExecution,
          };
        }),

      submitTaskForReview: (taskId, draftOutput) =>
        set((s) => ({
          tasks: s.tasks.map((t) =>
            t.id === taskId ? { 
              ...t, 
              status: 'review', 
              draftOutput,
              updatedAt: Date.now() 
            } : t
          ),
        })),

      approveTask: (taskId) => {
        set((s) => {
          const task = s.tasks.find(t => t.id === taskId);
          if (task) useUiStore.getState().setAgentStatus(task.assignedAgentId, 'idle');
          
          return {
            tasks: s.tasks.map((t) =>
              t.id === taskId ? { 
                ...t, 
                status: 'done', 
                output: t.draftOutput || t.output,
                revisions: t.draftOutput 
                  ? [...t.revisions, { output: t.draftOutput, timestamp: Date.now() }] 
                  : t.revisions,
                draftOutput: undefined,
                updatedAt: Date.now() 
              } : t
            ),
          };
        });
      },

      approveProposedTask: (taskId) =>
        set((s) => {
          const task = s.tasks.find((t) => t.id === taskId);
          if (!task || task.status !== 'scheduled' || !task.requiresUserApproval) return {};
          return {
            tasks: s.tasks.map((t) =>
              t.id === taskId
                ? { ...t, requiresUserApproval: false, updatedAt: Date.now() }
                : t,
            ),
          };
        }),

      approveAllAwaitingUserInput: () =>
        set((s) => {
          const ui = useUiStore.getState();
          let any = false;
          const tasks = s.tasks.map((t) => {
            if (t.status === 'scheduled' && t.requiresUserApproval) {
              any = true;
              return { ...t, requiresUserApproval: false, updatedAt: Date.now() };
            }
            if (t.status === 'review') {
              any = true;
              ui.setAgentStatus(t.assignedAgentId, 'idle');
              return {
                ...t,
                status: 'done' as const,
                output: t.draftOutput || t.output,
                revisions: t.draftOutput
                  ? [...t.revisions, { output: t.draftOutput, timestamp: Date.now() }]
                  : t.revisions,
                draftOutput: undefined,
                updatedAt: Date.now(),
              };
            }
            return t;
          });
          if (!any) return {};
          return { tasks };
        }),

      amendProposedTaskPlan: (taskId, feedback) => {
        const trimmed = feedback.trim();
        if (!trimmed) return;

        let payload: {
          title: string;
          description: string;
          assignedAgentId: number;
          feedback: string;
        } | null = null;

        set((s) => {
          const task = s.tasks.find((t) => t.id === taskId);
          if (!task || task.status !== 'scheduled' || !task.requiresUserApproval) return {};

          const leadIndex = getActiveAgentSet().leadAgent.index;
          const history = s.agentHistories[leadIndex] || [];
          const systemLine = `[SYSTEM] User chose "Amend & replan" on a proposed task (removed from board). Title: "${task.title}". Their direction: ${trimmed}`;

          const newTasks = s.tasks.filter((t) => t.id !== taskId);
          const hasRemainingTasks = newTasks.some((t) => t.status !== 'done');
          const isWorking = s.phase === 'working';
          let nextPhase = s.phase;
          if (isWorking && !hasRemainingTasks) {
            nextPhase = 'done';
          }

          payload = {
            title: task.title,
            description: task.description,
            assignedAgentId: task.assignedAgentId,
            feedback: trimmed,
          };

          return {
            tasks: newTasks,
            phase: nextPhase,
            agentHistories: {
              ...s.agentHistories,
              [leadIndex]: [...history, { role: 'user' as const, content: systemLine }],
            },
          };
        });

        if (payload) {
          queueMicrotask(() => dispatchProposedPlanAmendment(payload!));
          get().addLogEntry({
            agentIndex: getActiveAgentSet().leadAgent.index,
            action: 'User requested amend & replan; lead will propose revised task(s).',
            taskId: undefined,
          });
        }
      },

      rejectTask: (taskId, comments) => {
        set((s) => {
          const task = s.tasks.find(t => t.id === taskId);
          if (!task) return {};

          useUiStore.getState().setAgentStatus(task.assignedAgentId, 'idle');
          
          const history = s.agentHistories[task.assignedAgentId] || [];
          const updatedHistory = [
            ...history,
            {
              role: 'user' as const,
              content: `Rejected. Reason: ${comments}`,
            }
          ];

          return {
            tasks: s.tasks.map((t) =>
              t.id === taskId ? { 
                ...t, 
                status: 'scheduled', 
                reviewComments: comments,
                revisions: t.draftOutput 
                  ? [...t.revisions, { output: t.draftOutput, feedback: comments, timestamp: Date.now() }] 
                  : t.revisions,
                draftOutput: undefined,
                updatedAt: Date.now() 
              } : t
            ),
            agentHistories: {
              ...s.agentHistories,
              [task.assignedAgentId]: updatedHistory
            }
          };
        });
      },

      setTaskOutput: (taskId, output) =>
        set((s) => ({
          tasks: s.tasks.map((t) =>
            t.id === taskId ? { ...t, output, updatedAt: Date.now() } : t
          ),
        })),

      addLogEntry: (entry) =>
        set((s) => ({
          actionLog: [
            ...s.actionLog,
            { ...entry, id: `log_${uid()}`, timestamp: Date.now() },
          ],
        })),
      
      addRequestLog: (entry) =>
        set((s) => {
          const newEntry: DebugLogEntry = { 
            ...entry, 
            id: `debug_${uid()}`, 
            timestamp: Date.now(),
            phase: 'request',
            status: 'completed'
          };
          const updated = [...s.debugLog, newEntry];
          return { debugLog: updated.length > 30 ? updated.slice(-30) : updated };
        }),

      addResponseLog: (entry) =>
        set((s) => {
          const newEntry: DebugLogEntry = { 
            ...entry, 
            id: `debug_${uid()}`, 
            timestamp: Date.now(),
            phase: 'response',
            status: 'completed'
          };
          const updated = [...s.debugLog, newEntry];

          let usagePatch: Partial<Pick<CoreState,
            'totalTokenUsage' | 'agentTokenUsage' | 'totalEstimatedCost' | 'agentEstimatedCost' | 'usageLedger'
          >> = {};

          if (entry.usage) {
            const modelName =
              (entry.raw as { model?: string } | undefined)?.model ||
              resolveChatModelForSession(useLlmSessionStore.getState().llmConfig, undefined);
            const merged = accumulateUsageAfterResponse(
              {
                totalTokenUsage: s.totalTokenUsage,
                agentTokenUsage: s.agentTokenUsage,
                totalEstimatedCost: s.totalEstimatedCost,
                agentEstimatedCost: s.agentEstimatedCost,
                usageLedger: s.usageLedger,
              },
              {
                agentIndex: entry.agentIndex,
                agentName: entry.agentName,
                taskId: entry.taskId,
                usage: entry.usage,
                raw: entry.raw,
                modelForPricing: modelName,
                ledgerId: `fin_${uid()}`,
                timestamp: newEntry.timestamp,
              }
            );
            usagePatch = merged;
          }

          return { 
            debugLog: updated.length > 30 ? updated.slice(-30) : updated,
            ...usagePatch,
          };
        }),

      markTaskExecutionRunning: ({ taskId, agentIndex, step, retrying = false }) =>
        set((s) => {
          const prev = s.taskExecution[taskId];
          const now = Date.now();
          const attempts = prev ? prev.attempts + (retrying ? 1 : 0) : 1;
          return {
            taskExecution: {
              ...s.taskExecution,
              [taskId]: {
                taskId,
                agentIndex,
                status: 'running',
                currentStep: step,
                startedAt: prev?.startedAt ?? now,
                updatedAt: now,
                lastHeartbeatAt: now,
                attempts,
                retryable: true,
                lastError: prev?.lastError,
              },
            },
          };
        }),

      heartbeatTaskExecution: (taskId, step) =>
        set((s) => {
          const prev = s.taskExecution[taskId];
          if (!prev) return {};
          const now = Date.now();
          return {
            taskExecution: {
              ...s.taskExecution,
              [taskId]: {
                ...prev,
                currentStep: step || prev.currentStep,
                updatedAt: now,
                lastHeartbeatAt: now,
              },
            },
          };
        }),

      markTaskExecutionFailed: (taskId, errorMessage, retryable = true) =>
        set((s) => {
          const prev = s.taskExecution[taskId];
          if (!prev) return {};
          const now = Date.now();
          return {
            taskExecution: {
              ...s.taskExecution,
              [taskId]: {
                ...prev,
                status: 'failed',
                retryable,
                lastError: errorMessage,
                currentStep: 'Blocked: retry needed',
                updatedAt: now,
              },
            },
          };
        }),

      markTaskExecutionSucceeded: (taskId, step = 'Completed') =>
        set((s) => {
          const prev = s.taskExecution[taskId];
          if (!prev) return {};
          const now = Date.now();
          return {
            taskExecution: {
              ...s.taskExecution,
              [taskId]: {
                ...prev,
                status: 'succeeded',
                currentStep: step,
                updatedAt: now,
                lastHeartbeatAt: now,
                retryable: false,
                lastError: undefined,
              },
            },
          };
        }),

      queueTaskExecutionRetry: (taskId, step = 'Retry queued') =>
        set((s) => {
          const prev = s.taskExecution[taskId];
          if (!prev) return {};
          const now = Date.now();
          return {
            taskExecution: {
              ...s.taskExecution,
              [taskId]: {
                ...prev,
                status: 'retry_queued',
                currentStep: step,
                updatedAt: now,
                lastHeartbeatAt: now,
              },
            },
          };
        }),

      appendAgentHistory: (agentIndex, role, parts) =>
        set((s) => {
          const nextHist = [
            ...(s.agentHistories[agentIndex] ?? []),
            {
              role,
              content: Array.isArray(parts)
                ? parts.map((p) => (typeof p === 'string' ? p : JSON.stringify(p))).join(' ')
                : String(parts),
            },
          ];
          const ui = useUiStore.getState();
          const viewing = ui.isChatting && ui.selectedNpcIndex === agentIndex;
          const vis = visibleChatTurnCount(nextHist);
          return {
            agentHistories: {
              ...s.agentHistories,
              [agentIndex]: nextHist,
            },
            ...(viewing
              ? {
                  chatReadVisibleLength: {
                    ...s.chatReadVisibleLength,
                    [agentIndex]: vis,
                  },
                }
              : {}),
          };
        }),

      setAgentSummary: (agentIndex, summary) =>
        set((s) => ({
          agentSummaries: {
            ...s.agentSummaries,
            [agentIndex]: summary
          }
        })),

      appendBoardroomHistory: (taskId, role, parts) =>
        set((s) => ({
          boardroomHistories: {
            ...s.boardroomHistories,
            [taskId]: [
              ...(s.boardroomHistories[taskId] ?? []),
              {
                role,
                content: Array.isArray(parts) ? parts.map(p => typeof p === 'string' ? p : JSON.stringify(p)).join(' ') : String(parts),
              },
            ],
          },
        })),

      clearAllHistories: () =>
        set({
          agentHistories: {},
          boardroomHistories: {},
          chatReadVisibleLength: {},
          agentHistoryClearGeneration: {},
        }),

      bumpAgentHistoryClearGeneration: (agentIndex) =>
        set((s) => ({
          agentHistoryClearGeneration: {
            ...s.agentHistoryClearGeneration,
            [agentIndex]: (s.agentHistoryClearGeneration[agentIndex] ?? 0) + 1,
          },
        })),

      markAgentChatRead: (agentIndex) =>
        set((s) => {
          const vis = visibleChatTurnCount(s.agentHistories[agentIndex]);
          return {
            chatReadVisibleLength: {
              ...s.chatReadVisibleLength,
              [agentIndex]: vis,
            },
          };
        }),

      setKanbanOpen: (open) => set({ isKanbanOpen: open }),
      setLogOpen: (open, filterAgent = null) =>
        set({ isLogOpen: open, logFilterAgentIndex: filterAgent ?? null }),
      setFinalOutputOpen: (open) => set({ isFinalOutputOpen: open }),
      setIsResizing: (resizing) => set({ isResizing: resizing }),

      setAgentHistory: (agentIndex, history) =>
        set((s) => {
          const ui = useUiStore.getState();
          const viewing = ui.isChatting && ui.selectedNpcIndex === agentIndex;
          const vis = visibleChatTurnCount(history);
          return {
            agentHistories: { ...s.agentHistories, [agentIndex]: history },
            ...(viewing
              ? {
                  chatReadVisibleLength: {
                    ...s.chatReadVisibleLength,
                    [agentIndex]: vis,
                  },
                }
              : {}),
          };
        }),
    }),
    {
      name: 'core-storage',
      storage: createJSONStorage(() => createProjectScopedCoreStorage()),
      merge: (persistedState, currentState) => {
        if (!persistedState || typeof persistedState !== 'object') return currentState;
        const { viewMode: _removed, ...rest } = persistedState as CoreState & { viewMode?: unknown };
        const merged = { ...currentState, ...rest };
        const h = merged.agentHistories ?? {};
        merged.chatReadVisibleLength = backfillChatReadWatermarks(
          h,
          merged.chatReadVisibleLength,
        );
        return merged;
      },
      partialize: (state) => ({
        userBrief: state.userBrief,
        referenceImages: state.referenceImages,
        phase: state.phase,
        finalOutput: state.finalOutput,
        availableModels: state.availableModels,
        totalTokenUsage: state.totalTokenUsage,
        agentTokenUsage: state.agentTokenUsage,
        totalEstimatedCost: state.totalEstimatedCost,
        agentEstimatedCost: state.agentEstimatedCost,
        usageLedger: state.usageLedger,
        budgetLimitUsd: state.budgetLimitUsd,
        finalAssetType: state.finalAssetType,
        finalAssetContent: state.finalAssetContent,
        isGeneratingAsset: state.isGeneratingAsset,
        isReviewingOutput: state.isReviewingOutput,
        pendingOutputPrompt: state.pendingOutputPrompt,
        pendingOutputParams: state.pendingOutputParams,
        tasks: state.tasks,
        actionLog: state.actionLog,
        debugLog: state.debugLog,
        taskExecution: state.taskExecution,
        agentHistories: state.agentHistories,
        chatReadVisibleLength: state.chatReadVisibleLength,
        agentSummaries: state.agentSummaries,
        boardroomHistories: state.boardroomHistories,
        isKanbanOpen: state.isKanbanOpen,
        officeVisualStyle: state.officeVisualStyle,
        assetGenerationDefaults: state.assetGenerationDefaults,
        isLogOpen: state.isLogOpen,
        isFinalOutputOpen: state.isFinalOutputOpen,
        logFilterAgentIndex: state.logFilterAgentIndex,
        lastSyncedServerUpdatedAt: state.lastSyncedServerUpdatedAt,
        agentsOrchestrationPaused: state.agentsOrchestrationPaused,
        sessionPoseByAgent: state.sessionPoseByAgent,
        sessionSceneRevision: state.sessionSceneRevision,
      }),
      onRehydrateStorage: () => (_state, error) => {
        if (!error) reconcileProjectStoreAfterLoad();
      },
    }
  )
)

function shouldDropRunningExecutionAfterLoad(
  task: Task | undefined,
  run: TaskExecutionState,
): boolean {
  if (!task) return true;
  if (run.status !== 'running' && run.status !== 'retry_queued') return false;
  return task.status !== 'in_progress';
}

/**
 * Drops stale `running` / `retry_queued` rows when the task is not `in_progress`
 * (e.g. after reload or server restore).
 */
export function reconcileTaskExecutionRecord(
  tasks: Task[],
  taskExecution: Record<string, TaskExecutionState>,
): Record<string, TaskExecutionState> {
  const taskById = new Map(tasks.map((t) => [t.id, t]));
  const nextExec = { ...taskExecution };
  let changed = false;
  for (const [taskId, run] of Object.entries(nextExec)) {
    const task = taskById.get(taskId);
    if (shouldDropRunningExecutionAfterLoad(task, run)) {
      delete nextExec[taskId];
      changed = true;
    }
  }
  return changed ? nextExec : taskExecution;
}

/**
 * After hydrating from server or localStorage, clear UI and in-flight execution flags that
 * cannot still be valid without an active browser session.
 */
export function reconcileProjectStoreAfterLoad(): void {
  useUiStore.getState().setThinking(false);

  const s = useCoreStore.getState();

  if (s.isGeneratingAsset && s.phase !== 'working') {
    useCoreStore.setState({ isGeneratingAsset: false });
  }

  const nextExec = reconcileTaskExecutionRecord(s.tasks, s.taskExecution);
  if (nextExec !== s.taskExecution) {
    useCoreStore.setState({ taskExecution: nextExec });
  }
}

// Sync resetProject whenever the active team changes (unless hydrating from persisted project)
useTeamStore.subscribe((state, prevState) => {
  if (state.selectedAgentSetId !== prevState.selectedAgentSetId) {
    if (consumeSkipProjectResetOnce()) return;
    useCoreStore.getState().resetProject();
  }
});

