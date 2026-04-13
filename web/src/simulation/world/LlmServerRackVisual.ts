import * as THREE from 'three/webgpu';
import type { ServerChatHealthState } from '../../integration/store/llmConnectivityStore';
import {
  SERVER_DECK_IDS,
  type ServerDeckId,
  type ServerDeckVisualMode,
  useServerRackPresentationStore,
} from '../../integration/store/serverRackPresentationStore';
import {
  DEFAULT_SIMULATION_THEME,
  type LlmRackLabelTheme,
  type LlmServerRackVisualTheme,
  type RackStatusAccent,
} from '../visual/SimulationTheme';
import { getMeetingZoneCenter, getWorkDeskZoneBox } from '../visual/officeZoneRegions';
import type { ISimulationWorldResource } from '../visual/WorldResource';
import {
  TD_AI_SERVER_RACK_ROOT,
  TD_DECK_OBJECT_NAMES,
  TD_RACK_ANIM_CLIP_PREFIX,
  TD_VFX_LABEL_MOUNT,
  TD_VFX_SMOKE_ORIGIN,
  TD_VFX_STATUS_LIGHT,
} from './serverRackOfficeAnchors';

const SMOKE_COUNT = 8;
const SLOW_PROBE_MS = 650;

const RACK_FOOTPRINT_W = 0.42;
const RACK_FOOTPRINT_D = 0.3;
const RACK_DESK_STANDOFF = 0.26;

function rackDeskClearanceMargin(): number {
  return RACK_DESK_STANDOFF + Math.max(RACK_FOOTPRINT_W, RACK_FOOTPRINT_D) * 0.5;
}

function insideExpandedDeskXZ(px: number, pz: number, desk: THREE.Box3, margin: number): boolean {
  return (
    px >= desk.min.x - margin &&
    px <= desk.max.x + margin &&
    pz >= desk.min.z - margin &&
    pz <= desk.max.z + margin
  );
}

function pushRackOutOfDeskZone(
  px: number,
  pz: number,
  desk: THREE.Box3,
  margin: number,
  dirX: number,
  dirZ: number
): { x: number; z: number } {
  let x = px;
  let z = pz;
  const len = Math.hypot(dirX, dirZ);
  const nx = len > 1e-5 ? dirX / len : 1;
  const nz = len > 1e-5 ? dirZ / len : 0;
  const step = 0.045;
  let i = 0;
  while (insideExpandedDeskXZ(x, z, desk, margin) && i++ < 100) {
    x += nx * step;
    z += nz * step;
  }
  return { x, z };
}

function bboxFloorCorners(box: THREE.Box3, floorY: number, pad: number): THREE.Vector3[] {
  return [
    new THREE.Vector3(box.min.x + pad, floorY, box.min.z + pad),
    new THREE.Vector3(box.max.x - pad, floorY, box.min.z + pad),
    new THREE.Vector3(box.min.x + pad, floorY, box.max.z - pad),
    new THREE.Vector3(box.max.x - pad, floorY, box.max.z - pad),
  ];
}

function cornerFarthestFrom(box: THREE.Box3, floorY: number, pad: number, x: number, z: number): THREE.Vector3 {
  const corners = bboxFloorCorners(box, floorY, pad);
  let best = corners[0]!;
  let bestD = -1;
  for (const c of corners) {
    const d = (c.x - x) ** 2 + (c.z - z) ** 2;
    if (d > bestD) {
      bestD = d;
      best = c;
    }
  }
  return best;
}

function pickWorkSideServerCorner(
  box: THREE.Box3,
  floorY: number,
  pad: number,
  workCluster: THREE.Vector3 | null,
  meetingCenter: THREE.Vector3 | null,
  loungeAnchor: THREE.Vector3 | null
): THREE.Vector3 {
  const corners = bboxFloorCorners(box, floorY, pad);
  const wMeet = 1.12;
  const wWork = 0.44;
  const wLounge = 0.09;
  let best = corners[0]!;
  let bestScore = -Infinity;
  for (const c of corners) {
    let s = (c.x + c.z) * 0.04;
    if (meetingCenter) {
      s += ((c.x - meetingCenter.x) ** 2 + (c.z - meetingCenter.z) ** 2) * wMeet;
    }
    if (workCluster) {
      s -= ((c.x - workCluster.x) ** 2 + (c.z - workCluster.z) ** 2) * wWork;
    }
    if (loungeAnchor) {
      s += ((c.x - loungeAnchor.x) ** 2 + (c.z - loungeAnchor.z) ** 2) * wLounge;
    }
    if (s > bestScore) {
      bestScore = s;
      best = c;
    }
  }
  return best;
}

