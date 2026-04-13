import * as THREE from 'three/webgpu';
import type { OfficeSurfacePalette } from './SimulationTheme';
import {
  getBreakoutFurnitureBox,
  getMeetingZoneBox,
  getWorkDeskZoneBox,
} from './officeZoneRegions';

function addFloorRectOutline(
  parent: THREE.Group,
  box: THREE.Box3,
  y: number,
  color: number,
  margin: number,
  geometries: THREE.BufferGeometry[],
  materials: THREE.LineBasicMaterial[]
): void {
  const x0 = box.min.x - margin;
  const x1 = box.max.x + margin;
  const z0 = box.min.z - margin;
  const z1 = box.max.z + margin;
  const pts = [
    new THREE.Vector3(x0, y, z0),
    new THREE.Vector3(x1, y, z0),
    new THREE.Vector3(x1, y, z0),
    new THREE.Vector3(x1, y, z1),
    new THREE.Vector3(x1, y, z1),
    new THREE.Vector3(x0, y, z1),
    new THREE.Vector3(x0, y, z1),
    new THREE.Vector3(x0, y, z0),
  ];
  const geom = new THREE.BufferGeometry().setFromPoints(pts);
  geometries.push(geom);
  const mat = new THREE.LineBasicMaterial({
    color,
    transparent: true,
    opacity: 0.78,
    depthWrite: false,
  });
  materials.push(mat);
  const lines = new THREE.LineSegments(geom, mat);
  parent.add(lines);
}

export type RoomOutlineBuildResult = {
  group: THREE.Group;
  dispose: () => void;
};

/**
 * Flat **line loops** on the floor around desk, breakout, and meeting furniture clusters
 * so open-plan zones read as distinct “rooms”.
 */
export function buildOfficeRoomOutlineDecor(
  office: THREE.Object3D,
  floorY: number,
  loungeAnchor: THREE.Vector3 | null,
  palette: Pick<
    OfficeSurfacePalette,
    | 'roomOutlineWork'
    | 'roomOutlineBreakout'
    | 'roomOutlineMeeting'
    | 'roomOutlineMargin'
  >
): RoomOutlineBuildResult {
  const group = new THREE.Group();
  group.name = 'office-room-outlines';
  const y = floorY + 0.01;
  const m = palette.roomOutlineMargin;
  const geometries: THREE.BufferGeometry[] = [];
  const materials: THREE.LineBasicMaterial[] = [];

  const workBox = getWorkDeskZoneBox(office);
  if (workBox) {
    addFloorRectOutline(group, workBox, y, palette.roomOutlineWork, m, geometries, materials);
  }

  let breakBox = getBreakoutFurnitureBox(office);
  if (!breakBox && loungeAnchor) {
    const s = 2.15;
    breakBox = new THREE.Box3(
      new THREE.Vector3(loungeAnchor.x - s, floorY, loungeAnchor.z - s),
      new THREE.Vector3(loungeAnchor.x + s, floorY + 0.02, loungeAnchor.z + s)
    );
  }
  if (breakBox) {
    addFloorRectOutline(group, breakBox, y, palette.roomOutlineBreakout, m, geometries, materials);
  }

  const meetBox = getMeetingZoneBox(office);
  if (meetBox) {
    addFloorRectOutline(group, meetBox, y, palette.roomOutlineMeeting, m, geometries, materials);
  }

  return {
    group,
    dispose: () => {
      for (const g of geometries) g.dispose();
      for (const mat of materials) mat.dispose();
    },
  };
}
