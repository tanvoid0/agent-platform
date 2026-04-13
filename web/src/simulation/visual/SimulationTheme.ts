/**
 * Central simulation look: palettes, lighting, and material presets.
 * Swap defaults or use `mergeSimulationTheme` to customize without rewriting resources.
 */

import type { Placement3Config } from './placement3';
import {
  defaultLoungeKitchen,
  type LoungeKitchenLayoutConfig,
} from '../rooms/lounge/loungeKitchen.config';
import { defaultMeetingRoom } from '../rooms/meeting/meetingRoom.config';

export type { Placement3Config, Vec3 } from './placement3';
export { Placement3, Position, ZERO_PLACEMENT3_CONFIG, ZERO_VEC3 } from './placement3';
export { Placement3Attachment } from './placement3Attachment';

/** glTF-driven office: tints applied in `OfficeSurfaceColorResolver`. */
export interface OfficeSurfacePalette {
  plantTint: number;
  plantLerp: number;
  flexoWarm: number;
  flexoWarmLerp: number;
  flexoThemeLerp: number;
  boardWood: number;
  boardLerp: number;
  sofaFabric: number;
  sofaLerp: number;
  laptopDark: number;
  laptopLerp: number;
  workDeskWood: number;
  workDeskLerp: number;
  cafeWood: number;
  cafeLerp: number;
  chairThemeLerp: number;
  floorNeutral: number;
  floorNeutralLerp: number;
  /** Subtle floor tint toward lounge / breakout vs desk side (per tile center vs POIs). */
  floorBreakoutTint: number;
  floorBreakoutLerp: number;
  floorWorkTint: number;
  floorWorkLerp: number;
  /** Low partitions / room dividers — reads as a different “room” from open floor. */
  partitionTint: number;
  partitionLerp: number;
  /** Meeting / whiteboard side of the floor (third zone vs work + breakout). */
  floorMeetingTint: number;
  floorMeetingLerp: number;
  /** `LineSegments` on the floor around each zone (work / breakout / meeting). */
  roomOutlineWork: number;
  roomOutlineBreakout: number;
  roomOutlineMeeting: number;
  roomOutlineMargin: number;
  /** How strongly `OfficeVisualStyle.monochrome` pulls toward luminance gray. */
  monochromeGrayLerp: number;
}

export interface OfficeMaterialPreset {
  metalnessPerformant: number;
  metalnessFull: number;
  roughness: number;
}

export interface SceneLightingTheme {
  ambientColor: number;
  ambientIntensity: number;
  directionalColor: number;
  directionalIntensity: number;
}

export interface RackStatusAccent {
  seamEmissive: number;
  seamIntensity: number;
  ledEmissive: number;
  ledIntensity: number;
  activityEmissive: number;
  activityIntensity: number;
  pointLight: number;
  pointIntensity: number;
}

export interface LlmRackLabelTheme {
  panelBg: string;
  border: string;
  title: string;
  statusOk: string;
  statusError: string;
  statusIdle: string;
  statusChecking: string;
}

/** Per-deck emissive when the rack is embedded in `office.glb` (Blender `TD_Deck_*` groups). */
export interface ServerDeckPresentationSpec {
  emissive: number;
  emissiveIntensity: number;
}

export type ServerDeckPresentationTheme = Record<
  'off' | 'idle' | 'active' | 'alert',
  ServerDeckPresentationSpec
>;

/** Canvas + emissive colors + pose for the local LLM rack prop (parented under `office.glb` root). */
export interface LlmServerRackVisualTheme {
  /**
   * When true, the rack’s **floor position** is chosen by `LlmServerRackVisual` heuristics (desk cluster, lounge, room bbox),
   * then `placement.position` is added in **office-local** metres.
   * When false, `placement.position` is the full office-local position (Y = floor contact).
   */
  anchorToInteriorLayout: boolean;
  /**
   * Office-local metres and yaw. `rotationYDegrees` is **added** on top of the built-in −90° yaw (rack front faces local +Z).
   * Same pattern as `meetingRoom.whiteboardPlacement` / `loungeKitchen.groupPlacement` — see `placement3.ts`.
   */
  placement: Placement3Config;
  chassis: number;
  chassisShadow: number;
  railSteel: number;
  ventVoid: number;
  seamFace: number;
  seamBaseEmissive: number;
  seamBaseEmissiveIntensity: number;
  ledBaseEmissive: number;
  ledBaseEmissiveIntensity: number;
  activityLedFace: number;
  activityLedBaseEmissive: number;
  activityLedBaseEmissiveIntensity: number;
  smokeParticle: number;
  smokeOpacity: number;
  status: {
    ok: RackStatusAccent;
    error: RackStatusAccent;
    /** Server chat path not monitored (e.g. cloud chat) — dim neutral, not “warning”. */
    idle: RackStatusAccent;
    /** First probe or slow re-probe (see `LlmServerRackVisual`). */
    checking: RackStatusAccent;
  };
  label: LlmRackLabelTheme;
  /** Emissive mapping for deck presentation modes (`off` / `idle` / `active` / `alert`) on embedded meshes. */
  serverDeck: ServerDeckPresentationTheme;
}

