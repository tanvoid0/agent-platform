import {
  AGENT_ORCHESTRATION_KEYS,
  CHARACTER_LOCOMOTION_KEYS,
  type AgentState,
  type CharacterStateKey,
} from '../../types';
import { PROJECT_SESSION_FORMAT_VERSION } from './constants';
import type { AgentPose, ProjectSessionWire } from './types';

const LOCOMOTION = new Set<string>(CHARACTER_LOCOMOTION_KEYS);
const ORCHESTRATION = new Set<string>(AGENT_ORCHESTRATION_KEYS);

export function parseLocomotion(raw: unknown): CharacterStateKey | undefined {
  return typeof raw === 'string' && LOCOMOTION.has(raw) ? (raw as CharacterStateKey) : undefined;
}

export function parseOrchestrationStatus(raw: unknown): AgentState | undefined {
  return typeof raw === 'string' && ORCHESTRATION.has(raw) ? (raw as AgentState) : undefined;
}

/** Normalizes wire / unknown blobs into numeric-index maps for the stores. */
export function decodeSessionPoses(raw: unknown): Record<number, AgentPose> {
  const out: Record<number, AgentPose> = {};
  if (!raw || typeof raw !== 'object') return out;
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
    const idx = Number(k);
    if (!Number.isFinite(idx) || !v || typeof v !== 'object') continue;
    const o = v as Record<string, unknown>;
    const x = Number(o.x);
    const y = Number(o.y);
    const z = Number(o.z);
    if (![x, y, z].every((n) => Number.isFinite(n))) continue;
    const locomotion = parseLocomotion(o.locomotion);
    out[idx] = locomotion ? { x, y, z, locomotion } : { x, y, z };
  }
  return out;
}

export function decodeSessionOrchestration(raw: unknown): Record<number, AgentState> {
  const out: Record<number, AgentState> = {};
  if (!raw || typeof raw !== 'object') return out;
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
    const idx = Number(k);
    if (!Number.isFinite(idx)) continue;
    const s = parseOrchestrationStatus(v);
    if (s) out[idx] = s;
  }
  return out;
}

/**
 * Reads `payload.session` (or empty). Unknown `formatVersion` values return empty maps until a codec is added.
 */
export function decodeProjectSessionField(sessionRaw: unknown): {
  poses: Record<number, AgentPose>;
  orchestration: Record<number, AgentState>;
} {
  if (!sessionRaw || typeof sessionRaw !== 'object') {
    return { poses: {}, orchestration: {} };
  }
  const s = sessionRaw as Record<string, unknown>;
  const ver = s.formatVersion;
  if (ver !== PROJECT_SESSION_FORMAT_VERSION) {
    return { poses: {}, orchestration: {} };
  }
  return {
    poses: decodeSessionPoses(s.poses),
    orchestration: decodeSessionOrchestration(s.orchestration),
  };
}

export function encodeProjectSessionWire(input: {
  poses: Record<number, AgentPose>;
  orchestration: Record<number, AgentState>;
}): ProjectSessionWire {
  const poses: ProjectSessionWire['poses'] = {};
  for (const [k, p] of Object.entries(input.poses)) {
    const idx = Number(k);
    if (!Number.isFinite(idx)) continue;
    poses[String(idx)] = {
      x: p.x,
      y: p.y,
      z: p.z,
      ...(p.locomotion ? { locomotion: p.locomotion } : {}),
    };
  }
  const orchestration: ProjectSessionWire['orchestration'] = {};
  for (const [k, st] of Object.entries(input.orchestration)) {
    const idx = Number(k);
    if (!Number.isFinite(idx)) continue;
    orchestration[String(idx)] = st;
  }
  return {
    formatVersion: PROJECT_SESSION_FORMAT_VERSION,
    poses,
    orchestration,
  };
}

