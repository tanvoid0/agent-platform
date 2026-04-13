import * as THREE from 'three/webgpu';
import { DEFAULT_SIMULATION_THEME, type SimulationTheme } from '../../visual/SimulationTheme';
import type { ISimulationWorldResource } from '../../visual/WorldResource';
import { officeLogicalName } from '../../visual/officeMeshUtils';
import { getMeetingZoneBox, getWorkDeskZoneBox } from '../../visual/officeZoneRegions';
import { PoiManager } from '../../world/PoiManager';

const POI_PREFIX = 'sit_idle-meeting';

/** Snap a horizontal unit vector to the nearest world ±X or ±Z (Y unchanged at 0). */
function snapHorizontalForwardToCardinalXZ(v: THREE.Vector3): void {
  const ax = Math.abs(v.x);
  const az = Math.abs(v.z);
  if (ax >= az) {
    const sx = Math.sign(v.x);
    v.set(sx === 0 ? 1 : sx, 0, 0);
  } else {
    const sz = Math.sign(v.z);
    v.set(0, 0, sz === 0 ? 1 : sz);
  }
}

function glbNameMatchesAnyFragment(n: string, frags: readonly string[]): boolean {
  return frags.some((f) => n.includes(f));
}

/** Prefer a compact parent so we clone the whole chair, not a desk cluster. */
function pickChairCloneRoot(mesh: THREE.Mesh, office: THREE.Group): THREE.Object3D {
  const parent = mesh.parent;
  if (!parent || parent === office) return mesh;
  parent.updateWorldMatrix(true, true);
  const b = new THREE.Box3().setFromObject(parent);
  if (b.isEmpty()) return mesh;
  const sz = new THREE.Vector3();
  b.getSize(sz);
  if (sz.y > 0.06 && sz.y <= 1.55 && sz.x <= 1.25 && sz.z <= 1.25) return parent;
  return mesh;
}

/**
 * First visible `office.glb` chair that is not in the “replaced by procedural meeting” hide list.
 * Prefers chairs nearer the work cluster than the meeting zone (same asset is reused at the meeting table).
 */
function findWorkChairPrototype(
  office: THREE.Group,
  hideNameFragments: readonly string[],
  workC: THREE.Vector3,
  boardCenter: THREE.Vector3
): THREE.Object3D | null {
  let best: { root: THREE.Object3D; score: number } | null = null;
  office.traverse((child) => {
    if (!(child as THREE.Mesh).isMesh) return;
    const mesh = child as THREE.Mesh;
    if (!mesh.visible) return;
    const n = officeLogicalName(mesh);
    if (!n.includes('chair')) return;
    if (glbNameMatchesAnyFragment(n, hideNameFragments)) return;
    const root = pickChairCloneRoot(mesh, office);
    root.updateWorldMatrix(true, true);
    const box = new THREE.Box3().setFromObject(root);
    if (box.isEmpty()) return;
    const c = new THREE.Vector3();
    box.getCenter(c);
    const dw = (c.x - workC.x) ** 2 + (c.z - workC.z) ** 2;
    const dm = (c.x - boardCenter.x) ** 2 + (c.z - boardCenter.z) ** 2;
    const score = dw - dm * 0.35;
    if (!best || score < best.score) best = { root, score };
  });
  return best?.root ?? null;
}

function resetCloneRootTransform(clone: THREE.Object3D): void {
  clone.position.set(0, 0, 0);
  clone.quaternion.identity();
  clone.scale.set(1, 1, 1);
}

