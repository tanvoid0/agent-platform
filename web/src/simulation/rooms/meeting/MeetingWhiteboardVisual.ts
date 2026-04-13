import * as THREE from 'three/webgpu';
import { Placement3Attachment } from '../../visual/placement3Attachment';
import { ZERO_PLACEMENT3_CONFIG, type SimulationTheme } from '../../visual/SimulationTheme';
import type { ISimulationWorldResource } from '../../visual/WorldResource';
import { officeLogicalName } from '../../visual/officeMeshUtils';
import { isMeetingBoardMeshName } from '../../visual/officeZoneRegions';

const WRAPPER_NAME = 'meeting-whiteboard-placement';

/**
 * Reparents meeting whiteboard meshes from `office.glb` under one group so they can be shifted/yawed like the desk.
 * Placement: `meetingRoom.whiteboardPlacement` in `meetingRoom.config.ts` (office-local metres + yaw).
 */
export class MeetingWhiteboardVisual implements ISimulationWorldResource {
  readonly id = 'meeting-whiteboard-placement';
  private readonly wrapper: THREE.Group;
  private readonly office: THREE.Group;
  private readonly theme: SimulationTheme;
  private readonly captured: { mesh: THREE.Mesh; parent: THREE.Object3D | null }[] = [];
  private readonly disabled: boolean;
  private readonly whiteboardPose = new Placement3Attachment(ZERO_PLACEMENT3_CONFIG, 'meeting-whiteboard');
  private lastPlacementKey = '';

  constructor(getOffice: () => THREE.Group | null, simulationTheme: SimulationTheme) {
    this.theme = simulationTheme;
    const office = getOffice();
    if (!office) {
      throw new Error('MeetingWhiteboardVisual: office scene not loaded');
    }
    this.office = office;

    const extra = simulationTheme.meetingRoom.whiteboardMeshNameFragments;

    const meshes: THREE.Mesh[] = [];
    office.traverse((child) => {
      if (!(child as THREE.Mesh).isMesh) return;
      const mesh = child as THREE.Mesh;
      if (!mesh.visible) return;
      const n = officeLogicalName(mesh);
      const byDefault = isMeetingBoardMeshName(n);
      const byExtra = extra.some((frag) => n.includes(frag.toLowerCase()));
      if (!byDefault && !byExtra) return;
      meshes.push(mesh);
    });

    if (meshes.length === 0) {
      this.wrapper = new THREE.Group();
      this.disabled = true;
      if (import.meta.env.DEV) {
        const hints: string[] = [];
        office.traverse((child) => {
          if (!(child as THREE.Mesh).isMesh) return;
          const n = officeLogicalName(child as THREE.Mesh);
          if (/white|board|screen|panel|pin|flip|easel|present/i.test(n)) hints.push(n);
        });
        console.warn(
          '[MeetingWhiteboard] No meshes matched board heuristics — whiteboardPlacement has no effect.',
          'Add substrings to meetingRoom.whiteboardMeshNameFragments (logical names, lowercased).',
          'Sample mesh names containing white/board/screen/panel/…:',
          [...new Set(hints)].slice(0, 24)
        );
      }
      return;
    }

    this.disabled = false;
    this.wrapper = new THREE.Group();
    this.wrapper.name = WRAPPER_NAME;
    office.add(this.wrapper);

    for (const mesh of meshes) {
      this.captured.push({ mesh, parent: mesh.parent });
      this.wrapper.attach(mesh);
    }
  }

  update(_delta: number): void {
    if (this.disabled) return;
    const gp = this.theme.meetingRoom.whiteboardPlacement;
    this.whiteboardPose.setConfig(gp);
    this.whiteboardPose.applyAbsolute(this.wrapper);
    this.wrapper.updateMatrixWorld(true);

    if (import.meta.env.DEV) {
      const world = new THREE.Vector3();
      this.wrapper.getWorldPosition(world);
      window.__delegationMeetingWhiteboardWorld = { x: world.x, y: world.y, z: world.z };

      const key = `${gp.position.x}|${gp.position.y}|${gp.position.z}|${gp.rotationYDegrees}`;
      if (key !== this.lastPlacementKey) {
        this.lastPlacementKey = key;
        console.info(
          '[Meeting room] whiteboard — office-local offset (m) x,y,z:',
          gp.position.x.toFixed(3),
          gp.position.y.toFixed(3),
          gp.position.z.toFixed(3),
          'yaw°',
          gp.rotationYDegrees,
          '| world origin x,y,z:',
          world.x.toFixed(3),
          world.y.toFixed(3),
          world.z.toFixed(3),
          '| Theme: meetingRoom.whiteboardPlacement (see meetingRoom.config.ts)'
        );
      }
    }
  }

  dispose(): void {
    if (import.meta.env.DEV) {
      delete window.__delegationMeetingWhiteboardWorld;
    }
    if (this.disabled) return;
    for (const { mesh, parent } of this.captured) {
      if (parent) parent.attach(mesh);
    }
    this.captured.length = 0;
    this.wrapper.removeFromParent();
  }
}
