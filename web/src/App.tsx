/**
 * @license
 * SPDX-License-Identifier: Apache-2.0
 */

import React, { useCallback, useEffect, useRef, useState } from 'react';
import { GripHorizontal, Maximize2, Minimize2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { useCoreStore } from './integration/store/coreStore';
import { ActionLogPanel } from './interface/ActionLogPanel';
import { FinalOutputModal } from './interface/FinalOutputModal';
import Header from './interface/Header';
import InspectorPanel from './interface/InspectorPanel';
import { KanbanPanel } from './interface/KanbanPanel';
import { AuditModal } from './interface/AuditModal';
import { MultimodalAssetBlockedModal } from './interface/MultimodalAssetBlockedModal';
import { OutputReviewModal } from './interface/OutputReviewModal';
import SimulationView from './interface/SimulationView';
import { useUiStore } from './integration/store/uiStore';
import { SceneContext } from './simulation/SceneContext';
import { SceneManager } from './simulation/SceneManager';

const KANBAN_EXPANDED_STORAGE_KEY = 'ui:kanban-expanded';
const INSPECTOR_WIDTH_STORAGE_KEY = 'ui:inspector-width';
const RIGHT_PROJECT_RAIL_EXPANDED_KEY = 'ui:right-project-rail-expanded';
const LEFT_LOG_PANEL_WIDTH_STORAGE_KEY = 'ui:left-log-panel-width';
const LEFT_LOG_EXPANDED_KEY = 'ui:left-log-expanded';
const DEFAULT_INSPECTOR_WIDTH_PX = 320;
const INSPECTOR_WIDTH_MIN_PX = 260;
const INSPECTOR_WIDTH_MAX_PX = 720;
const INSPECTOR_RAIL_PX = 52;
const DEFAULT_LEFT_LOG_PANEL_TOTAL_PX = 320;
const LEFT_LOG_PANEL_MIN_PX = 200;
const LEFT_LOG_PANEL_MAX_PX = 520;

function readStoredInspectorWidth(): number {
  try {
    const raw = localStorage.getItem(INSPECTOR_WIDTH_STORAGE_KEY);
    const n = raw ? parseInt(raw, 10) : NaN;
    if (Number.isFinite(n) && n >= INSPECTOR_WIDTH_MIN_PX && n <= INSPECTOR_WIDTH_MAX_PX) {
      return n;
    }
  } catch {
    /* ignore */
  }
  return DEFAULT_INSPECTOR_WIDTH_PX;
}

function readStoredRightProjectRailExpanded(): boolean {
  try {
    const raw = localStorage.getItem(RIGHT_PROJECT_RAIL_EXPANDED_KEY);
    if (raw === null) return true;
    return raw === '1';
  } catch {
    return true;
  }
}

function readStoredLeftLogPanelWidth(): number {
  try {
    const raw = localStorage.getItem(LEFT_LOG_PANEL_WIDTH_STORAGE_KEY);
    const n = raw ? parseInt(raw, 10) : NaN;
    if (Number.isFinite(n) && n >= LEFT_LOG_PANEL_MIN_PX && n <= LEFT_LOG_PANEL_MAX_PX) {
      return n;
    }
  } catch {
    /* ignore */
  }
  return DEFAULT_LEFT_LOG_PANEL_TOTAL_PX;
}

function readStoredLeftLogExpanded(): boolean {
  try {
    const raw = localStorage.getItem(LEFT_LOG_EXPANDED_KEY);
    if (raw === null) return true;
    return raw === '1';
  } catch {
    return true;
  }
}

const App: React.FC = () => {
  const canvasRef = useRef<HTMLDivElement>(null);
  const managerRef = useRef<SceneManager | null>(null);
  const [sceneManager, setSceneManager] = useState<SceneManager | null>(null);
  const { isLogOpen, isKanbanOpen, setKanbanOpen, setIsResizing } = useCoreStore();
  const activeAuditTaskId = useUiStore((s) => s.activeAuditTaskId);
  const setActiveAuditTaskId = useUiStore((s) => s.setActiveAuditTaskId);
  const projectRailExpandRequestNonce = useUiStore((s) => s.projectRailExpandRequestNonce);

  const [isFullscreen, setIsFullscreen] = useState(false);
  const [kanbanHeight, setKanbanHeight] = useState(220);
  const [inspectorWidthPx, setInspectorWidthPx] = useState(readStoredInspectorWidth);
  const [rightProjectRailExpanded, setRightProjectRailExpanded] = useState(
    readStoredRightProjectRailExpanded,
  );
  const [leftLogPanelWidthPx, setLeftLogPanelWidthPx] = useState(readStoredLeftLogPanelWidth);
  const [leftLogExpanded, setLeftLogExpanded] = useState(readStoredLeftLogExpanded);
  const inspectorWidthRef = useRef(inspectorWidthPx);
  const inspectorResizeRef = useRef({ active: false, startX: 0, startWidth: DEFAULT_INSPECTOR_WIDTH_PX });
  const leftLogWidthRef = useRef(leftLogPanelWidthPx);
  const leftLogResizeRef = useRef({
    active: false,
    startX: 0,
    startWidth: DEFAULT_LEFT_LOG_PANEL_TOTAL_PX,
  });
  const [kanbanExpandedPref, setKanbanExpandedPref] = useState<boolean>(() => {
    try {
      return localStorage.getItem(KANBAN_EXPANDED_STORAGE_KEY) === '1';
    } catch {
      return false;
    }
  });
  const isKanbanExpanded = kanbanExpandedPref && isKanbanOpen && !isFullscreen;

  const toggleSimulationWorkspace = useCallback(() => {
    if (isFullscreen) return;
    if (isKanbanExpanded) {
      setKanbanExpandedPref(false);
    } else {
      setKanbanOpen(true);
      setKanbanExpandedPref(true);
    }
  }, [isFullscreen, isKanbanExpanded, setKanbanOpen]);

  const startResizing = useCallback(() => {
    setIsResizing(true);
  }, [setIsResizing]);

  const stopResizing = useCallback(() => {
    setIsResizing(false);
  }, [setIsResizing]);

  const resize = useCallback((e: MouseEvent) => {
    if (leftLogResizeRef.current.active) {
      const { startX, startWidth } = leftLogResizeRef.current;
      const delta = e.clientX - startX;
      const maxW = Math.min(
        LEFT_LOG_PANEL_MAX_PX,
        Math.floor(window.innerWidth * 0.58),
      );
      const next = Math.round(
        Math.min(maxW, Math.max(LEFT_LOG_PANEL_MIN_PX, startWidth + delta)),
      );
      setLeftLogPanelWidthPx(next);
      return;
    }
    if (inspectorResizeRef.current.active) {
      const { startX, startWidth } = inspectorResizeRef.current;
      const delta = startX - e.clientX;
      const maxW = Math.min(
        INSPECTOR_WIDTH_MAX_PX,
        Math.floor(window.innerWidth * 0.58),
      );
      const next = Math.round(
        Math.min(maxW, Math.max(INSPECTOR_WIDTH_MIN_PX, startWidth + delta)),
      );
      setInspectorWidthPx(next);
      return;
    }
    if (useCoreStore.getState().isResizing) {
      const windowHeight = window.innerHeight;
      const newHeight = windowHeight - e.clientY;
      const minHeight = windowHeight * 0.2;
      const maxHeight = windowHeight * 0.5;
      if (newHeight >= minHeight && newHeight <= maxHeight) {
        setKanbanHeight(newHeight);
      }
    }
  }, []);

  inspectorWidthRef.current = inspectorWidthPx;
  leftLogWidthRef.current = leftLogPanelWidthPx;

  const stopLeftLogResize = useCallback(() => {
    if (!leftLogResizeRef.current.active) return;
    leftLogResizeRef.current.active = false;
    document.body.style.cursor = '';
    document.body.style.userSelect = '';
    try {
      localStorage.setItem(LEFT_LOG_PANEL_WIDTH_STORAGE_KEY, String(leftLogWidthRef.current));
    } catch {
      /* ignore */
    }
  }, []);

  const stopInspectorResize = useCallback(() => {
    if (!inspectorResizeRef.current.active) return;
    inspectorResizeRef.current.active = false;
    document.body.style.cursor = '';
    document.body.style.userSelect = '';
    try {
      localStorage.setItem(INSPECTOR_WIDTH_STORAGE_KEY, String(inspectorWidthRef.current));
    } catch {
      /* ignore */
    }
  }, []);

  const startLeftLogResize = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    leftLogResizeRef.current = {
      active: true,
      startX: e.clientX,
      startWidth: leftLogWidthRef.current,
    };
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
  }, []);

  const startInspectorResize = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      inspectorResizeRef.current = {
        active: true,
        startX: e.clientX,
        startWidth: inspectorWidthRef.current,
      };
      document.body.style.cursor = 'col-resize';
      document.body.style.userSelect = 'none';
    },
    [],
  );

  useEffect(() => {
    const onMouseUp = () => {
      stopResizing();
      stopLeftLogResize();
      stopInspectorResize();
    };
    window.addEventListener('mousemove', resize);
    window.addEventListener('mouseup', onMouseUp);
    return () => {
      window.removeEventListener('mousemove', resize);
      window.removeEventListener('mouseup', onMouseUp);
    };
  }, [resize, stopResizing, stopLeftLogResize, stopInspectorResize]);

  useEffect(() => {
    try {
      localStorage.setItem(KANBAN_EXPANDED_STORAGE_KEY, kanbanExpandedPref ? '1' : '0');
    } catch {
      // Ignore write failures (private mode / restricted storage).
    }
  }, [kanbanExpandedPref]);

  useEffect(() => {
    try {
      localStorage.setItem(
        RIGHT_PROJECT_RAIL_EXPANDED_KEY,
        rightProjectRailExpanded ? '1' : '0',
      );
    } catch {
      /* ignore */
    }
  }, [rightProjectRailExpanded]);

  useEffect(() => {
    try {
      localStorage.setItem(LEFT_LOG_EXPANDED_KEY, leftLogExpanded ? '1' : '0');
    } catch {
      /* ignore */
    }
  }, [leftLogExpanded]);

  useEffect(() => {
    if (activeAuditTaskId) {
      document.body.style.overflow = 'hidden';
    } else {
      document.body.style.overflow = '';
    }
    return () => {
      document.body.style.overflow = '';
    };
  }, [activeAuditTaskId]);

  useEffect(() => {
    if (projectRailExpandRequestNonce > 0) {
      setRightProjectRailExpanded(true);
    }
  }, [projectRailExpandRequestNonce]);

  useEffect(() => {
    if (canvasRef.current && !managerRef.current) {
      const manager = new SceneManager(canvasRef.current);
      managerRef.current = manager;
      setSceneManager(manager);
    }

    return () => {
      if (managerRef.current) {
        managerRef.current.dispose();
        managerRef.current = null;
        setSceneManager(null);
      }
    };
  }, []);

  const inspectorColumnWidthPx = rightProjectRailExpanded ? inspectorWidthPx : INSPECTOR_RAIL_PX;

  return (
    <SceneContext.Provider value={sceneManager}>
      <div className="w-screen h-screen bg-white overflow-hidden flex flex-col">
        {/* Top: Header */}
        {!isFullscreen && <Header />}

        <div className="flex-1 flex flex-row min-h-0 min-w-0 overflow-hidden">
          {/* Left: Log panel */}
          {isLogOpen && !isFullscreen && (
            <div className="flex h-full shrink-0">
              <ActionLogPanel
                leftLogExpanded={leftLogExpanded}
                onLeftLogExpandedChange={setLeftLogExpanded}
                expandedTotalWidthPx={leftLogPanelWidthPx}
              />
              {leftLogExpanded && (
                <div
                  role="separator"
                  aria-orientation="vertical"
                  aria-label="Resize action log sidebar"
                  title="Drag to resize sidebar"
                  onMouseDown={startLeftLogResize}
                  className="relative z-40 w-2 shrink-0 cursor-col-resize bg-zinc-200/40 hover:bg-zinc-300/90 active:bg-zinc-400/90"
                />
              )}
            </div>
          )}

          {/* Center: canvas + kanban drawer stacked */}
          <div className="relative flex-1 flex flex-col min-w-0 min-h-0 overflow-hidden bg-zinc-50">

            {/* Simulation Context - Persistently Mounted */}
            <div className="flex-1 flex flex-col min-w-0 min-h-0">
              <SimulationView
                canvasRef={canvasRef}
                isFullscreen={isFullscreen}
                setIsFullscreen={setIsFullscreen}
                simulationCollapsed={isKanbanExpanded}
                onToggleSimulationCollapsed={toggleSimulationWorkspace}
              />

              {/* Split view: resize bar + expand todo board */}
              {isKanbanOpen && !isFullscreen && !isKanbanExpanded && (
                <div
                  className={`z-30 flex h-9 shrink-0 items-center gap-2 border-t border-black/5 bg-white px-2 pl-3 transition-colors ${
                    useCoreStore.getState().isResizing ? 'bg-zinc-50' : ''
                  }`}
                >
                  <div
                    className="flex min-h-0 min-w-0 flex-1 cursor-row-resize items-center gap-2"
                    onMouseDown={startResizing}
                    aria-label="Drag to resize the office and todo board split"
                    role="separator"
                  >
                    <GripHorizontal className="size-4 shrink-0 text-zinc-300" aria-hidden />
                    <p className="min-w-0 flex-1 text-[10px] leading-snug text-zinc-500">
                      <span className="font-semibold text-zinc-600">Split view</span>
                      <span className="text-zinc-400">
                        {' '}
                        — drag to resize · use the eye in the office bar to hide or show the 3D view
                      </span>
                    </p>
                    <div className="hidden h-1 w-12 shrink-0 rounded-full bg-zinc-200 sm:block" aria-hidden />
                  </div>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    className="shrink-0 text-zinc-600"
                    title="Expand todo board"
                    aria-label="Expand todo board"
                    onMouseDown={(e) => e.stopPropagation()}
                    onClick={() => setKanbanExpandedPref(true)}
                  >
                    <Maximize2 className="size-4" />
                  </Button>
                </div>
              )}

              {/* Full-width todo board: minimize to show simulation again */}
              {isKanbanOpen && !isFullscreen && isKanbanExpanded && (
                <div className="z-30 flex h-9 shrink-0 items-center justify-end gap-2 border-t border-black/5 bg-white px-2 pl-3">
                  <p className="min-w-0 flex-1 text-[10px] leading-snug text-zinc-500">
                    <span className="font-semibold text-zinc-600">Todo board</span>
                    <span className="text-zinc-400"> — full height · restore split view below</span>
                  </p>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    className="shrink-0 text-zinc-600"
                    title="Show simulation again"
                    aria-label="Show simulation again"
                    onClick={() => setKanbanExpandedPref(false)}
                  >
                    <Minimize2 className="size-4" />
                  </Button>
                </div>
              )}

              {isKanbanOpen && !isFullscreen && (
                <KanbanPanel height={kanbanHeight} expanded={isKanbanExpanded} />
              )}
            </div>
          </div>

          {/* Right: resize handle + inspector sidebar */}
          {!isFullscreen && (
            <div className="flex h-full shrink-0">
              {rightProjectRailExpanded && (
                <div
                  role="separator"
                  aria-orientation="vertical"
                  aria-label="Resize project and chat sidebar"
                  title="Drag to resize sidebar"
                  onMouseDown={startInspectorResize}
                  className="relative z-40 w-2 shrink-0 cursor-col-resize bg-zinc-200/40 hover:bg-zinc-300/90 active:bg-zinc-400/90"
                />
              )}
              <div
                className="flex min-h-0 min-w-0 flex-col overflow-hidden"
                style={{ width: inspectorColumnWidthPx }}
              >
                <InspectorPanel
                  projectRailExpanded={rightProjectRailExpanded}
                  onProjectRailExpanded={setRightProjectRailExpanded}
                />
              </div>
            </div>
          )}
        </div>

        {/* Final output — fixed viewport overlay */}
        <FinalOutputModal />
        <OutputReviewModal />
        {activeAuditTaskId && (
          <AuditModal
            isOpen={!!activeAuditTaskId}
            taskId={activeAuditTaskId}
            onClose={() => setActiveAuditTaskId(null)}
          />
        )}
        <MultimodalAssetBlockedModal />
      </div>
    </SceneContext.Provider>
  );
};

export default App;

