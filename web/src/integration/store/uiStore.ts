import { create } from 'zustand';
import { getAllAgents } from '../../data/agents';
import { AgentState, SimulationUiState } from '../../types';
import { useTeamStore, getActiveAgentSet } from './teamStore';

let npcSpeechPulseTimer: ReturnType<typeof setTimeout> | null = null;

function clearNpcSpeechPulseTimer(): void {
  if (npcSpeechPulseTimer) {
    clearTimeout(npcSpeechPulseTimer);
    npcSpeechPulseTimer = null;
  }
}

/** Stop any active NPC “speaking” pulse (e.g. user sends another message mid-pulse). */
export function resetNpcSpeechPulseUi(): void {
  clearNpcSpeechPulseTimer();
  useUiStore.setState({ npcSpeechPulseActive: false });
}

/** Clears ephemeral chat UI so it cannot show another project’s conversation after switch/reset. */
export function clearProjectScopedUi(): void {
  clearNpcSpeechPulseTimer();
  useUiStore.setState({
    agentStatuses: {},
    isThinking: false,
    isChatting: false,
    isTyping: false,
    chatMessages: [],
    selectedNpcIndex: null,
    selectedPosition: null,
    activeAuditTaskId: null,
    npcSpeechPulseActive: false,
  });
}

export const useUiStore = create<SimulationUiState>()(
  (set) => ({
    isThinking: false,
    instanceCount: getAllAgents(getActiveAgentSet()).length + 1, // +1 for user

    selectedNpcIndex: null,
    selectedPosition: null,
    hoveredNpcIndex: null,
    hoveredPoiId: null,
    hoveredPoiLabel: null,
    hoverPosition: null,
    npcScreenPositions: {},
    isChatting: false,
    isTyping: false,
    npcSpeechPulseActive: false,
    chatMessages: [],
    inspectorTab: 'info',
    agentStatuses: {},
    setAgentStatus: (index: number, status: AgentState) => set((s) => ({
      agentStatuses: { ...s.agentStatuses, [index]: status }
    })),

    isBYOKOpen: false,
    byokError: null,
    setBYOKOpen: (open: boolean, error: string | null = null) =>
      set({ isBYOKOpen: open, byokError: error }),

    activeAuditTaskId: null,
    setActiveAuditTaskId: (taskId: string | null) => set({ activeAuditTaskId: taskId }),

    setThinking: (isThinking: boolean) => set({ isThinking }),
    setIsTyping: (isTyping: boolean) => set({ isTyping }),
    triggerNpcSpeechPulse: () => {
      clearNpcSpeechPulseTimer();
      set({ npcSpeechPulseActive: true });
      npcSpeechPulseTimer = setTimeout(() => {
        useUiStore.setState({ npcSpeechPulseActive: false });
        npcSpeechPulseTimer = null;
      }, 2600);
    },
    setInspectorTab: (tab: 'info' | 'chat') => set({ inspectorTab: tab }),
    setInstanceCount: (count: number) => set({ instanceCount: count }),

    setSelectedNpc: (index: number | null) => set({
      selectedNpcIndex: index,
      selectedPosition: null,
    }),
    setSelectedPosition: (pos: { x: number; y: number } | null) => set({ selectedPosition: pos }),
    setHoveredNpc: (index: number | null, pos: { x: number; y: number } | null) => set({
      hoveredNpcIndex: index,
      hoverPosition: pos,
      hoveredPoiId: null,
      hoveredPoiLabel: null,
    }),
    setHoveredPoi: (id: string | null, label: string | null, pos: { x: number; y: number } | null) => set({
      hoveredPoiId: id,
      hoveredPoiLabel: label,
      hoverPosition: pos,
      hoveredNpcIndex: null,
    }),
    setChatting: (isChatting: boolean) => {
      if (!isChatting) clearNpcSpeechPulseTimer();
      set((s) => ({
        isChatting,
        isTyping: isChatting ? s.isTyping : false,
        isThinking: isChatting ? s.isThinking : false,
        chatMessages: isChatting ? s.chatMessages : [],
        npcSpeechPulseActive: isChatting ? s.npcSpeechPulseActive : false,
      }));
    },

    consultantChatKick: 0,
    bumpConsultantChatKick: () => set((s) => ({ consultantChatKick: s.consultantChatKick + 1 })),
    chatInputFocusNonce: 0,
    bumpChatInputFocusRequest: () => set((s) => ({ chatInputFocusNonce: s.chatInputFocusNonce + 1 })),

    projectRailExpandRequestNonce: 0,
    bumpProjectRailExpandRequest: () =>
      set((s) => ({ projectRailExpandRequestNonce: s.projectRailExpandRequestNonce + 1 })),
  })
);

// Keep instanceCount in sync whenever the active agent set changes
useTeamStore.subscribe((state, prevState) => {
  if (state.selectedAgentSetId !== prevState.selectedAgentSetId) {
    const system = getActiveAgentSet();
    useUiStore.getState().setInstanceCount(getAllAgents(system).length + 1);
  }
});