function resolveMeetingBoardPlacement(
  office: THREE.Object3D,
  poiManager: PoiManager
): { boardCenter: THREE.Vector3; boardHalf: number; floorY: number; roomBox: THREE.Box3 } | null {
  const roomBox = new THREE.Box3().setFromObject(office);
  if (roomBox.isEmpty()) return null;
  const floorY = roomBox.min.y;

  const meetBox = getMeetingZoneBox(office);
  if (meetBox && !meetBox.isEmpty()) {
    const boardCenter = new THREE.Vector3();
    meetBox.getCenter(boardCenter);
    boardCenter.y = floorY;
    const meetSize = new THREE.Vector3();
    meetBox.getSize(meetSize);
    const boardHalf = 0.5 * Math.min(meetSize.x, meetSize.z);
    return { boardCenter, boardHalf, floorY, roomBox };
  }

  const boardroomPois = poiManager.getPoisByPrefix('boardroom');
  const poi =
    poiManager.getPoi('area-boardroom') ??
    poiManager.getPoi('boardroom') ?? // legacy id if present
    (boardroomPois.length > 0 ? boardroomPois.sort((a, b) => a.id.localeCompare(b.id))[0] : undefined);
  if (poi) {
    return {
      boardCenter: new THREE.Vector3(poi.position.x, floorY, poi.position.z),
      boardHalf: 0.55,
      floorY,
      roomBox,
    };
  }

  return null;
}

type PoseInputs = {
  boardCenter: THREE.Vector3;
  boardHalf: number;
  floorY: number;
  roomCenter: THREE.Vector3;
  workC: THREE.Vector3;
};

/**
 * Conference table + chairs in the meeting / board zone (derived from whiteboard meshes in `office.glb`,
 * or from `poi-area-boardroom` / `poi-boardroom-*` when mesh names do not match).
 * Registers `sit_idle-meeting-*` POIs so `moveNpcToBoardroom` can seat agents at the table.
 *
 * **Placement:** `meetingRoom.groupPlacement` (`position`, `rotationYDegrees`) is read from the active theme each frame in `update()`.
 *
 * **Chairs:** When `meetingRoom.useOfficeChairClones` is true, seats clone a desk/work chair from `office.glb`
 * (see `findWorkChairPrototype`). Otherwise procedural box chairs are built (same colors as theme).
 *
 * Only constructed when `meetingRoom.useProceduralMeetingFurniture` is true (see `meetingRoom.config.ts`).
 */
export class MeetingRoomFurnitureVisual implements ISimulationWorldResource {
  readonly id = 'meeting-room-furniture';
  readonly group = new THREE.Group();
  private readonly geometries: THREE.BufferGeometry[] = [];
  private readonly materials: THREE.MeshStandardNodeMaterial[] = [];
  private readonly poiIds: string[] = [];
  private readonly seatGroups: THREE.Group[] = [];
  private poseInputs: PoseInputs | null = null;
  private lastPlacementThemeKey = '';
  private useGlbChairs = false;
  private glbChairsFloorAligned = false;
  private readonly theme: SimulationTheme;