function interiorServerPlacement(
  office: THREE.Object3D,
  camera: THREE.PerspectiveCamera,
  workClusterAnchor: THREE.Vector3 | null,
  loungeAnchor: THREE.Vector3 | null
): THREE.Vector3 {
  const box = new THREE.Box3().setFromObject(office);
  const floorY = box.min.y;
  const cx = (box.min.x + box.max.x) / 2;
  const cz = (box.min.z + box.max.z) / 2;
  const dx = box.max.x - box.min.x;
  const dz = box.max.z - box.min.z;
  const pad = 0.62;

  let px: number;
  let pz: number;
  let rackPushDirX = 1;
  let rackPushDirZ = 0;

  const meetingCenter = getMeetingZoneCenter(office, floorY);

  if (workClusterAnchor) {
    const corner = pickWorkSideServerCorner(box, floorY, pad, workClusterAnchor, meetingCenter, loungeAnchor);
    const deskBox = getWorkDeskZoneBox(office);
    const deskCx = deskBox && !deskBox.isEmpty() ? (deskBox.min.x + deskBox.max.x) * 0.5 : workClusterAnchor.x;
    const deskCz = deskBox && !deskBox.isEmpty() ? (deskBox.min.z + deskBox.max.z) * 0.5 : workClusterAnchor.z;
    let backDx = corner.x - deskCx;
    let backDz = corner.z - deskCz;
    const backLen = Math.hypot(backDx, backDz);
    if (backLen > 1e-4) {
      backDx /= backLen;
      backDz /= backLen;
    } else {
      backDx = 1;
      backDz = 0;
    }
    rackPushDirX = backDx;
    rackPushDirZ = backDz;

    px = THREE.MathUtils.lerp(corner.x, cx, 0.08);
    pz = THREE.MathUtils.lerp(corner.z, cz, 0.08);
    const wx = workClusterAnchor.x;
    const wz = workClusterAnchor.z;
    px = THREE.MathUtils.lerp(px, wx, 0.04);
    pz = THREE.MathUtils.lerp(pz, wz, 0.04);

    const behindAlongBack = 0.68;
    px += backDx * behindAlongBack;
    pz += backDz * behindAlongBack;
  } else if (loungeAnchor) {
    const wx = loungeAnchor.x;
    const wz = loungeAnchor.z;
    const corner = cornerFarthestFrom(box, floorY, pad, wx, wz);
    px = THREE.MathUtils.lerp(corner.x, cx, 0.1);
    pz = THREE.MathUtils.lerp(corner.z, cz, 0.1);
  } else {
    const toCam = new THREE.Vector2(camera.position.x - cx, camera.position.z - cz);
    if (toCam.lengthSq() < 1e-5) toCam.set(1, 0);
    else toCam.normalize();
    const move = Math.min(dx, dz) * 0.2;
    px = cx - toCam.x * move;
    pz = cz - toCam.y * move;
  }

  if (loungeAnchor) {
    const lx = loungeAnchor.x;
    const lz = loungeAnchor.z;
    const d = Math.hypot(px - lx, pz - lz);
    const minAway = 1.45;
    if (d < minAway) {
      const nx = px - lx;
      const nz = pz - lz;
      const len = Math.hypot(nx, nz) || 1;
      const push = minAway - d + 0.12;
      px += (nx / len) * push;
      pz += (nz / len) * push;
    }
  }

  const deskClear = rackDeskClearanceMargin();
  const deskZone = getWorkDeskZoneBox(office);
  if (workClusterAnchor && deskZone && !deskZone.isEmpty()) {
    const pushed = pushRackOutOfDeskZone(px, pz, deskZone, deskClear, rackPushDirX, rackPushDirZ);
    px = pushed.x;
    pz = pushed.z;
  }

  px = THREE.MathUtils.clamp(px, box.min.x + pad, box.max.x - pad);
  pz = THREE.MathUtils.clamp(pz, box.min.z + pad, box.max.z - pad);

  return new THREE.Vector3(px, floorY, pz);
}

