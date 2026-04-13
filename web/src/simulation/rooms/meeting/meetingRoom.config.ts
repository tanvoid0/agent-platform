import type { MeetingRoomFurnitureTheme } from '../../visual/SimulationTheme';

/**
 * Meeting / board zone — tuned for `public/models/office.glb`.
 *
 * That file authors `static-board`, meeting seating via `poi-sit_idle-*`, and `poi-idle-area-boardroom` (loaded as
 * POI id `area-boardroom`). We keep `useProceduralMeetingFurniture: false` so the GLB layout stays authoritative.
 *
 * For a GLB **without** meeting props, set `useProceduralMeetingFurniture: true` and add `hideGlbMeetingMeshSubstrings`
 * for any overlapping meshes.
 *
 * `groupPlacement` / `whiteboardPlacement` apply only when procedural meeting is enabled; otherwise they are unused
 * (identity here keeps overrides predictable).
 */
export const defaultMeetingRoom: MeetingRoomFurnitureTheme = {
  tableTop: 0xd8d2c8,
  tableEdge: 0x9a8f82,
  leg: 0x3a3e44,
  chairSeat: 0x5c6678,
  chairBack: 0x4a5366,
  roughnessTable: 0.65,
  metalnessTable: 0.06,
  roughnessLeg: 0.55,
  metalnessLeg: 0.35,
  roughnessChair: 0.78,
  metalnessChair: 0.08,
  groupPlacement: {
    position: { x: 0, y: 0, z: 0 },
    rotationYDegrees: 0,
  },
  whiteboardPlacement: {
    position: { x: 0, y: 0, z: 0 },
    rotationYDegrees: 0,
  },
  whiteboardMeshNameFragments: [],
  placementSnapForwardToCardinalAxes: true,
  useProceduralMeetingFurniture: false,
  useOfficeChairClones: true,
  hideGlbMeetingMeshSubstrings: [],
};