  constructor(
    private readonly scene: THREE.Scene,
    getOffice: () => THREE.Group | null,
    private readonly poiManager: PoiManager,
    simulationTheme: SimulationTheme = DEFAULT_SIMULATION_THEME
  ) {
    this.theme = simulationTheme;
    const mt = simulationTheme.meetingRoom;
    const office = getOffice();
    if (!office) {
      throw new Error('MeetingRoomFurnitureVisual: office scene not loaded');
    }

    const placement = resolveMeetingBoardPlacement(office, this.poiManager);
    if (!placement) {
      return;
    }

    const { boardCenter, boardHalf, floorY, roomBox: ob } = placement;

    const roomCenter = new THREE.Vector3();
    ob.getCenter(roomCenter);
    roomCenter.y = floorY;

    const workBox = getWorkDeskZoneBox(office);
    const workC = new THREE.Vector3();
    if (workBox && !workBox.isEmpty()) {
      workBox.getCenter(workC);
      workC.y = floorY;
    } else {
      workC.copy(roomCenter);
    }

    this.poseInputs = {
      boardCenter: boardCenter.clone(),
      boardHalf,
      floorY,
      roomCenter: roomCenter.clone(),
      workC: workC.clone(),
    };

    this.scene.add(this.group);

    const chairPrototype =
      mt.useOfficeChairClones
        ? findWorkChairPrototype(office, mt.hideGlbMeetingMeshSubstrings, workC, boardCenter)
        : null;
    this.useGlbChairs = Boolean(chairPrototype);
    if (import.meta.env.DEV && mt.useOfficeChairClones && !chairPrototype) {
      console.warn(
        '[MeetingRoomFurniture] useOfficeChairClones is true but no desk chair mesh was found in office.glb; using procedural chairs.'
      );
    }

    const topMat = new THREE.MeshStandardNodeMaterial({
      color: mt.tableTop,
      roughness: mt.roughnessTable,
      metalness: mt.metalnessTable,
    });
    const edgeMat = new THREE.MeshStandardNodeMaterial({
      color: mt.tableEdge,
      roughness: mt.roughnessTable,
      metalness: mt.metalnessTable,
    });
    const legMat = new THREE.MeshStandardNodeMaterial({
      color: mt.leg,
      roughness: mt.roughnessLeg,
      metalness: mt.metalnessLeg,
    });
    const seatMat = new THREE.MeshStandardNodeMaterial({
      color: mt.chairSeat,
      roughness: mt.roughnessChair,
      metalness: mt.metalnessChair,
    });
    const backMat = new THREE.MeshStandardNodeMaterial({
      color: mt.chairBack,
      roughness: mt.roughnessChair,
      metalness: mt.metalnessChair,
    });
    this.materials.push(topMat, edgeMat, legMat, seatMat, backMat);

    const tableL = 2.18;
    const tableH = 0.72;
    const topT = 0.038;
    const tableD = 0.98;

    const addBox = (
      mat: THREE.MeshStandardNodeMaterial,
      w: number,
      h: number,
      d: number,
      x: number,
      bottom: number,
      z: number
    ): void => {
      const g = new THREE.BoxGeometry(w, h, d);
      g.translate(x, bottom + h * 0.5, z);
      this.geometries.push(g);
      const m = new THREE.Mesh(g, mat);
      m.castShadow = true;
      m.receiveShadow = true;
      this.group.add(m);
    };

    addBox(topMat, tableL, topT, tableD, 0, tableH - topT, 0);
    addBox(edgeMat, tableL + 0.02, 0.028, tableD + 0.02, 0, tableH - topT - 0.014, 0);

    const legW = 0.07;
    const legD = 0.07;
    const legH = tableH - topT;
    const lx = tableL * 0.5 - legW * 0.5 - 0.04;
    const lz = tableD * 0.5 - legD * 0.5 - 0.03;
    addBox(legMat, legW, legH, legD, lx, 0, lz);
    addBox(legMat, legW, legH, legD, -lx, 0, lz);
    addBox(legMat, legW, legH, legD, lx, 0, -lz);
    addBox(legMat, legW, legH, legD, -lx, 0, -lz);

    const laptopBaseMat = new THREE.MeshStandardNodeMaterial({
      color: mt.leg,
      roughness: 0.5,
      metalness: 0.35,
    });
    const laptopScreenMat = new THREE.MeshStandardNodeMaterial({
      color: 0x243040,
      roughness: 0.4,
      metalness: 0.12,
    });
    this.materials.push(laptopBaseMat, laptopScreenMat);

    const halfL = tableL * 0.5;
    const halfD = tableD * 0.5;

    const up = new THREE.Vector3(0, 1, 0);

    const seatY = 0.45;
    const seatT = 0.055;
    const seatW = 0.44;
    const seatD = 0.42;
    const backH = 0.36;
    const backT = 0.05;

    type SeatDef = { lx: number; lz: number };
    const seatOutZ = halfD + seatD * 0.5 + 0.12;
    const seats: SeatDef[] = [
      { lx: -0.55, lz: -seatOutZ },
      { lx: 0.55, lz: -seatOutZ },
      { lx: -0.55, lz: seatOutZ },
      { lx: 0.55, lz: seatOutZ },
    ];

    /** Top of table slab (local Y); laptop bases sit here — not `tableH + epsilon`, which read as floating. */
    const tableSurfaceY = tableH;
    const laptopInset = 0.2;
    const lbW = 0.32;
    const lbH = 0.012;
    const lbD = 0.22;
    const lsW = 0.3;
    const lsH = 0.14;
    const lsT = 0.006;
    const laptopSeatIndices = [0, 1];
    for (const si of laptopSeatIndices) {
      const s = seats[si];
      const lp = new THREE.Group();
      if (Math.abs(s.lz) > Math.abs(s.lx) * 0.5) {
        lp.position.set(s.lx * 0.92, tableSurfaceY, Math.sign(s.lz) * (halfD - laptopInset));
      } else {
        lp.position.set(Math.sign(s.lx) * (halfL - laptopInset), tableSurfaceY, s.lz * 0.92);
      }
      const dx = s.lx - lp.position.x;
      const dz = s.lz - lp.position.z;
      lp.rotation.y = Math.atan2(dx, dz);

      const baseG = new THREE.BoxGeometry(lbW, lbH, lbD);
      baseG.translate(0, lbH * 0.5, 0);
      this.geometries.push(baseG);
      const baseMesh = new THREE.Mesh(baseG, laptopBaseMat);
      baseMesh.castShadow = true;
      baseMesh.receiveShadow = true;
      lp.add(baseMesh);

      const screenG = new THREE.BoxGeometry(lsW, lsH, lsT);
      const screenPivot = new THREE.Group();
      screenPivot.position.set(0, lbH, -lbD * 0.5 + lsT * 0.5);
      screenPivot.rotation.x = -0.38;
      screenG.translate(0, lsH * 0.5, 0);
      this.geometries.push(screenG);
      const screenMesh = new THREE.Mesh(screenG, laptopScreenMat);
      screenMesh.castShadow = true;
      screenMesh.receiveShadow = true;
      screenPivot.add(screenMesh);
      lp.add(screenPivot);

      this.group.add(lp);
    }

    let poiIndex = 0;
    for (const s of seats) {
      const cg = new THREE.Group();
      cg.position.set(s.lx, 0, s.lz);
      const yaw = Math.atan2(-s.lx, -s.lz);
      // Desk chairs in `office.glb` are authored with local forward opposite our procedural seat convention.
      if (chairPrototype) {
        cg.quaternion.setFromAxisAngle(up, yaw + Math.PI);
        const glbChair = chairPrototype.clone(true);
        resetCloneRootTransform(glbChair);
        glbChair.traverse((o) => {
          if ((o as THREE.Mesh).isMesh) {
            const m = o as THREE.Mesh;
            m.castShadow = true;
            m.receiveShadow = true;
          }
        });
        cg.add(glbChair);
        cg.userData.meetingChairGlb = glbChair;
      } else {
        cg.quaternion.setFromAxisAngle(up, yaw);
        const seatLegH = Math.max(0.04, seatY - seatT * 0.5 - 0.02);
        const slW = 0.042;
        const slX = seatW * 0.5 - slW * 0.5 - 0.01;
        const slZ = seatD * 0.5 - slW * 0.5 - 0.01;
        for (const [sx, sz] of [
          [slX, slZ],
          [-slX, slZ],
          [slX, -slZ],
          [-slX, -slZ],
        ] as const) {
          const lg = new THREE.BoxGeometry(slW, seatLegH, slW);
          lg.translate(sx, seatLegH * 0.5, sz);
          this.geometries.push(lg);
          const legMesh = new THREE.Mesh(lg, legMat);
          legMesh.castShadow = true;
          legMesh.receiveShadow = true;
          cg.add(legMesh);
        }

        const sg = new THREE.BoxGeometry(seatW, seatT, seatD);
        sg.translate(0, seatY + seatT * 0.5, 0);
        this.geometries.push(sg);
        const seatMesh = new THREE.Mesh(sg, seatMat);
        seatMesh.castShadow = true;
        seatMesh.receiveShadow = true;
        cg.add(seatMesh);

        const bg = new THREE.BoxGeometry(seatW, backH, backT);
        bg.translate(0, seatY + seatT + backH * 0.5, -seatD * 0.5 - backT * 0.5 + 0.02);
        this.geometries.push(bg);
        const backMesh = new THREE.Mesh(bg, backMat);
        backMesh.castShadow = true;
        backMesh.receiveShadow = true;
        cg.add(backMesh);
      }

      this.group.add(cg);
      this.seatGroups.push(cg);

      poiIndex += 1;
      const id = `${POI_PREFIX}-${String(poiIndex).padStart(2, '0')}`;
      this.poiIds.push(id);
      this.poiManager.addPoi({
        id,
        position: new THREE.Vector3(),
        quaternion: new THREE.Quaternion(),
        arrivalState: 'sit_idle',
        occupiedBy: null,
        label: 'Sit down',
      });
    }

    this.applyTablePose(this.theme.meetingRoom);
    this.syncSeatMeetingPois();
  }