const LABEL_W = 768;
const LABEL_H = 280;

function paintAiServerLabel(
  canvas: HTMLCanvasElement,
  status: ServerChatHealthState,
  lt: LlmRackLabelTheme
): void {
  const ctx = canvas.getContext('2d');
  if (!ctx) return;
  const w = canvas.width;
  const h = canvas.height;
  ctx.clearRect(0, 0, w, h);
  ctx.fillStyle = lt.panelBg;
  ctx.fillRect(12, 12, w - 24, h - 24);
  ctx.strokeStyle = lt.border;
  ctx.lineWidth = 3;
  ctx.strokeRect(12, 12, w - 24, h - 24);

  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.font = 'bold 56px system-ui, "Segoe UI", sans-serif';
  ctx.fillStyle = lt.title;
  ctx.fillText('AI SERVER', w / 2, 78);

  let sub: string;
  let color: string;
  if (status === 'ok') {
    sub = 'OLLAMA ONLINE';
    color = lt.statusOk;
  } else if (status === 'error') {
    sub = 'AI DOWN — OLLAMA OFFLINE';
    color = lt.statusError;
  } else if (status === 'idle') {
    sub = 'LOCAL LLM NOT ACTIVE';
    color = lt.statusIdle;
  } else {
    sub = 'CONNECTING…';
    color = lt.statusChecking;
  }
  ctx.font = 'bold 34px system-ui, "Segoe UI", sans-serif';
  ctx.fillStyle = color;
  ctx.fillText(sub, w / 2, 168);
}

function deckModesEqual(
  a: Record<ServerDeckId, ServerDeckVisualMode>,
  b: Record<ServerDeckId, ServerDeckVisualMode>
): boolean {
  return SERVER_DECK_IDS.every((id) => a[id] === b[id]);
}

function clipListsEqual(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((v, i) => v === b[i]!);
}

function setMaterialEmissive(mat: THREE.Material, emissive: number, intensity: number): void {
  const m = mat as THREE.MeshStandardMaterial & { emissiveIntensity?: number };
  if (m.emissive && typeof m.emissive.setHex === 'function') {
    m.emissive.setHex(emissive);
  }
  if (typeof m.emissiveIntensity === 'number') {
    m.emissiveIntensity = intensity;
  }
  m.needsUpdate = true;
}

type RackMode = 'embedded' | 'procedural';

/**
 * Local LLM health rack: either **embedded** in `office.glb` under {@link TD_AI_SERVER_RACK_ROOT} (Blender)
 * or **procedural** fallback when that node is absent.
 *
 * Embedded: three deck empties (`TD_Deck_*`), optional VFX mounts, and glTF `AnimationClip`s on the office file.
 */
export class LlmServerRackVisual implements ISimulationWorldResource {
  readonly id = 'llm-server-rack';
  /** Procedural rack root, or the embedded GLB root — useful for debugging. */
  readonly group: THREE.Object3D;

  private readonly mode: RackMode;

  private readonly rackMat: THREE.MeshStandardNodeMaterial | null = null;
  private readonly railMat: THREE.MeshStandardNodeMaterial | null = null;
  private readonly ventMat: THREE.MeshStandardNodeMaterial | null = null;
  private readonly seamGlowMat: THREE.MeshStandardNodeMaterial | null = null;
  private readonly ledMat: THREE.MeshStandardNodeMaterial | null = null;
  private readonly activityLedMat: THREE.MeshStandardNodeMaterial | null = null;