export interface LoungeKitchenVisualTheme {
  cabinet: number;
  countertop: number;
  steel: number;
  dark: number;
  cooktop: number;
  cooktopEmissive: number;
  cooktopEmissiveIntensity: number;
  roughnessCabinet: number;
  metalnessCabinet: number;
  roughnessCounter: number;
  metalnessCounter: number;
  roughnessSteel: number;
  metalnessSteel: number;
  roughnessDark: number;
  metalnessDark: number;
  roughnessCook: number;
  metalnessCook: number;
  /** Espresso steam puffs (procedural spheres). */
  coffeeSteam: number;
  coffeeSteamOpacity: number;
  /** “Ready” accent on the machine (point light + optional emissive in mesh). */
  coffeeLightColor: number;
  coffeeLightIntensity: number;
  /**
   * Optional local trim + world nudge after wall-snap (see `loungeKitchen.config.ts`). Stock office uses zeros;
   * world offset for that floor is applied in `LoungeKitchenVisual`.
   */
  layout: LoungeKitchenLayoutConfig;
  /**
   * Optional world delta after `layout`, `KITCHEN_FACING_Y_OFFSET_RAD`, and the built-in stock-office bias in `LoungeKitchenVisual`.
   * Defaults are zero; use for custom GLBs only — {@link Placement3Config}.
   */
  groupPlacement: Placement3Config;
}

/** Procedural conference table + chairs in the meeting / board zone. */
export interface MeetingRoomFurnitureTheme {
  tableTop: number;
  tableEdge: number;
  leg: number;
  chairSeat: number;
  chairBack: number;
  roughnessTable: number;
  metalnessTable: number;
  roughnessLeg: number;
  metalnessLeg: number;
  roughnessChair: number;
  metalnessChair: number;
  /**
   * World-space transform applied to the **whole** desk+chairs group after the board/work layout solve.
   * `position`: metres (+X = E, +Z = S on the reference mat). `rotationYDegrees`: yaw about world +Y (0–360).
   * Reusable pattern: see `Placement3` / `Placement3Config` in `placement3.ts` for other props.
   * In dev, resulting group origin is also `window.__delegationMeetingRoomWorld`.
   */
  groupPlacement: Placement3Config;
  /**
   * Extra transform for meeting **whiteboard / pinboard** meshes from `office.glb` (see `isMeetingBoardMeshName`).
   * Applied to a wrapper group **parented to the office root** (same axes as world when the office root is untransformed).
   * In dev: `window.__delegationMeetingWhiteboardWorld` = wrapper world position each frame.
   */
  whiteboardPlacement: Placement3Config;
  /**
   * Extra logical mesh name substrings (lowercase) so the whiteboard wrapper finds your GLB if `isMeetingBoardMeshName` misses it.
   * Inspect dev console `[MeetingWhiteboard]` hints or a glTF viewer for `office.glb` node names.
   */
  whiteboardMeshNameFragments: string[];
  /**
   * When true, after the layout solve the table’s horizontal “toward board” direction is snapped to the nearest **world ±X or ±Z**.
   * That matches the floor grid; baked GLB whiteboards can still look rotated relative to the procedural set.
   */
  placementSnapForwardToCardinalAxes: boolean;
  /**
   * When true, registers `MeetingRoomFurnitureVisual`: procedural table/chairs/laptops and `sit_idle-meeting-*` POIs.
   * Use when the office GLB has **no** suitable meeting set. When false, the scene uses authored GLB furniture only;
   * `hideGlbMeetingMeshSubstrings` is ignored and nothing is hidden for this reason.
   */
  useProceduralMeetingFurniture: boolean;
  /**
   * When true, meeting seats **clone** the best-matching desk/work chair from `office.glb` (names contain `chair` but do not
   * match `hideGlbMeetingMeshSubstrings`). When false or no candidate is found, procedural box chairs are used.
   * Only applies if {@link useProceduralMeetingFurniture} is true.
   */
  useOfficeChairClones: boolean;
  /**
   * When {@link useProceduralMeetingFurniture} is true: logical mesh name substrings (lowercase) whose matches are **hidden**
   * so the same meeting props are not drawn twice (GLB file pose vs moved procedural group). Empty when you rely on the GLB only.
   */
  hideGlbMeetingMeshSubstrings: string[];
}

