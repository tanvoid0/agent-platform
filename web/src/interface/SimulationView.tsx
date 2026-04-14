import { Eye, EyeOff, Maximize2, Minimize2, Pause, Play, Power, Waypoints } from 'lucide-react';
import React, { useEffect, useRef, useState } from 'react';
import { Button } from '@/components/ui/button';
import { CONSULTANT_WORKSHOP_TEAM_ID, getAgentSet } from '../data/agents';
import { hasRemoteProjectBackend } from '../integration/api/projectRemoteApi';
import { createAndSwitchToNewProject, persistClearedProjectWorkspace } from '../integration/projectPersistence';
import { useCoreStore } from '../integration/store/coreStore';
import { useActiveTeam, useTeamStore } from '../integration/store/teamStore';
import { useUiStore } from '../integration/store/uiStore';
import { useSceneManager } from '../simulation/SceneContext';
import InspectorPanel from './InspectorPanel';
import UIOverlay from './UIOverlay';
import TeamFlowModal from './TeamFlowModal';
import ResetModal from './ResetModal';
import { TeamBadge } from './components/TeamBadge';
import { TeamOutputBadge } from './components/TeamOutputBadge';
import { OfficeAppearanceToolbar } from './components/OfficeAppearanceToolbar';

interface SimulationViewProps {
  canvasRef: React.RefObject<HTMLDivElement>;
  isFullscreen: boolean;
  setIsFullscreen: (value: boolean) => void;
  /** When true, the 3D view is collapsed so the todo board can use the workspace (keeps canvas mounted). */
  simulationCollapsed?: boolean;
  /** Hide or show the 3D canvas (toolbar stays visible). */
  onToggleSimulationCollapsed?: () => void;
}