  private readonly statusGlow: THREE.PointLight;
  private readonly statusGlowSideL: THREE.PointLight;
  private readonly statusGlowSideR: THREE.PointLight;
  private readonly labelCanvas: HTMLCanvasElement;
  private readonly labelTexture: THREE.CanvasTexture;
  private readonly labelMaterial: THREE.MeshBasicMaterial;
  private readonly labelMesh: THREE.Mesh;
  private readonly smokeMaterials: THREE.MeshStandardNodeMaterial[] = [];
  private readonly rackGeometries: THREE.BufferGeometry[] = [];
  private ledGeometry: THREE.BufferGeometry | null = null;
  private labelGeometry: THREE.BufferGeometry | null = null;
  private smokeGeometry: THREE.BufferGeometry | null = null;
  private readonly smokeMeshes: THREE.Mesh[] = [];
  private readonly smokeBase: THREE.Vector3[] = [];
  private readonly smokePhase: number[] = [];

  private totalHeight: number;
  private readonly visualTheme: LlmServerRackVisualTheme;

  private time = 0;
  private status: ServerChatHealthState = 'checking';
  private lastResolved: 'ok' | 'error' | null = null;
  private checkingSince: number | null = null;
  private checkingSlowVisual = false;

  private presentationUnsub: (() => void) | null = null;
  private mixer: THREE.AnimationMixer | null = null;
  private readonly animActions = new Map<string, THREE.AnimationAction>();
  private readonly clipsByName = new Map<string, THREE.AnimationClip>();
  private readonly deckMeshMaterials: { deck: ServerDeckId; material: THREE.Material }[] = [];
  private readonly ownedMaterials: THREE.Material[] = [];

