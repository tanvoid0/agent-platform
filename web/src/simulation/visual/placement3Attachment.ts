import * as THREE from 'three/webgpu';
import {
  applyPlacement3WorldDelta,
  type Placement3Config,
  Placement3,
} from './placement3';

/**
 * Applies a {@link Placement3Config} to a `THREE.Object3D`: metres + yaw about +Y.
 * Config stays plain data; this class centralizes Three.js wiring for room visuals.
 */
export class Placement3Attachment {
  constructor(
    public config: Placement3Config,
    readonly label?: string
  ) {}

  setConfig(c: Placement3Config): void {
    this.config = c;
  }

  merge(delta: Placement3Config): void {
    this.config = Placement3.merge(this.config, delta);
  }

  /** Replace `position` / `rotation.x,z`; set yaw from `rotationYDegrees` (radians zero on other axes). */
  applyAbsolute(obj: THREE.Object3D): void {
    const c = this.config;
    obj.position.set(c.position.x, c.position.y, c.position.z);
    obj.rotation.x = 0;
    obj.rotation.z = 0;
    obj.rotation.y = THREE.MathUtils.degToRad(c.rotationYDegrees);
  }

  /** Mutating: add position delta and return new `rotation.y` (radians). */
  composeWorldDelta(obj: THREE.Object3D, existingRotationYRad: number): number {
    return applyPlacement3WorldDelta(obj.position, existingRotationYRad, this.config);
  }
}