  /** Uses `mt` for placement knobs only (offsets). Geometry is unchanged. */
  private applyTablePose(mt: SimulationTheme['meetingRoom']): void {
    const inps = this.poseInputs;
    if (!inps) return;

    const boardCenter = inps.boardCenter;
    const boardHalf = inps.boardHalf;
    const floorY = inps.floorY;
    const roomCenter = inps.roomCenter;
    const workC = inps.workC;

    const fromWorkToBoard = new THREE.Vector3().subVectors(boardCenter, workC);
    fromWorkToBoard.y = 0;

    const gap = 0.38;
    const tableD = 0.98;
    const dist = boardHalf + gap + tableD * 0.5;

    let tableOffsetDir = new THREE.Vector3();
    if (fromWorkToBoard.lengthSq() >= 0.04) {
      fromWorkToBoard.normalize();
      tableOffsetDir.copy(fromWorkToBoard).multiplyScalar(-1);
    } else {
      tableOffsetDir.subVectors(roomCenter, boardCenter);
      tableOffsetDir.y = 0;
      if (tableOffsetDir.lengthSq() < 1e-6) tableOffsetDir.set(0, 0, 1);
      else tableOffsetDir.normalize();
    }

    const tableCenter = boardCenter.clone().addScaledVector(tableOffsetDir, dist);
    tableCenter.y = floorY;

    const up = new THREE.Vector3(0, 1, 0);
    let zAxis = new THREE.Vector3().subVectors(boardCenter, tableCenter);
    zAxis.y = 0;
    if (zAxis.lengthSq() < 1e-6) zAxis.set(0, 0, 1);
    else zAxis.normalize();
    let xAxis = new THREE.Vector3().crossVectors(up, zAxis);
    if (xAxis.lengthSq() < 1e-8) xAxis.set(1, 0, 0);
    else xAxis.normalize();

    const gp = mt.groupPlacement;
    tableCenter.x += gp.position.x;
    tableCenter.y += gp.position.y;
    tableCenter.z += gp.position.z;

    zAxis.subVectors(boardCenter, tableCenter);
    zAxis.y = 0;
    if (zAxis.lengthSq() < 1e-6) zAxis.set(0, 0, 1);
    else zAxis.normalize();
    xAxis.crossVectors(up, zAxis);
    if (xAxis.lengthSq() < 1e-8) xAxis.set(1, 0, 0);
    else xAxis.normalize();

    if (mt.placementSnapForwardToCardinalAxes) {
      snapHorizontalForwardToCardinalXZ(zAxis);
      xAxis.crossVectors(up, zAxis);
      if (xAxis.lengthSq() < 1e-8) xAxis.set(1, 0, 0);
      else xAxis.normalize();
    }

    const themeKey = `${gp.position.x}|${gp.position.y}|${gp.position.z}|${gp.rotationYDegrees}|${mt.placementSnapForwardToCardinalAxes}`;
    if (import.meta.env.DEV && themeKey !== this.lastPlacementThemeKey) {
      this.lastPlacementThemeKey = themeKey;
      console.info(
        '[Meeting room] desk+chairs group world (m) — x:',
        tableCenter.x.toFixed(3),
        'y:',
        tableCenter.y.toFixed(3),
        'z:',
        tableCenter.z.toFixed(3),
        '| Configure: rooms/meeting/meetingRoom.config.ts → groupPlacement (+X=E, +Z=S) or mergeSimulationTheme.',
        '| Live: window.__delegationMeetingRoomWorld',
        'placement',
        gp.position.x,
        gp.position.y,
        gp.position.z,
        'yaw°',
        gp.rotationYDegrees,
        'snap',
        mt.placementSnapForwardToCardinalAxes
      );
    }

    const heading = Math.atan2(zAxis.x, zAxis.z);
    const qAlign = new THREE.Quaternion().setFromAxisAngle(up, heading);
    const yaw = THREE.MathUtils.degToRad(gp.rotationYDegrees);
    const qYaw = new THREE.Quaternion().setFromAxisAngle(up, yaw);

    this.group.position.copy(tableCenter);
    this.group.quaternion.multiplyQuaternions(qYaw, qAlign);
    if (import.meta.env.DEV) {
      window.__delegationMeetingRoomWorld = {
        x: this.group.position.x,
        y: this.group.position.y,
        z: this.group.position.z,
      };
    }
    this.alignGlbMeetingChairsToFloorOnce();
  }