  constructor(
    getOffice: () => THREE.Group | null,
    camera: THREE.PerspectiveCamera,
    getWorkClusterAnchor: () => THREE.Vector3 | null,
    getLoungeAnchorWorld: () => THREE.Vector3 | null,
    getOfficeAnimations: () => readonly THREE.AnimationClip[],
    rackTheme: LlmServerRackVisualTheme = DEFAULT_SIMULATION_THEME.llmRack
  ) {
    this.visualTheme = rackTheme;
    const T = rackTheme;

    const office = getOffice();
    if (!office) {
      throw new Error('LlmServerRackVisual: office scene not loaded');
    }
    const embeddedRoot = office.getObjectByName(TD_AI_SERVER_RACK_ROOT);
    this.mode = embeddedRoot ? 'embedded' : 'procedural';

    let proceduralGroup: THREE.Group | undefined;

    if (this.mode === 'embedded') {
      this.group = embeddedRoot!;
      office.updateMatrixWorld(true);
      const box = new THREE.Box3().setFromObject(embeddedRoot!);
      this.totalHeight = Math.max(0.2, box.max.y - box.min.y);

      for (const clip of getOfficeAnimations()) {
        if (clip.name.startsWith(TD_RACK_ANIM_CLIP_PREFIX)) {
          this.clipsByName.set(clip.name, clip);
        }
      }
      this.mixer = new THREE.AnimationMixer(embeddedRoot!);

      this.bindEmbeddedDeckMaterials(embeddedRoot!);
    } else {
      proceduralGroup = new THREE.Group();
      proceduralGroup.name = 'llm-server-rack';
      this.group = proceduralGroup;

      const place = T.placement;
      office.updateMatrixWorld(true);
      const localPos = new THREE.Vector3();
      if (T.anchorToInteriorLayout) {
        const worldPos = interiorServerPlacement(office, camera, getWorkClusterAnchor(), getLoungeAnchorWorld());
        localPos.copy(worldPos);
        office.worldToLocal(localPos);
        localPos.x += place.position.x;
        localPos.y += place.position.y;
        localPos.z += place.position.z;
      } else {
        localPos.set(place.position.x, place.position.y, place.position.z);
      }
      proceduralGroup.position.copy(localPos);
      proceduralGroup.rotation.y = -Math.PI / 2 + THREE.MathUtils.degToRad(place.rotationYDegrees);
      office.add(proceduralGroup);

      this.rackMat = new THREE.MeshStandardNodeMaterial({
        color: T.chassis,
        roughness: 0.86,
        metalness: 0.14,
      });
      this.railMat = new THREE.MeshStandardNodeMaterial({
        color: T.railSteel,
        roughness: 0.55,
        metalness: 0.55,
      });
      this.ventMat = new THREE.MeshStandardNodeMaterial({
        color: T.ventVoid,
        roughness: 0.95,
        metalness: 0.15,
      });
      this.seamGlowMat = new THREE.MeshStandardNodeMaterial({
        color: T.seamFace,
        emissive: T.seamBaseEmissive,
        emissiveIntensity: T.seamBaseEmissiveIntensity,
        roughness: 1,
        metalness: 0,
      });
      this.ledMat = new THREE.MeshStandardNodeMaterial({
        color: T.chassisShadow,
        emissive: T.ledBaseEmissive,
        emissiveIntensity: T.ledBaseEmissiveIntensity,
        roughness: 0.55,
        metalness: 0.25,
      });
      this.activityLedMat = new THREE.MeshStandardNodeMaterial({
        color: T.activityLedFace,
        emissive: T.activityLedBaseEmissive,
        emissiveIntensity: T.activityLedBaseEmissiveIntensity,
        roughness: 0.4,
        metalness: 0.1,
      });

      const rackW = RACK_FOOTPRINT_W;
      const rackD = RACK_FOOTPRINT_D;
      const unitH = 0.186;
      const gap = 0.011;
      const nUnits = 6;
      this.totalHeight = nUnits * unitH + (nUnits - 1) * gap;

      const railT = 0.024;
      const railZ = rackD * 0.97;
      for (const sign of [-1, 1] as const) {
        const geo = new THREE.BoxGeometry(railT, this.totalHeight + 0.02, railZ);
        geo.translate(sign * (rackW * 0.5 + railT * 0.45), this.totalHeight * 0.5, 0);
        this.rackGeometries.push(geo);
        const rail = new THREE.Mesh(geo, this.railMat);
        rail.castShadow = true;
        rail.receiveShadow = true;
        proceduralGroup.add(rail);
      }

      const topLipGeo = new THREE.BoxGeometry(rackW + railT * 2.2, 0.026, rackD * 0.55);
      topLipGeo.translate(0, this.totalHeight + 0.018, rackD * 0.08);
      this.rackGeometries.push(topLipGeo);
      proceduralGroup.add(new THREE.Mesh(topLipGeo, this.railMat));

      const ventSlatT = 0.018;
      const ventSlatZ = 0.022;
      const ventW = rackW * 0.62;
      const zFace = rackD * 0.5 + ventSlatZ * 0.5 + 0.004;

      for (let i = 0; i < nUnits; i++) {
        const unitCy = unitH * 0.5 + i * (unitH + gap);
        const geo = new THREE.BoxGeometry(rackW * 0.92, unitH * 0.94, rackD);
        geo.translate(0, unitCy, 0);
        this.rackGeometries.push(geo);
        const shell = new THREE.Mesh(geo, this.rackMat!);
        shell.castShadow = true;
        shell.receiveShadow = true;
        proceduralGroup.add(shell);

        const nSlats = 4;
        const slatGap = 0.014;
        const stackH = nSlats * ventSlatT + (nSlats - 1) * slatGap;
        let sy = unitCy - stackH * 0.5 + ventSlatT * 0.5;
        for (let s = 0; s < nSlats; s++) {
          const sg = new THREE.BoxGeometry(ventW, ventSlatT, ventSlatZ);
          sg.translate(0, sy, zFace);
          this.rackGeometries.push(sg);
          const slat = new THREE.Mesh(sg, this.ventMat!);
          proceduralGroup.add(slat);
          sy += ventSlatT + slatGap;
        }
      }

      const seamH = 0.014;
      const seamW = rackW * 0.82;
      const seamZ = rackD * 0.95;
      for (let i = 0; i < nUnits - 1; i++) {
        const seamY = unitH * (i + 1) + gap * (i + 0.5);
        const geo = new THREE.BoxGeometry(seamW, seamH, seamZ);
        geo.translate(0, seamY, 0);
        this.rackGeometries.push(geo);
        proceduralGroup.add(new THREE.Mesh(geo, this.seamGlowMat!));
      }

      this.ledGeometry = new THREE.BoxGeometry(rackW * 0.68, 0.042, 0.024);
      const led = new THREE.Mesh(this.ledGeometry, this.ledMat!);
      led.position.set(0, this.totalHeight * 0.88, zFace + 0.008);
      proceduralGroup.add(led);

      const ledRowY = this.totalHeight * 0.42;
      const ledX0 = rackW * 0.26;
      for (let k = 0; k < 3; k++) {
        const dg = new THREE.BoxGeometry(0.044, 0.032, 0.018);
        dg.translate(ledX0 + k * 0.058, ledRowY, zFace + 0.01);
        this.rackGeometries.push(dg);
        proceduralGroup.add(new THREE.Mesh(dg, this.activityLedMat!));
      }
    }

    const rackW = RACK_FOOTPRINT_W;
    const rackD = RACK_FOOTPRINT_D;
    const zFace = rackD * 0.5 + 0.015;

    this.smokeGeometry = new THREE.SphereGeometry(0.06, 6, 5);
    const smokeMount =
      this.mode === 'embedded'
        ? embeddedRoot!.getObjectByName(TD_VFX_SMOKE_ORIGIN) ?? embeddedRoot!
        : proceduralGroup!;
    const smokeY = this.mode === 'procedural' ? this.totalHeight * 0.92 : 0;
    const smokeZ = this.mode === 'procedural' ? rackD * 0.28 : 0;
    const origin = new THREE.Vector3(0, smokeY, smokeZ);
    for (let i = 0; i < SMOKE_COUNT; i++) {
      const smMat = new THREE.MeshStandardNodeMaterial({
        color: T.smokeParticle,
        roughness: 1,
        metalness: 0,
        transparent: true,
        opacity: T.smokeOpacity,
        depthWrite: false,
      });
      this.smokeMaterials.push(smMat);
      const sm = new THREE.Mesh(this.smokeGeometry, smMat);
      sm.visible = false;
      this.smokeBase.push(
        origin.clone().add(new THREE.Vector3((Math.random() - 0.5) * 0.14, 0, (Math.random() - 0.5) * 0.1))
      );
      this.smokePhase.push(Math.random() * Math.PI * 2);
      this.smokeMeshes.push(sm);
      smokeMount.add(sm);
    }

    const glowY = this.mode === 'procedural' ? this.totalHeight * 0.52 : 0;
    const sideX = rackW * 0.48 + 0.024 * 0.5;
    const lightMount =
      this.mode === 'embedded'
        ? embeddedRoot!.getObjectByName(TD_VFX_STATUS_LIGHT) ?? embeddedRoot!
        : proceduralGroup!;

    this.statusGlow = new THREE.PointLight(0xffffff, 0, 7, 2);
    this.statusGlow.position.set(0, glowY, 0);
    lightMount.add(this.statusGlow);

    this.statusGlowSideL = new THREE.PointLight(0xffffff, 0, 5.2, 2);
    this.statusGlowSideL.position.set(this.mode === 'procedural' ? -sideX : 0, glowY, this.mode === 'procedural' ? 0 : 0);
    this.statusGlowSideR = new THREE.PointLight(0xffffff, 0, 5.2, 2);
    this.statusGlowSideR.position.set(this.mode === 'procedural' ? sideX : 0, glowY, this.mode === 'procedural' ? 0 : 0);
    if (this.mode === 'procedural') {
      lightMount.add(this.statusGlowSideL);
      lightMount.add(this.statusGlowSideR);
    } else {
      lightMount.add(this.statusGlowSideL);
      lightMount.add(this.statusGlowSideR);
      this.statusGlowSideL.position.set(-sideX, 0, 0);
      this.statusGlowSideR.position.set(sideX, 0, 0);
    }

    this.labelCanvas = document.createElement('canvas');
    this.labelCanvas.width = LABEL_W;
    this.labelCanvas.height = LABEL_H;
    this.labelTexture = new THREE.CanvasTexture(this.labelCanvas);
    this.labelTexture.colorSpace = THREE.SRGBColorSpace;
    this.labelTexture.minFilter = THREE.LinearFilter;
    this.labelTexture.magFilter = THREE.LinearFilter;

    this.labelMaterial = new THREE.MeshBasicMaterial({
      map: this.labelTexture,
      transparent: true,
      depthWrite: false,
      side: THREE.DoubleSide,
    });
    this.labelGeometry = new THREE.PlaneGeometry(rackW * 0.92, 0.2);
    this.labelMesh = new THREE.Mesh(this.labelGeometry, this.labelMaterial);
    if (this.mode === 'procedural') {
      this.labelMesh.position.set(0, this.totalHeight * 0.74, zFace + 0.042);
      this.labelMesh.rotation.x = THREE.MathUtils.degToRad(-8);
      proceduralGroup!.add(this.labelMesh);
    } else {
      const lm = embeddedRoot!.getObjectByName(TD_VFX_LABEL_MOUNT) ?? embeddedRoot!;
      this.labelMesh.position.set(0, 0, 0);
      this.labelMesh.rotation.set(0, 0, 0);
      lm.add(this.labelMesh);
    }

    this.presentationUnsub = useServerRackPresentationStore.subscribe((s, p) => {
      if (!deckModesEqual(s.deckMode, p.deckMode)) {
        this.applyDeckVisuals();
      }
      if (!clipListsEqual(s.loopingAnimationClipNames, p.loopingAnimationClipNames)) {
        this.syncLoopingClips();
      }
    });

    this.applyDeckVisuals();
    this.syncLoopingClips();
    this.applyStatusVisuals();
  }