const SimulationView: React.FC<SimulationViewProps> = ({
  canvasRef,
  isFullscreen,
  setIsFullscreen,
  simulationCollapsed = false,
  onToggleSimulationCollapsed,
}) => {
  const selectedNpcIndex = useUiStore((s) => s.selectedNpcIndex);
  const phase = useCoreStore((s) => s.phase);
  const agentsOrchestrationPaused = useCoreStore((s) => s.agentsOrchestrationPaused);
  const setAgentsOrchestrationPaused = useCoreStore((s) => s.setAgentsOrchestrationPaused);
  const resetProject = useCoreStore((s) => s.resetProject);
  const bumpSimSceneReset = useCoreStore((s) => s.bumpSimSceneReset);
  const scene = useSceneManager();
  const activeSet = useActiveTeam();
  const [isFlowModalOpen, setIsFlowModalOpen] = useState(false);
  const [isResetModalOpen, setIsResetModalOpen] = useState(false);
  const serverProjectsEnabled = hasRemoteProjectBackend();

  const handleShutDownClearThisProject = async () => {
    scene?.resetScene();
    resetProject();
    bumpSimSceneReset();
    await persistClearedProjectWorkspace();
  };

  const handleShutDownStartFresh = async (userTitle: string) => {
    await createAndSwitchToNewProject(userTitle);
    scene?.resetScene();
  };

  const simSceneResetNonce = useCoreStore((s) => s.simSceneResetNonce);
  const consultantChatKick = useUiStore((s) => s.consultantChatKick);
  const lastConsultantKickApplied = useRef(0);

  useEffect(() => {
    if (simSceneResetNonce > 0) {
      scene?.resetScene();
    }
  }, [simSceneResetNonce, scene]);

  useEffect(() => {
    if (!scene || consultantChatKick === 0 || consultantChatKick === lastConsultantKickApplied.current) return;
    lastConsultantKickApplied.current = consultantChatKick;
    const { selectedAgentSetId, customSystems } = useTeamStore.getState();
    if (selectedAgentSetId !== CONSULTANT_WORKSHOP_TEAM_ID) return;
    const leadIdx = getAgentSet(CONSULTANT_WORKSHOP_TEAM_ID, customSystems).leadAgent.index;

    useUiStore.getState().setChatting(false);
    requestAnimationFrame(() => {
      scene.startChat(leadIdx);
      useUiStore.getState().bumpChatInputFocusRequest();
    });
  }, [scene, consultantChatKick]);

  return (
    <div
      className={`flex flex-col min-w-0 min-h-0 relative ${
        simulationCollapsed ? 'flex-none shrink-0' : 'flex-1'
      }`}
    >
      {/* Simulation View Header */}
      <div className="h-14 border-b border-black/5 flex items-center justify-between px-5 bg-white shrink-0">
        <div className="flex-1 min-w-0 flex items-center gap-4 overflow-x-auto">
          <Button
            type="button"
            variant="ghost"
            onClick={() => setIsFlowModalOpen(true)}
            className="group flex h-auto shrink-0 items-center gap-4 rounded-2xl px-2.5 py-1.5 font-normal hover:bg-zinc-50 active:scale-95"
            title="View Team Flow"
          >
            <TeamBadge system={activeSet} />
            <div className="flex size-8 items-center justify-center rounded-full border border-zinc-100 text-zinc-300 transition-colors group-hover:border-zinc-200 group-hover:text-darkDelegation">
              <Waypoints size={14} />
            </div>
          </Button>

          <TeamOutputBadge system={activeSet} className="hidden md:flex shrink-0" />
        </div>

        <div className="flex items-center justify-end gap-1 sm:gap-1.5 shrink-0">
          <OfficeAppearanceToolbar className="flex items-center gap-1" />
          {phase === 'working' && (
            <>
              {agentsOrchestrationPaused ? (
                <Button
                  type="button"
                  variant="ghost"
                  onClick={() => setAgentsOrchestrationPaused(false)}
                  className="flex h-auto items-center gap-1.5 rounded-xl px-2 py-1.5 text-emerald-700 hover:bg-emerald-50 sm:px-2.5"
                  title="Resume — agents continue autonomous work"
                >
                  <Play size={16} strokeWidth={2.5} />
                  <span className="hidden text-[9px] font-black uppercase tracking-widest sm:inline">Resume</span>
                </Button>
              ) : (
                <Button
                  type="button"
                  variant="ghost"
                  onClick={() => setAgentsOrchestrationPaused(true)}
                  className="flex h-auto items-center gap-1.5 rounded-xl px-2 py-1.5 text-zinc-500 hover:bg-amber-50 hover:text-amber-800 sm:px-2.5"
                  title="Pause — stops new LLM turns and tool actions while paused. A request already in flight may still finish billing once."
                >
                  <Pause size={16} strokeWidth={2.5} />
                  <span className="hidden text-[9px] font-black uppercase tracking-widest sm:inline">Pause</span>
                </Button>
              )}
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                onClick={() => setIsResetModalOpen(true)}
                className="text-zinc-400 hover:text-red-600"
                title="Shut down project — reset this project or start a new one"
              >
                <Power size={16} strokeWidth={2.5} />
              </Button>
            </>
          )}
          {onToggleSimulationCollapsed && (
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              onClick={onToggleSimulationCollapsed}
              disabled={isFullscreen}
              className={
                isFullscreen
                  ? 'cursor-not-allowed text-zinc-200'
                  : simulationCollapsed
                    ? 'text-emerald-600 hover:bg-emerald-50 hover:text-emerald-700'
                    : 'text-zinc-500 hover:bg-zinc-50 hover:text-darkDelegation'
              }
              title={
                isFullscreen
                  ? 'Exit fullscreen first to show or hide the office'
                  : simulationCollapsed
                    ? 'Show office — todo board shares the workspace again'
                    : 'Hide office — todo board uses the full workspace (simulation stays loaded)'
              }
              aria-expanded={!simulationCollapsed}
              aria-label={
                simulationCollapsed ? 'Show office simulation' : 'Hide office simulation'
              }
            >
              {simulationCollapsed ? (
                <EyeOff size={18} strokeWidth={2.25} />
              ) : (
                <Eye size={18} strokeWidth={2.25} />
              )}
            </Button>
          )}
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            onClick={() => setIsFullscreen(!isFullscreen)}
            className="text-zinc-400 hover:text-darkDelegation"
            title={isFullscreen ? "Exit Fullscreen" : "Fullscreen Panel"}
          >
            {isFullscreen ? <Minimize2 size={16} /> : <Maximize2 size={16} />}
          </Button>
        </div>
      </div>

      <div
        ref={canvasRef}
        className={`relative overflow-hidden bg-black/5 ${
          simulationCollapsed ? 'h-0 flex-none min-h-0' : 'flex-1 min-h-0'
        }`}
        aria-hidden={simulationCollapsed || undefined}
      >
        <UIOverlay />
        {isFullscreen && selectedNpcIndex !== null && (
          <div className="absolute top-4 right-4 bottom-4 w-96 z-50 pointer-events-none flex flex-col gap-4">
            <InspectorPanel isFloating />
          </div>
        )}
      </div>

      {isFlowModalOpen && (
        <TeamFlowModal
          isOpen={isFlowModalOpen}
          onClose={() => setIsFlowModalOpen(false)}
          system={activeSet}
        />
      )}

      <ResetModal
        isOpen={isResetModalOpen}
        onClose={() => setIsResetModalOpen(false)}
        serverProjectsEnabled={serverProjectsEnabled}
        onConfirmClearThisProject={handleShutDownClearThisProject}
        onConfirmStartFreshProject={serverProjectsEnabled ? handleShutDownStartFresh : undefined}
      />
    </div>
  );
};

export default SimulationView;
