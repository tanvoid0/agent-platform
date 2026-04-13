
import React, { useEffect, useMemo, useState } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { getAgentSet, getAllAgents, getAllCharacters } from '../data/agents';
import { useUiOverlayInteraction } from '../integration/store/uiSelectors';
import InfoModal from './InfoModal';
import { AgentPresenceBadge } from './components/AgentPresenceBadge';

import { MessageSquareWarning, PartyPopper, Siren, Loader2 } from 'lucide-react';
import { Task, useCoreStore } from '../integration/store/coreStore';
import { useTeamStore, useActiveTeam } from '../integration/store/teamStore';
import { USER_COLOR, USER_COLOR_LIGHT, USER_COLOR_SOFT } from '../theme/brand';



interface AlertBubbleProps {
  icon: React.ReactNode;
  position: { x: number; y: number };
  visible: boolean;
  color?: string;
  onClick?: () => void;
}

const THOUGHT_GLYPHS = ['✦', '◇', '◎', '…', '?', '∴', '※', '○', '⌇', '⋰', '~', '✧', '◌', '⁂'];

/** Seeded shuffle so each agent’s cloud feels stable but still chaotic. */
function pickThoughtGlyphs(seed: number, count: number): string[] {
  let s = seed % 2147483647;
  const next = () => {
    s = (s * 48271) % 2147483647;
    return s;
  };
  const out: string[] = [];
  for (let i = 0; i < count; i++) {
    out.push(THOUGHT_GLYPHS[next() % THOUGHT_GLYPHS.length]!);
  }
  return out;
}

interface ThoughtCloudProps {
  position: { x: number; y: number };
  visible: boolean;
  agentIndex: number;
  tint: string;
}

const ThoughtCloud: React.FC<ThoughtCloudProps> = ({ position, visible, agentIndex, tint }) => {
  const [tick, setTick] = useState(0);

  useEffect(() => {
    if (!visible) return;
    const id = window.setInterval(() => setTick((t) => t + 1), 340);
    return () => window.clearInterval(id);
  }, [visible]);

  const glyphs = useMemo(() => pickThoughtGlyphs(agentIndex * 7919 + tick * 97, 5), [agentIndex, tick]);

  if (!visible) return null;

  return (
    <div
      className="absolute z-30 pointer-events-none flex flex-col items-center td-thought-cloud-anchor"
      style={{ left: position.x, top: position.y }}
    >
      <div
        className="relative rounded-[22px] border border-white/15 bg-darkDelegation/85 px-3 py-2 shadow-xl backdrop-blur-md animate-in fade-in zoom-in-95 duration-200"
        style={{ boxShadow: `0 0 28px ${tint}22` }}
      >
        <div className="flex items-center justify-center gap-1.5 min-h-[28px]">
          {glyphs.map((g, i) => (
            <span
              key={`${tick}-${i}`}
              className="text-[15px] leading-none text-white/90 select-none inline-block"
              style={{
                animation: `td-thought-glyph 0.55s ease-in-out ${i * 0.08}s infinite alternate`,
                color: i % 3 === 0 ? tint : undefined,
              }}
            >
              {g}
            </span>
          ))}
        </div>
        <div
          className="absolute left-1/2 -bottom-1.5 h-2.5 w-2.5 -translate-x-1/2 rotate-45 border-b border-r border-white/12 bg-darkDelegation/85"
          aria-hidden
        />
      </div>
    </div>
  );
};

const AlertBubble: React.FC<AlertBubbleProps> = ({ icon, position, visible, color = '#facc15', onClick }) => {
  if (!visible) return null;

  return (
    <div
      className={`absolute z-20 ${onClick ? 'pointer-events-auto cursor-pointer' : 'pointer-events-none'}`}
      style={{
        left: position.x,
        top: position.y,
        transform: 'translate(-50%, -100%) translateY(-10px)'
      }}
      onClick={(e) => {
        if (onClick) {
          e.stopPropagation();
          onClick();
        }
      }}
    >
      <div
        className={`bg-darkDelegation/90 backdrop-blur-md p-1.5 rounded-full border border-white/10 shadow-xl flex items-center justify-center hover:scale-110 active:scale-95 transition-transform ${onClick ? 'hover:border-white/30' : ''}`}
        style={{ color }}
      >
        {icon}
      </div>
    </div>
  );
};