  private bindEmbeddedDeckMaterials(rackRoot: THREE.Object3D): void {
    const deckEntries: [ServerDeckId, string][] = [
      ['llm', TD_DECK_OBJECT_NAMES.llm],
      ['backend', TD_DECK_OBJECT_NAMES.backend],
      ['database', TD_DECK_OBJECT_NAMES.database],
    ];
    for (const [deckId, objName] of deckEntries) {
      const sub = rackRoot.getObjectByName(objName);
      if (!sub) continue;
      sub.traverse((child) => {
        const m = child as THREE.Mesh;
        if (!(m as any).isMesh) return;
        const mats = Array.isArray(m.material) ? m.material : [m.material];
        const nextMats = mats.map((mat) => {
          const c = mat.clone();
          this.ownedMaterials.push(c);
          return c;
        });
        m.material = nextMats.length === 1 ? nextMats[0]! : nextMats;
        for (const mat of nextMats) {
          this.deckMeshMaterials.push({ deck: deckId, material: mat });
        }
      });
    }
  }

  private applyDeckVisuals(): void {
    if (this.mode !== 'embedded' || this.deckMeshMaterials.length === 0) return;
    const modes = useServerRackPresentationStore.getState().deckMode;
    const pal = this.visualTheme.serverDeck;
    for (const { deck, material } of this.deckMeshMaterials) {
      const mode = modes[deck];
      const spec = pal[mode];
      setMaterialEmissive(material, spec.emissive, spec.emissiveIntensity);
    }
  }

