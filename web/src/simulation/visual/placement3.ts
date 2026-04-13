/**
 * Reusable world-space offset + yaw for a single mesh, procedural group, or GLB root.
 * Metres for position; degrees for rotation about **world +Y** (CCW from above).
 */

export type Vec3 = { x: number; y: number; z: number };

/** Plain snapshot for themes / JSON / `deepMerge`. */
export interface Placement3Config {
  position: Vec3;
  /** Yaw around world +Y, degrees (0–360 is typical; any real number is fine). */
  rotationYDegrees: number;
}

export const ZERO_VEC3: Vec3 = { x: 0, y: 0, z: 0 };

export const ZERO_PLACEMENT3_CONFIG: Placement3Config = {
  position: { x: 0, y: 0, z: 0 },
  rotationYDegrees: 0,
};

/** Metres along world X / Y / Z (+X = E, +Z = S on the office reference mat). */
export class Position {
  constructor(public x = 0, public y = 0, public z = 0) {}

  toVec(): Vec3 {
    return { x: this.x, y: this.y, z: this.z };
  }
}

export class Placement3 {
  constructor(
    public position: Position = new Position(),
    public rotationYDegrees = 0
  ) {}

  static fromConfig(c: Placement3Config): Placement3 {
    return new Placement3(new Position(c.position.x, c.position.y, c.position.z), c.rotationYDegrees);
  }

  toConfig(): Placement3Config {
    return {
      position: this.position.toVec(),
      rotationYDegrees: this.rotationYDegrees,
    };
  }

  addConfig(delta: Placement3Config): Placement3 {
    return new Placement3(
      new Position(
        this.position.x + delta.position.x,
        this.position.y + delta.position.y,
        this.position.z + delta.position.z
      ),
      this.rotationYDegrees + delta.rotationYDegrees
    );
  }

  static merge(base: Placement3Config, delta: Placement3Config): Placement3Config {
    return Placement3.fromConfig(base).addConfig(delta).toConfig();
  }
}

/**
 * Add `delta.position` to `position` (mutating) and return `rotationYRad` plus yaw from `delta.rotationYDegrees`.
 * Use with `THREE.Object3D`: `obj.rotation.y = applyPlacement3WorldDelta(obj.position, obj.rotation.y, config)`.
 */
export function applyPlacement3WorldDelta(
  position: { x: number; y: number; z: number },
  rotationYRad: number,
  delta: Placement3Config
): number {
  position.x += delta.position.x;
  position.y += delta.position.y;
  position.z += delta.position.z;
  return rotationYRad + (delta.rotationYDegrees * Math.PI) / 180;
}
