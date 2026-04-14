import { Circle, CircleOff, MessageCircle, Minus, Moon } from 'lucide-react';
import React, { useEffect, useMemo, useState } from 'react';
import { InfoTooltip } from './InfoTooltip';
import type { PresenceKind } from '../../integration/presence/resolveAgentPresence';
import { resolveAgentPresence } from '../../integration/presence/resolveAgentPresence';
import { useCoreStore } from '../../integration/store/coreStore';
import { useActiveTeam } from '../../integration/store/teamStore';
import { useUiStore } from '../../integration/store/uiStore';

function useViewerTabHidden(): boolean {
  const [hidden, setHidden] = useState(
    () => typeof document !== 'undefined' && document.visibilityState === 'hidden',
  );
  useEffect(() => {
    const onVis = () => setHidden(document.visibilityState === 'hidden');
    document.addEventListener('visibilitychange', onVis);
    return () => document.removeEventListener('visibilitychange', onVis);
  }, []);
  return hidden;
}

export function useAgentPresence(agentIndex: number | null) {
  const team = useActiveTeam();
  const humanIndex = team.user.index;
  const viewerTabHidden = useViewerTabHidden();

  const agentState = useUiStore((s) => (agentIndex !== null ? s.agentStatuses[agentIndex] : undefined));
  const isThinking = useUiStore((s) => s.isThinking);
  const isSpeechPulse = useUiStore((s) => s.npcSpeechPulseActive);
  const isChatting = useUiStore((s) => s.isChatting);
  const selectedNpc = useUiStore((s) => s.selectedNpcIndex);
  const orchestrationPaused = useCoreStore((s) => s.agentsOrchestrationPaused);

  return useMemo(() => {
    if (agentIndex === null) {
      return resolveAgentPresence({
        agentState: undefined,
        viewerTabHidden,
        isHuman: false,
        isChatOpenWithAgent: false,
        isThinking: false,
        isSpeechPulse: false,
        orchestrationPaused,
      });
    }
    const isHuman = agentIndex === humanIndex;
    const chatHere = isChatting && selectedNpc === agentIndex;
    return resolveAgentPresence({
      agentState,
      viewerTabHidden,
      isHuman,
      isChatOpenWithAgent: chatHere,
      isThinking,
      isSpeechPulse,
      orchestrationPaused,
    });
  }, [
    agentIndex,
    agentState,
    humanIndex,
    isThinking,
    isSpeechPulse,
    isChatting,
    selectedNpc,
    orchestrationPaused,
    viewerTabHidden,
  ]);
}

const PRESENCE_ICONS: Record<
  PresenceKind,
  React.ComponentType<{ className?: string; strokeWidth?: number; size?: number }>
> = {
  online: Circle,
  away: Moon,
  busy: Minus,
  active: MessageCircle,
  offline: CircleOff,
};

const KIND_ICON_CLASS: Record<PresenceKind, string> = {
  online: 'text-emerald-600',
  away: 'text-amber-500',
  busy: 'text-red-500',
  active: 'text-emerald-600',
  offline: 'text-zinc-400',
};

export const AgentPresenceBadge: React.FC<{
  agentIndex: number | null;
  size?: 'sm' | 'md';
  variant?: 'default' | 'onDark';
  className?: string;
  /** Pip / icon only; label shown via title and aria-label (narrow sidebars). */
  compact?: boolean;
}> = ({ agentIndex, size = 'md', variant = 'default', className = '', compact = false }) => {
  const p = useAgentPresence(agentIndex);
  const Icon = PRESENCE_ICONS[p.kind];
  const iconPx = size === 'sm' ? 12 : 14;
  const textClass =
    size === 'sm'
      ? variant === 'onDark'
        ? 'text-[9px] text-white/80'
        : 'text-[9px] text-zinc-500'
      : variant === 'onDark'
        ? 'text-[10px] text-white/80'
        : 'text-[10px] text-zinc-500';
  const iconClass =
    variant === 'onDark'
      ? p.kind === 'online'
        ? 'text-emerald-300'
        : p.kind === 'away'
          ? 'text-amber-300'
          : p.kind === 'busy'
            ? 'text-red-300'
            : p.kind === 'active'
              ? 'text-emerald-200'
              : 'text-zinc-300'
      : KIND_ICON_CLASS[p.kind];

  const pipOrIcon =
    p.kind === 'online' ? (
      <span
        className={`rounded-full shrink-0 border border-black/10 shadow-sm ${variant === 'onDark' ? 'bg-emerald-300' : 'bg-emerald-500'} ${size === 'sm' ? 'size-2' : 'size-2.5'} ${p.pulse ? 'animate-pulse' : ''}`}
        aria-hidden
      />
    ) : (
      <Icon
        className={`shrink-0 ${iconClass} ${p.pulse ? 'animate-pulse' : ''}`}
        strokeWidth={2.25}
        size={iconPx}
        aria-hidden
      />
    );

  if (compact) {
    return (
      <InfoTooltip
        text={`${p.label} — simulation/UI presence (not “last seen” on the server).`}
        maxWidth={240}
      >
        <span
          className={`inline-flex items-center justify-center shrink-0 ${className}`}
          aria-label={p.label}
        >
          {pipOrIcon}
        </span>
      </InfoTooltip>
    );
  }

  return (
    <span
      className={`inline-flex items-center gap-1.5 max-w-full min-w-0 ${className}`}
      title={p.label}
    >
      {pipOrIcon}
      <span className={`font-black uppercase tracking-widest truncate ${textClass}`}>{p.label}</span>
    </span>
  );
};