  private syncLoopingClips(): void {
    if (!this.mixer) return;
    const want = new Set(useServerRackPresentationStore.getState().loopingAnimationClipNames);
    for (const [name, clip] of this.clipsByName) {
      let action = this.animActions.get(name);
      if (!action) {
        action = this.mixer.clipAction(clip);
        this.animActions.set(name, action);
      }
      if (want.has(name)) {
        action.loop = THREE.LoopRepeat;
        action.clampWhenFinished = false;
        action.reset().fadeIn(0.28).play();
      } else if (action.isRunning() || action.getEffectiveWeight() > 0) {
        action.fadeOut(0.25);
      }
    }
  }

  setConnectionState(s: ServerChatHealthState): void {
    const prev = this.status;
    this.status = s;
    if (s === 'checking') {
      if (prev !== 'checking') {
        this.checkingSince = performance.now();
        this.checkingSlowVisual = false;
      }
    } else {
      this.checkingSince = null;
      this.checkingSlowVisual = false;
      if (s === 'ok' || s === 'error') this.lastResolved = s;
      if (s === 'idle') this.lastResolved = null;
    }
    this.applyStatusVisuals();
  }

  private pickStatusAccent(): RackStatusAccent {
    const S = this.visualTheme.status;
    if (this.status === 'ok') return S.ok;
    if (this.status === 'error') return S.error;
    if (this.status === 'idle') return S.idle;
    if (this.lastResolved === null) return S.checking;
    const since = this.checkingSince ?? 0;
    if (performance.now() - since >= SLOW_PROBE_MS) return S.checking;
    return this.lastResolved === 'ok' ? S.ok : S.error;
  }

