import { useShallow } from 'zustand/react/shallow';
import { useUiStore } from './uiStore';

/** Kanban task card: NPC focus + audit navigation. */
export function useUiKanbanTaskActions() {
  return useUiStore(
    useShallow((s) => ({
      setSelectedNpc: s.setSelectedNpc,
      setActiveAuditTaskId: s.setActiveAuditTaskId,
    })),
  );
}

/** Chat sidebar: typing and audit affordances tied to the selected NPC. */
export function useChatPanelUi() {
  return useUiStore(
    useShallow((s) => ({
      isChatting: s.isChatting,
      isThinking: s.isThinking,
      selectedNpcIndex: s.selectedNpcIndex,
      setIsTyping: s.setIsTyping,
      setActiveAuditTaskId: s.setActiveAuditTaskId,
    })),
  );
}

/** Inspector: which NPC is focused and whether chat mode is active. */
export function useInspectorNpcUi() {
  return useUiStore(
    useShallow((s) => ({
      selectedNpcIndex: s.selectedNpcIndex,
      isChatting: s.isChatting,
    })),
  );
}

/** Audit flow: reset selection and chat when task is resolved. */
export function useAuditModalUiActions() {
  return useUiStore(
    useShallow((s) => ({
      setSelectedNpc: s.setSelectedNpc,
      setChatting: s.setChatting,
    })),
  );
}

/** 2D overlay: NPC labels, hover, selection rings. */
export function useUiOverlayInteraction() {
  return useUiStore(
    useShallow((s) => ({
      selectedNpcIndex: s.selectedNpcIndex,
      selectedPosition: s.selectedPosition,
      hoveredNpcIndex: s.hoveredNpcIndex,
      hoveredPoiLabel: s.hoveredPoiLabel,
      hoverPosition: s.hoverPosition,
      npcScreenPositions: s.npcScreenPositions,
      setSelectedNpc: s.setSelectedNpc,
      isChatting: s.isChatting,
      isThinking: s.isThinking,
    })),
  );
}

/** Header: BYOK entry. */
export function useHeaderByokUi() {
  return useUiStore(
    useShallow((s) => ({
      isBYOKOpen: s.isBYOKOpen,
      setBYOKOpen: s.setBYOKOpen,
    })),
  );
}
