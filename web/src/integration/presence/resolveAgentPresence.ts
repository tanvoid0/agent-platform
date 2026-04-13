import type { AgentState } from '../../types';

export type PresenceKind = 'online' | 'away' | 'busy' | 'active' | 'offline';

export interface ResolveAgentPresenceInput {
  agentState: AgentState | undefined;
  /** True when the viewer’s tab is hidden — used for human user “AFK”. */
  viewerTabHidden: boolean;
  isHuman: boolean;
  isChatOpenWithAgent: boolean;
  isThinking: boolean;
  isSpeechPulse: boolean;
  orchestrationPaused: boolean;
}

/**
 * Slack-style presence from simulation/UI state. Task-heavy labels are handled
 * elsewhere (e.g. world bubbles); this is for “Online / Away / Busy / …” affordances.
 */
export function resolveAgentPresence(input: ResolveAgentPresenceInput): {
  kind: PresenceKind;
  label: string;
  /** Tailwind bg-* for the status pip */
  pipClass: string;
  pulse: boolean;
} {
  if (input.isHuman) {
    if (input.viewerTabHidden) {
      return { kind: 'away', label: 'Away', pipClass: 'bg-amber-400', pulse: false };
    }
    if (input.isChatOpenWithAgent) {
      if (input.isThinking) {
        return { kind: 'busy', label: 'Thinking', pipClass: 'bg-amber-500', pulse: true };
      }
      return { kind: 'active', label: 'In chat', pipClass: 'bg-emerald-500', pulse: false };
    }
    return { kind: 'online', label: 'Online', pipClass: 'bg-emerald-500', pulse: false };
  }

  const st = input.agentState ?? 'idle';

  if (input.isChatOpenWithAgent && input.isThinking) {
    return { kind: 'busy', label: 'Thinking', pipClass: 'bg-amber-500', pulse: true };
  }
  if (input.isChatOpenWithAgent && input.isSpeechPulse) {
    return { kind: 'active', label: 'Speaking', pipClass: 'bg-emerald-400', pulse: true };
  }
  if (input.orchestrationPaused && st === 'working') {
    return { kind: 'offline', label: 'Paused', pipClass: 'bg-zinc-400', pulse: false };
  }
  if (st === 'working') {
    return { kind: 'busy', label: 'Busy', pipClass: 'bg-red-500', pulse: false };
  }
  if (st === 'on_hold') {
    return { kind: 'away', label: 'Away', pipClass: 'bg-amber-400', pulse: false };
  }
  if (st === 'talking') {
    return { kind: 'active', label: 'In call', pipClass: 'bg-emerald-500', pulse: true };
  }
  if (input.isChatOpenWithAgent) {
    return { kind: 'active', label: 'In chat', pipClass: 'bg-emerald-500', pulse: false };
  }
  if (st === 'moving') {
    return { kind: 'online', label: 'Active', pipClass: 'bg-emerald-500', pulse: false };
  }
  return { kind: 'online', label: 'Online', pipClass: 'bg-emerald-500', pulse: false };
}
