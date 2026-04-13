import type { CharacterStateKey } from '../../types';
import { PROJECT_SESSION_FORMAT_VERSION } from './constants';

/** One agent’s physical state in the office sim (persisted + reapplied on load). */
export interface AgentPose {
  x: number;
  y: number;
  z: number;
  locomotion?: CharacterStateKey;
}

/**
 * `PersistedProjectPayload.session` — JSON-safe string keys, versioned for forward-compatible codecs.
 * Add new optional fields here when extending server-driven simulation state.
 */
export interface ProjectSessionWire {
  formatVersion: typeof PROJECT_SESSION_FORMAT_VERSION;
  poses: Record<string, { x: number; y: number; z: number; locomotion?: string }>;
  orchestration: Record<string, string>;
}