  private applyStatusVisuals(): void {
    const smokeOn = this.status === 'error';
    for (const sm of this.smokeMeshes) sm.visible = smokeOn;

    const accent = this.pickStatusAccent();

    if (this.seamGlowMat && this.ledMat && this.activityLedMat) {
      this.seamGlowMat.emissive.setHex(accent.seamEmissive);
      this.seamGlowMat.emissiveIntensity = accent.seamIntensity;
      this.ledMat.emissive.setHex(accent.ledEmissive);
      this.ledMat.emissiveIntensity = accent.ledIntensity;
      this.activityLedMat.emissive.setHex(accent.activityEmissive);
      this.activityLedMat.emissiveIntensity = accent.activityIntensity;
      this.seamGlowMat.needsUpdate = true;
      this.ledMat.needsUpdate = true;
      this.activityLedMat.needsUpdate = true;
    }

    this.statusGlow.color.setHex(accent.pointLight);
    this.statusGlow.intensity = accent.pointIntensity;
    const sideMul = 0.48;
    this.statusGlowSideL.color.copy(this.statusGlow.color);
    this.statusGlowSideR.color.copy(this.statusGlow.color);
    this.statusGlowSideL.intensity = this.statusGlow.intensity * sideMul;
    this.statusGlowSideR.intensity = this.statusGlow.intensity * sideMul;

    paintAiServerLabel(this.labelCanvas, this.status, this.visualTheme.label);
    this.labelTexture.needsUpdate = true;
  }

  update(delta: number): void {
    if (this.mixer) {
      this.mixer.update(delta);
    }

    if (this.status === 'checking' && this.lastResolved !== null && this.checkingSince !== null) {
      const slowNow = performance.now() - this.checkingSince >= SLOW_PROBE_MS;
      if (slowNow !== this.checkingSlowVisual) {
        this.checkingSlowVisual = slowNow;
        this.applyStatusVisuals();
      }
    }

    if (this.status !== 'error') return;
    this.time += delta;
    const T = this.visualTheme;
    for (let i = 0; i < this.smokeMeshes.length; i++) {
      const sm = this.smokeMeshes[i];
      const base = this.smokeBase[i]!;
      const ph = this.smokePhase[i]!;
      const rise = (this.time * 0.3 + ph) % 2.5;
      const spread = rise * 0.2;
      sm.position.set(
        base.x + Math.sin(ph + this.time) * spread,
        base.y + rise * 0.34,
        base.z + Math.cos(ph * 0.88 + this.time * 0.75) * spread
      );
      const s = 1 + rise * 0.46;
      sm.scale.setScalar(s);
      const mat = sm.material as THREE.MeshStandardNodeMaterial;
      mat.opacity = Math.max(0.22, T.smokeOpacity - rise * 0.13);
    }
  }

  dispose(): void {
    this.presentationUnsub?.();
    this.presentationUnsub = null;

    this.mixer?.stopAllAction();
    this.animActions.clear();
    this.mixer = null;

    for (const sm of this.smokeMeshes) {
      sm.removeFromParent();
    }
    this.labelMesh.removeFromParent();
    this.statusGlow.removeFromParent();
    this.statusGlowSideL.removeFromParent();
    this.statusGlowSideR.removeFromParent();

    if (this.mode === 'procedural') {
      this.group.removeFromParent();
      for (const g of this.rackGeometries) g.dispose();
    }

    for (const m of this.ownedMaterials) m.dispose();

    this.ledGeometry?.dispose();
    this.labelGeometry?.dispose();
    this.labelTexture.dispose();
    this.labelMaterial.dispose();
    this.rackMat?.dispose();
    this.railMat?.dispose();
    this.ventMat?.dispose();
    this.seamGlowMat?.dispose();
    this.ledMat?.dispose();
    this.activityLedMat?.dispose();
    for (const m of this.smokeMaterials) m.dispose();
    this.smokeGeometry?.dispose();
  }
}
