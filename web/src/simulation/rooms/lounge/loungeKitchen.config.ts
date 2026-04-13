import type { LoungeKitchenVisualTheme } from '../../visual/SimulationTheme';
import type { Placement3Config } from '../../visual/placement3';

/**
 * Optional layout deltas applied **before** `KITCHEN_FACING_Y_OFFSET_RAD` and {@link LoungeKitchenVisualTheme.groupPlacement}.
 * Stock `office.glb` needs no defaults here: former theme nudges are baked into
 * `DEFAULT_OFFICE_KITCHEN_WORLD_BIAS` in `LoungeKitchenVisual.ts`. Use this only to trim a custom GLB.
 *
 * `afterWallSnapLocal`: group **local** frame — `position.x` along the run, `position.z` into the room, `position.y` vertical.
 */
export interface LoungeKitchenLayoutConfig {
  afterWallSnapLocal: Placement3Config;
  /** World-space delta on the kitchen group (metres + yaw) after local trim. */
  worldNudge: Placement3Config;
}

/** Identity layout for stock office; override via `mergeSimulationTheme` for other floors. */
export const defaultLoungeKitchenLayout: LoungeKitchenLayoutConfig = {
  afterWallSnapLocal: {
    position: { x: 0, y: 0, z: 0 },
    rotationYDegrees: 0,
  },
  worldNudge: {
    position: { x: 0, y: 0, z: 0 },
    rotationYDegrees: 0,
  },
};

/** Default breakout kitchen materials, `layout`, and optional `groupPlacement` (world delta after solver + stock bias). */
export const defaultLoungeKitchen: LoungeKitchenVisualTheme = {
  layout: defaultLoungeKitchenLayout,
  cabinet: 0xf5f6f8,
  countertop: 0xd4c4b0,
  steel: 0xc8ced6,
  dark: 0x3a4048,
  cooktop: 0x1a1e24,
  cooktopEmissive: 0x1a2228,
  cooktopEmissiveIntensity: 0.12,
  roughnessCabinet: 0.88,
  metalnessCabinet: 0.08,
  roughnessCounter: 0.72,
  metalnessCounter: 0.06,
  roughnessSteel: 0.42,
  metalnessSteel: 0.55,
  roughnessDark: 0.75,
  metalnessDark: 0.12,
  roughnessCook: 0.35,
  metalnessCook: 0.25,
  coffeeSteam: 0xe8ecf0,
  coffeeSteamOpacity: 0.42,
  coffeeLightColor: 0xffb48a,
  coffeeLightIntensity: 0.38,
  groupPlacement: {
    position: { x: 0, y: 0, z: 0 },
    rotationYDegrees: 0,
  },
};
