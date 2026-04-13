/**
 * Project session — simulation presence that travels with `PersistedProjectPayload`.
 *
 * - **poses**: 3D positions + locomotion (reapplied when `sessionSceneRevision` bumps).
 * - **orchestration**: `AgentState` per index (UI + task-adjacent behavior).
 *
 * To add fields: extend `ProjectSessionWire`, bump `PROJECT_SESSION_FORMAT_VERSION`, branch in `decodeProjectSessionField`.
 */
export {
  PROJECT_SESSION_FORMAT_VERSION,
  PROJECT_SESSION_SCENE_CAPTURE_MS,
} from './constants';
export type { AgentPose, ProjectSessionWire } from './types';
export {
  decodeProjectSessionField,
  decodeSessionOrchestration,
  decodeSessionPoses,
  encodeProjectSessionWire,
  parseLocomotion,
  parseOrchestrationStatus,
} from './codec';
export { applyProjectSessionToStores, readProjectSessionFromStores } from './sync';