/**
 * Decorative floor mat with world-axis labels. Y is always world up; the mat lies in the XZ plane.
 * Texture mapping: canvas **right** = **+X**, canvas **top** = **+Z** (after mesh rotation).
 * Cardinals: **E +X**, **W −X**, **N −Z**, **S +Z** (right‑handed, Y up).
 */
export interface OfficeReferenceMatTheme {
  enabled: boolean;
  /** Metres (world X). */
  width: number;
  /** Metres (world Z). */
  depth: number;
  worldOffsetX: number;
  worldOffsetZ: number;
  /** Clearance above `office.glb` floor to avoid z-fighting. */
  elevation: number;
  roughness: number;
  metalness: number;
  opacity: number;
  /** Small “N / E / S / W” under the +X −X +Z −Z labels. */
  showCardinalLetters: boolean;
}

export interface SimulationTheme {
  scene: { background: number };
  lighting: SceneLightingTheme;
  office: OfficeSurfacePalette;
  officeMaterials: OfficeMaterialPreset;
  llmRack: LlmServerRackVisualTheme;
  loungeKitchen: LoungeKitchenVisualTheme;
  meetingRoom: MeetingRoomFurnitureTheme;
  referenceMat: OfficeReferenceMatTheme;
}

export const DEFAULT_SIMULATION_THEME: SimulationTheme = {
  scene: { background: 0xfafcfb },
  lighting: {
    ambientColor: 0xffffff,
    ambientIntensity: Math.PI,
    directionalColor: 0xffffff,
    directionalIntensity: 0.5 * Math.PI,
  },
  office: {
    plantTint: 0x4a7c59,
    plantLerp: 0.62,
    flexoWarm: 0xf5e6d3,
    flexoWarmLerp: 0.35,
    flexoThemeLerp: 0.14,
    boardWood: 0xc4a882,
    boardLerp: 0.48,
    sofaFabric: 0x6b7c9c,
    sofaLerp: 0.42,
    laptopDark: 0x2a3d4f,
    laptopLerp: 0.52,
    workDeskWood: 0x9a7b5c,
    workDeskLerp: 0.38,
    cafeWood: 0xd4c4b0,
    cafeLerp: 0.32,
    chairThemeLerp: 0.14,
    floorNeutral: 0xe8e4dc,
    floorNeutralLerp: 0.12,
    floorBreakoutTint: 0xe8dcc8,
    floorBreakoutLerp: 0.085,
    floorWorkTint: 0xd8dfe8,
    floorWorkLerp: 0.085,
    partitionTint: 0xc8d2dc,
    partitionLerp: 0.22,
    floorMeetingTint: 0xd9e4dc,
    floorMeetingLerp: 0.095,
    roomOutlineWork: 0x4a6fa8,
    roomOutlineBreakout: 0xa67c4a,
    roomOutlineMeeting: 0x5a8f7a,
    roomOutlineMargin: 0.24,
    monochromeGrayLerp: 0.9,
  },
  officeMaterials: {
    metalnessPerformant: 0.12,
    metalnessFull: 0.35,
    roughness: 1,
  },
  llmRack: {
    anchorToInteriorLayout: true,
    placement: {
      position: { x: 9.2, y: 0, z: -0.2 },
      rotationYDegrees: 90,
    },
    chassis: 0xd4d8df,
    chassisShadow: 0xb8bec8,
    railSteel: 0x3a4149,
    ventVoid: 0x1c2128,
    seamFace: 0x1a1a1a,
    seamBaseEmissive: 0x222222,
    seamBaseEmissiveIntensity: 0.35,
    ledBaseEmissive: 0x1a1a1a,
    ledBaseEmissiveIntensity: 0.2,
    activityLedFace: 0x0a0c0e,
    activityLedBaseEmissive: 0x111111,
    activityLedBaseEmissiveIntensity: 0.15,
    smokeParticle: 0x2a2a2a,
    smokeOpacity: 0.52,
    status: {
      ok: {
        seamEmissive: 0x00e860,
        seamIntensity: 1.45,
        ledEmissive: 0x00c058,
        ledIntensity: 0.85,
        activityEmissive: 0x00ff88,
        activityIntensity: 1.35,
        pointLight: 0x66ffaa,
        pointIntensity: 1.35,
      },
      error: {
        seamEmissive: 0xff2a2a,
        seamIntensity: 1.4,
        ledEmissive: 0xd81818,
        ledIntensity: 0.8,
        activityEmissive: 0xff4444,
        activityIntensity: 1.35,
        pointLight: 0xff6655,
        pointIntensity: 1.25,
      },
      idle: {
        seamEmissive: 0x3a4a58,
        seamIntensity: 0.42,
        ledEmissive: 0x2a3540,
        ledIntensity: 0.32,
        activityEmissive: 0x4a5c6e,
        activityIntensity: 0.48,
        pointLight: 0x7a8c9c,
        pointIntensity: 0.32,
      },
      checking: {
        seamEmissive: 0xffaa22,
        seamIntensity: 1.05,
        ledEmissive: 0xdd8800,
        ledIntensity: 0.62,
        activityEmissive: 0xffcc44,
        activityIntensity: 0.95,
        pointLight: 0xffdd88,
        pointIntensity: 0.85,
      },
    },
    label: {
      panelBg: 'rgba(22,26,32,0.94)',
      border: 'rgba(255,255,255,0.12)',
      title: '#eef1f5',
      statusOk: '#3dff9a',
      statusError: '#ff4d4d',
      statusIdle: '#9aa3ad',
      statusChecking: '#ffcc55',
    },
    serverDeck: {
      off: { emissive: 0x000000, emissiveIntensity: 0 },
      idle: { emissive: 0x3a4a58, emissiveIntensity: 0.38 },
      active: { emissive: 0x00c058, emissiveIntensity: 0.88 },
      alert: { emissive: 0xff2a2a, emissiveIntensity: 0.9 },
    },
  },
  /**
   * Breakout kitchen: materials + optional {@link LoungeKitchenVisualTheme.layout} /
   * `groupPlacement` for non-stock GLBs. Shipped `office.glb` uses identity theme defaults; world offset for
   * that floor is `DEFAULT_OFFICE_KITCHEN_WORLD_BIAS` in `LoungeKitchenVisual.ts`.
   */
  loungeKitchen: { ...defaultLoungeKitchen },
  meetingRoom: { ...defaultMeetingRoom },
  referenceMat: {
    enabled: true,
    width: 2.6,
    depth: 2.6,
    worldOffsetX: 0,
    worldOffsetZ: 0,
    elevation: 0.004,
    roughness: 0.92,
    metalness: 0.04,
    opacity: 1,
    showCardinalLetters: true,
  },
};

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v);
}

export type DeepPartial<T> = {
  [K in keyof T]?: T[K] extends object ? DeepPartial<T[K]> : T[K];
};

/** Deep-merge partial theme overrides (e.g. dark mode palette). */
export function mergeSimulationTheme(partial: DeepPartial<SimulationTheme>): SimulationTheme {
  return deepMerge(DEFAULT_SIMULATION_THEME, partial) as SimulationTheme;
}

function deepMerge(base: unknown, patch: unknown): unknown {
  if (!isPlainObject(base) || !isPlainObject(patch)) return patch ?? base;
  const out: Record<string, unknown> = { ...base };
  for (const k of Object.keys(patch)) {
    const pb = patch[k];
    if (pb === undefined) continue;
    const bb = base[k];
    if (isPlainObject(bb) && isPlainObject(pb)) {
      out[k] = deepMerge(bb, pb);
    } else {
      out[k] = pb;
    }
  }
  return out;
}
