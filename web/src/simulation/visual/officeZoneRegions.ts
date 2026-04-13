import * as THREE from 'three/webgpu';
import { officeLogicalName } from './officeMeshUtils';

/** Union world AABB of meshes whose logical name includes any of `keywords`. */
export function mergeBboxByKeywords(office: THREE.Object3D, keywords: string[]): THREE.Box3 | null {
  const box = new THREE.Box3();
  let found = false;
  office.traverse((child) => {
    if (!(child as THREE.Mesh).isMesh) return;
    const mesh = child as THREE.Mesh;
    const n = officeLogicalName(mesh);
    if (!keywords.some((k) => n.includes(k))) return;
    const b = new THREE.Box3().setFromObject(mesh);
    if (b.isEmpty()) return;
    if (!found) {
      box.copy(b);
      found = true;
    } else box.union(b);
  });
  return found ? box : null;
}

export function getWorkDeskZoneBox(office: THREE.Object3D): THREE.Box3 | null {
  return mergeBboxByKeywords(office, ['work-desk', 'work_desk']);
}

/** Logical mesh name (lowercased) for whiteboard / pinboard / flipchart in the meeting zone. */
export function isMeetingBoardMeshName(n: string): boolean {
  if (n.includes('keyboard') || n.includes('clipboard') || n.includes('cupboard')) return false;
  if (n.includes('motherboard')) return false;
  /** `public/models/office.glb` meeting board node. */
  if (n === 'static-board') return true;
  if (
    n.includes('whiteboard') ||
    n.includes('white-board') ||
    n.includes('white_board') ||
    n.includes('pinboard') ||
    n.includes('pin-board') ||
    n.includes('flipchart') ||
    n.includes('flip-chart') ||
    n.includes('flip_chart')
  ) {
    return true;
  }
  if (n.includes('meeting') && n.includes('board')) return true;
  if (n.includes('notice') && n.includes('board')) return true;
  if (n.includes('presentation') && (n.includes('board') || n.includes('screen'))) return true;
  if (n === 'board' || n.startsWith('board_') || n.endsWith('_board') || n.includes('easel')) return true;
  return false;
}

/** Whiteboard / pinboard — meeting end of the office (avoids `keyboard` false positives on `board`). */
export function getMeetingZoneBox(office: THREE.Object3D): THREE.Box3 | null {
  const box = new THREE.Box3();
  let found = false;
  office.traverse((child) => {
    if (!(child as THREE.Mesh).isMesh) return;
    const mesh = child as THREE.Mesh;
    if (!isMeetingBoardMeshName(officeLogicalName(mesh))) return;
    const b = new THREE.Box3().setFromObject(mesh);
    if (b.isEmpty()) return;
    if (!found) {
      box.copy(b);
      found = true;
    } else box.union(b);
  });
  return found ? box : null;
}

export function getBreakoutFurnitureBox(office: THREE.Object3D): THREE.Box3 | null {
  return mergeBboxByKeywords(office, [
    'sofa',
    'cafe-table',
    'cafetable',
    'coffee-table',
    'coffee_table',
    'round-table',
    'round_table',
  ]);
}

export function centroidOnFloor(box: THREE.Box3, floorY: number): THREE.Vector3 {
  const c = new THREE.Vector3();
  box.getCenter(c);
  c.y = floorY;
  return c;
}

export function getMeetingZoneCenter(office: THREE.Object3D, floorY: number): THREE.Vector3 | null {
  const b = getMeetingZoneBox(office);
  if (!b) return null;
  return centroidOnFloor(b, floorY);
}
