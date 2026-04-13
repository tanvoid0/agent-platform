import type * as THREE from 'three/webgpu';

/** glTF node name on the mesh (or userData) — matches nodes in `public/models/office.glb`. */
export function officeLogicalName(mesh: THREE.Mesh): string {
  const fromUser = mesh.userData?.name;
  return String(fromUser || mesh.name || '').toLowerCase();
}
