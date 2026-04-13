import type * as THREE from 'three/webgpu';

/**
 * Pluggable 3D decoration / prop mounted alongside the office GLB.
 * New resources implement this contract and register in `buildSimulationWorldResources`.
 */
export interface ISimulationWorldResource {
  /** Stable id for logging, feature flags, and future persistence. */
  readonly id: string;
  /** Remove from scene and dispose GPU objects. */
  dispose(): void;
  /** Optional per-frame hook (particles, LOD, etc.). */
  update?(deltaSeconds: number): void;
}

/** Optional base: shared disposal pattern for resources that own a scene subgraph. */
export abstract class SimulationWorldResourceBase implements ISimulationWorldResource {
  abstract readonly id: string;

  constructor(
    protected readonly scene: THREE.Scene,
    protected readonly root: THREE.Object3D
  ) {
    scene.add(root);
  }

  dispose(): void {
    this.scene.remove(this.root);
  }
}