type PhaseLabel = { text: string; className: string };

function getAgentPhaseLabel(
  agentIndex: number,
  leadAgentIndex: number,
  tasks: Task[],
  phase: string,
  isGeneratingAsset: boolean,
  orchestrationPaused: boolean,
  fallback: string,
): PhaseLabel {
  if (isGeneratingAsset && agentIndex === leadAgentIndex) {
    return { text: 'Delivering...', className: 'text-indigo-400 animate-pulse' };
  }
  if (agentIndex === leadAgentIndex && phase === 'done') {
    return { text: 'Project Ready!', className: 'text-yellow-400' };
  }
  if (phase === 'working' && orchestrationPaused) {
    return { text: 'Paused', className: 'text-amber-300' };
  }
  const holdTask = tasks.find(
    t =>
      t.assignedAgentId === agentIndex &&
      (t.status === 'review' || (t.status === 'scheduled' && t.requiresUserApproval)),
  );
  if (holdTask && phase !== 'done') {
    return { text: 'Approval Needed', className: 'text-[#7EACEA]' };
  }
  const activeTask = tasks.find(
    t => t.assignedAgentId === agentIndex && t.status === 'in_progress',
  );
  if (activeTask) {
    return { text: 'Working', className: 'text-emerald-400' };
  }
  return { text: fallback, className: 'text-white/70' };
}

