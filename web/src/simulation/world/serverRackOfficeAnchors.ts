/**
 * Object names for the AI server rack when authored inside `public/models/office.glb` (Blender).
 * Match these exactly on empties / parent objects so runtime can bind VFX, deck tinting, and animations.
 */
export const TD_AI_SERVER_RACK_ROOT = 'TD_AI_Server_Rack';

/** One workstation “deck” per logical tier (meshes parented under each empty). */
export const TD_DECK_OBJECT_NAMES = {
  llm: 'TD_Deck_LLM',
  backend: 'TD_Deck_Backend',
  database: 'TD_Deck_Database',
} as const;

export type ServerDeckObjectKey = keyof typeof TD_DECK_OBJECT_NAMES;

/** Optional empties for procedural overlay (smoke, lights, canvas label). */
export const TD_VFX_SMOKE_ORIGIN = 'TD_VFX_SmokeOrigin';
export const TD_VFX_STATUS_LIGHT = 'TD_VFX_StatusLight';
export const TD_VFX_LABEL_MOUNT = 'TD_VFX_LabelMount';

/** Recommended prefix for Blender actions exported with the GLB (e.g. `TD_Rack_FanLoop`). */
export const TD_RACK_ANIM_CLIP_PREFIX = 'TD_Rack_';