  /** After the meeting group has its world pose, nudge cloned GLB chairs so their AABB bottom meets `floorY`. */
  private alignGlbMeetingChairsToFloorOnce(): void {
    if (this.glbChairsFloorAligned || !this.useGlbChairs || !this.poseInputs) return;
    const floorY = this.poseInputs.floorY;
    this.group.updateMatrixWorld(true);
    for (const cg of this.seatGroups) {
      const glb = cg.userData.meetingChairGlb as THREE.Object3D | undefined;
      if (!glb) continue;
      const box = new THREE.Box3().setFromObject(glb);
      if (box.isEmpty()) continue;
      glb.position.y += floorY - box.min.y;
    }
    this.glbChairsFloorAligned = true;
  }

  private syncSeatMeetingPois(): void {
    if (!this.poseInputs) return;
    const floorY = this.poseInputs.floorY;
    this.group.updateMatrixWorld(true);
    for (let i = 0; i < this.seatGroups.length; i++) {
      const cg = this.seatGroups[i];
      const id = this.poiIds[i];
      if (!cg || !id) continue;
      cg.updateMatrixWorld(true);
      const worldPos = new THREE.Vector3();
      cg.getWorldPosition(worldPos);
      worldPos.y = floorY;
      const qWorld = new THREE.Quaternion();
      cg.getWorldQuaternion(qWorld);
      const poi = this.poiManager.getPoi(id);
      if (poi) {
        poi.position.copy(worldPos);
        poi.quaternion.copy(qWorld);
      }
    }
  }

  update(_delta: number): void {
    if (!this.poseInputs) return;
    this.applyTablePose(this.theme.meetingRoom);
    this.syncSeatMeetingPois();
  }

  dispose(): void {
    for (const id of this.poiIds) this.poiManager.removePoi(id);
    this.poiIds.length = 0;
    this.seatGroups.length = 0;
    this.glbChairsFloorAligned = false;
    this.poseInputs = null;
    if (import.meta.env.DEV) {
      delete window.__delegationMeetingRoomWorld;
    }
    this.scene.remove(this.group);
    for (const g of this.geometries) g.dispose();
    this.geometries.length = 0;
    for (const m of this.materials) m.dispose();
    this.materials.length = 0;
  }
}