const UIOverlay: React.FC = () => {
  const {
    selectedNpcIndex,
    selectedPosition,
    hoveredNpcIndex,
    hoveredPoiLabel,
    hoverPosition,
    npcScreenPositions,
    setSelectedNpc,
    isChatting,
    isThinking,
  } = useUiOverlayInteraction();
  const [isHelpOpen, setHelpOpen] = useState(false);
  const { tasks, phase, isGeneratingAsset, agentsOrchestrationPaused } = useCoreStore(
    useShallow((s) => ({
      tasks: s.tasks,
      phase: s.phase,
      isGeneratingAsset: s.isGeneratingAsset,
      agentsOrchestrationPaused: s.agentsOrchestrationPaused,
    })),
  );
  const system = useActiveTeam();
  const npcAgents = getAllAgents(system);
  const allPossibleAgents = getAllCharacters(system);

  const selectedAgent =
    selectedNpcIndex != null ? allPossibleAgents.find((a) => a.index === selectedNpcIndex) ?? null : null;
  const hoveredAgent =
    hoveredNpcIndex != null ? allPossibleAgents.find((a) => a.index === hoveredNpcIndex) ?? null : null;


  const thinkingNpc =
    isChatting && isThinking && selectedNpcIndex !== null ? allPossibleAgents.find((a) => a.index === selectedNpcIndex) : null;
  const thinkingPos =
    thinkingNpc && npcScreenPositions[thinkingNpc.index] ? npcScreenPositions[thinkingNpc.index] : null;

  return (
    <div className="absolute inset-0 pointer-events-none z-10 overflow-hidden select-none">
      <style>{`
        .td-thought-cloud-anchor {
          transform: translate(-50%, -100%) translateY(-56px);
          animation: td-thought-float 2.8s ease-in-out infinite;
        }
        @keyframes td-thought-float {
          0%, 100% { transform: translate(-50%, -100%) translateY(-54px); }
          50% { transform: translate(-50%, -100%) translateY(-62px); }
        }
        @keyframes td-thought-glyph {
          0% { transform: translateY(0) scale(0.92); opacity: 0.65; }
          100% { transform: translateY(-3px) scale(1.08); opacity: 1; }
        }
      `}</style>
      {thinkingNpc && thinkingPos && (
        <ThoughtCloud
          position={thinkingPos}
          visible
          agentIndex={thinkingNpc.index}
          tint={thinkingNpc.color}
        />
      )}

      {/* 1. Parallel Alert Bubbles System */}
      {npcAgents.map((agent) => {
        const pos = npcScreenPositions[agent.index];
        if (!pos) return null;

        // Condition: Alert disappears when hovered
        const isCurrentlyHovered = hoveredNpcIndex === agent.index || selectedNpcIndex === agent.index;
        if (isCurrentlyHovered) return null;

        let alertIcon: React.ReactNode = null;
        let alertColor = '#facc15'; // Default yellow

        // Check specific conditions
        // - Lead Agent (index 1) idle: siren
        if (agent.index === system.leadAgent.index && isGeneratingAsset) {
          alertIcon = <Loader2 size={18} className="animate-spin" />;
          alertColor = '#818cf8'; // Indigo-400
        }
        else if (agent.index === system.leadAgent.index && phase === 'idle') {
          alertIcon = <Siren size={18} />;
          alertColor = '#ffffff'; // White for siren
        }
        // - Lead Agent (index 1) project finished: party-popper
        else if (agent.index === system.leadAgent.index && phase === 'done') {
          alertIcon = <PartyPopper size={18} />;
          alertColor = '#facc15'; // Yellow
        }
        // - Any agent waiting for USER approval (target 0): message-square-warning
        else {
          const pendingTask = tasks.find(
            t =>
              t.assignedAgentId === agent.index &&
              (t.status === 'review' || (t.status === 'scheduled' && t.requiresUserApproval)),
          );
          if (pendingTask) {
            alertIcon = <MessageSquareWarning size={18} />;
            alertColor = USER_COLOR;
          }
        }

        if (!alertIcon) return null;

        return (
          <AlertBubble
            key={`alert-${agent.index}`}
            icon={alertIcon}
            position={pos}
            visible={true}
            color={alertColor}
            onClick={() => setSelectedNpc(agent.index)}
          />
        );
      })}

      {/* 2. Selection/Hover/Project Ready Bubble (Detailed) */}
      {(() => {
        // Priority 1: Selected Agent
        if (selectedAgent && selectedPosition) {
          const isLeadAgentProjectReady = selectedAgent.index === system.leadAgent.index && phase === 'done';
          const label = getAgentPhaseLabel(
            selectedAgent.index,
            system.leadAgent.index,
            tasks,
            phase,
            isGeneratingAsset,
            agentsOrchestrationPaused,
            '',
          );

          return (
            <div
              className="absolute z-25 pointer-events-none transition-all duration-75 ease-out"
              style={{
                left: selectedPosition.x,
                top: selectedPosition.y,
                transform: 'translate(-50%, -100%) translateY(-10px)'
              }}
            >
              <div className="bg-darkDelegation/90 backdrop-blur-md px-3 py-1.5 rounded-full border border-white/10 shadow-xl flex items-center gap-2 whitespace-nowrap animate-in fade-in zoom-in-95 duration-200">
                <div
                  className="w-2 h-2 rounded-full shrink-0"
                  style={{ backgroundColor: selectedAgent.color }}
                />
                <div className="flex items-center gap-1.5">
                  {selectedAgent.index === system.user.index ? (
                    <>
                      <span className="text-[10px] font-black uppercase tracking-widest text-white">
                        {selectedAgent.name} (You)
                      </span>
                      <span className="text-[10px] font-medium uppercase tracking-widest text-white/40">·</span>
                      <AgentPresenceBadge
                        agentIndex={selectedAgent.index}
                        size="sm"
                        variant="onDark"
                        className="!gap-1"
                      />
                    </>
                  ) : isLeadAgentProjectReady ? (
                    <span className={`text-[10px] font-black uppercase tracking-widest ${label.className}`}>
                      {label.text}
                    </span>
                  ) : (
                    <>
                      <span className="text-[10px] font-black uppercase tracking-widest text-white">
                        {selectedAgent.name}
                      </span>
                      {label.text ? (
                        <>
                          <span className="text-[10px] font-medium uppercase tracking-widest text-white/40">·</span>
                          <span className={`text-[10px] font-bold uppercase tracking-widest ${label.className}`}>
                            {label.text}
                          </span>
                        </>
                      ) : (
                        <>
                          <span className="text-[10px] font-medium uppercase tracking-widest text-white/40">·</span>
                          <AgentPresenceBadge
                            agentIndex={selectedAgent.index}
                            size="sm"
                            variant="onDark"
                            className="!gap-1"
                          />
                        </>
                      )}
                    </>
                  )}
                </div>
              </div>
            </div>
          );
        }

        // Priority 2: Hovered Agent with dynamic phase label (only if not selected)
        if (hoveredAgent && hoverPosition && hoveredNpcIndex !== selectedNpcIndex) {
          const isLeadAgentProjectReady = hoveredAgent.index === system.leadAgent.index && phase === 'done';
          const label = getAgentPhaseLabel(
            hoveredAgent.index,
            system.leadAgent.index,
            tasks,
            phase,
            isGeneratingAsset,
            agentsOrchestrationPaused,
            '',
          );

          return (
            <div
              className="absolute z-25 pointer-events-none transition-all duration-75 ease-out"
              style={{
                left: hoverPosition.x,
                top: hoverPosition.y,
                transform: 'translate(-50%, -100%) translateY(-10px)'
              }}
            >
              <div className="bg-darkDelegation/90 backdrop-blur-md px-3 py-1.5 rounded-full border border-white/10 shadow-xl flex items-center gap-2 whitespace-nowrap animate-in fade-in zoom-in-95 duration-200">
                <div
                  className="w-2 h-2 rounded-full shrink-0"
                  style={{ backgroundColor: hoveredAgent.color }}
                />
                <div className="flex items-center gap-1.5">
                  {hoveredAgent.index === system.user.index ? (
                    <>
                      <span className="text-[10px] font-black uppercase tracking-widest text-white">
                        {hoveredAgent.name} (You)
                      </span>
                      <span className="text-[10px] font-medium uppercase tracking-widest text-white/40">·</span>
                      <AgentPresenceBadge
                        agentIndex={hoveredAgent.index}
                        size="sm"
                        variant="onDark"
                        className="!gap-1"
                      />
                    </>
                  ) : isLeadAgentProjectReady ? (
                    <span className={`text-[10px] font-black uppercase tracking-widest ${label.className}`}>
                      {label.text}
                    </span>
                  ) : (
                    <>
                      <span className="text-[10px] font-black uppercase tracking-widest text-white">
                        {hoveredAgent.name}
                      </span>
                      {label.text ? (
                        <>
                          <span className="text-[10px] font-medium uppercase tracking-widest text-white/40">·</span>
                          <span className={`text-[10px] font-bold uppercase tracking-widest ${label.className}`}>
                            {label.text}
                          </span>
                        </>
                      ) : (
                        <>
                          <span className="text-[10px] font-medium uppercase tracking-widest text-white/40">·</span>
                          <AgentPresenceBadge
                            agentIndex={hoveredAgent.index}
                            size="sm"
                            variant="onDark"
                            className="!gap-1"
                          />
                        </>
                      )}
                    </>
                  )}
                </div>
              </div>
            </div>
          );
        }

        return null;
      })()}

      {/* POI Hover Bubble */}
      {hoveredPoiLabel && hoverPosition && (
        <div
          className="absolute z-10 pointer-events-none transition-all duration-75 ease-out"
          style={{
            left: hoverPosition.x,
            top: hoverPosition.y,
            transform: 'translate(-50%, -100%) translateY(-10px)'
          }}
        >
          <div className="bg-darkDelegation/90 backdrop-blur-md px-3 py-1.5 rounded-full border border-white/10 shadow-xl flex items-center gap-2 whitespace-nowrap animate-in fade-in zoom-in-95 duration-200">
            <span className="text-[10px] font-black uppercase tracking-widest text-white">{hoveredPoiLabel}</span>
          </div>
        </div>
      )}

      {/* Help Modal */}
      {isHelpOpen && <InfoModal onClose={() => setHelpOpen(false)} />}
    </div>
  );
};

export default UIOverlay;
