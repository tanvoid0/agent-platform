import * as THREE from 'three/webgpu';
import {
  defaultLoungeKitchenLayout,
  type LoungeKitchenLayoutConfig,
} from './loungeKitchen.config';
import { Placement3Attachment } from '../../visual/placement3Attachment';
import {
  DEFAULT_SIMULATION_THEME,
  type LoungeKitchenVisualTheme,
  type SimulationTheme,
} from '../../visual/SimulationTheme';
import type { ISimulationWorldResource } from '../../visual/WorldResource';
import { officeLogicalName } from '../../visual/officeMeshUtils';
import { getBreakoutFurnitureBox } from '../../visual/officeZoneRegions';
import { PoiManager } from '../../world/PoiManager';

function meshCentroidByKeyword(office: THREE.Object3D, keyword: string): THREE.Vector3 | null {
  const box = new THREE.Box3();
  let found = false;
  office.traverse((child) => {
    if (!(child as THREE.Mesh).isMesh) return;
    const mesh = child as THREE.Mesh;
    const n = officeLogicalName(mesh);
    if (!n.includes(keyword)) return;
    const b = new THREE.Box3().setFromObject(mesh);
    if (b.isEmpty()) return;
    if (!found) {
      box.copy(b);
      found = true;
    } else box.union(b);
  });
  if (!found) return null;
  const c = new THREE.Vector3();
  box.getCenter(c);
  return c;
}

/** World-space center of lounge sofa mesh(es), if present in `office.glb`. */
function sofaCentroid(office: THREE.Object3D): THREE.Vector3 | null {
  return meshCentroidByKeyword(office, 'sofa');
}

/** Meshes that usually live in the breakout zone (names vary in `office.glb`). */
function mergedCentroidForTableLikeMeshes(office: THREE.Object3D): THREE.Vector3 | null {
  const box = new THREE.Box3();
  let found = false;
  office.traverse((child) => {
    if (!(child as THREE.Mesh).isMesh) return;
    const mesh = child as THREE.Mesh;
    const n = officeLogicalName(mesh);
    const tableLike =
      n.includes('cafe-table') ||
      n.includes('cafetable') ||
      n.includes('coffee-table') ||
      n.includes('coffee_table') ||
      n.includes('round-table') ||
      n.includes('round_table') ||
      (n.includes('round') && n.includes('table'));
    if (!tableLike) return;
    const b = new THREE.Box3().setFromObject(mesh);
    if (b.isEmpty()) return;
    if (!found) {
      box.copy(b);
      found = true;
    } else box.union(b);
  });
  if (!found) return null;
  const c = new THREE.Vector3();
  box.getCenter(c);
  return c;
}

/**
 * Prefer a point **deep in the living / breakout zone**, not the first POI name we find:
 * `poi-area-lounge` is sometimes authored near the wrong corner; café table + sofa are
 * usually next to the real lounge. Pick the candidate **farthest from the desk cluster** in XZ.
 */
function pickLoungeHintWorld(
  office: THREE.Object3D,
  ob: THREE.Box3,
  floorY: number,
  loungePoi: THREE.Vector3 | null,
  workC: THREE.Vector3 | null
): THREE.Vector3 {
  const candidates: THREE.Vector3[] = [];
  if (loungePoi) candidates.push(loungePoi.clone());
  const sofaC = sofaCentroid(office);
  const tableC = mergedCentroidForTableLikeMeshes(office);
  if (sofaC) candidates.push(sofaC);
  if (tableC) candidates.push(tableC);

  if (candidates.length === 0) {
    const c = new THREE.Vector3();
    ob.getCenter(c);
    c.y = floorY;
    return c;
  }

  if (!workC) {
    const avg = new THREE.Vector3();
    for (const p of candidates) avg.add(p);
    avg.multiplyScalar(1 / candidates.length);
    avg.y = floorY;
    return avg;
  }

  let best = candidates[0]!;
  let bestD = -1;
  for (const p of candidates) {
    const dx = p.x - workC.x;
    const dz = p.z - workC.z;
    const d2 = dx * dx + dz * dz;
    if (d2 > bestD) {
      bestD = d2;
      best = p;
    }
  }
  best.y = floorY;
  return best;
}

/** Distance from group origin to the back of the base cabinets (local −Z), see `addBox` layout. */
function cabinetBackOffsetZ(baseD: number): number {
  return baseD - 0.02;
}

function meshBlocksKitchenPlacement(mesh: THREE.Mesh): boolean {
  const n = officeLogicalName(mesh);
  if (n.includes('navmesh')) return false;
  if (n.includes('floor')) return false;
  if (n.includes('ceiling')) return false;
  if (!mesh.visible) return false;
  return true;
}

/** Shrink vertical extent so we mostly score furniture overlap, not the floor plane. */
function kitchenCollisionFootprint(group: THREE.Object3D, floorY: number): THREE.Box3 {
  const kb = new THREE.Box3().setFromObject(group);
  kb.min.y = Math.max(kb.min.y, floorY + 0.06);
  kb.max.y = Math.min(kb.max.y, floorY + 2.35);
  return kb;
}

/**
 * There is no semantic “living room” layer in the GLB — only mesh names and POIs.
 * We approximate collision by summing intersecting AABB volume with static meshes.
 */
function kitchenMeshOverlapPenalty(office: THREE.Object3D, kitchenWorldBox: THREE.Box3): number {
  let pen = 0;
  office.traverse((child) => {
    if (!(child as THREE.Mesh).isMesh) return;
    const mesh = child as THREE.Mesh;
    if (!meshBlocksKitchenPlacement(mesh)) return;
    const b = new THREE.Box3().setFromObject(mesh);
    if (b.isEmpty()) return;
    if (!kitchenWorldBox.intersectsBox(b)) return;
    const ix = Math.max(0, Math.min(kitchenWorldBox.max.x, b.max.x) - Math.max(kitchenWorldBox.min.x, b.min.x));
    const iz = Math.max(0, Math.min(kitchenWorldBox.max.z, b.max.z) - Math.max(kitchenWorldBox.min.z, b.min.z));
    const iy = Math.max(0, Math.min(kitchenWorldBox.max.y, b.max.y) - Math.max(kitchenWorldBox.min.y, b.min.y));
    pen += ix * iz * Math.max(iy, 0.18);
  });
  return pen;
}

/**
 * Extra cost per squared world-unit away from the lounge hint (XZ).
 * Without this, a wall deep in the open desk zone can win on overlap alone.
 */
const LOUNGE_HINT_DISTANCE_WEIGHT = 0.045;

/** Narrower run = gap before breakout plant / tub; must stay in sync with geometry below. */
const KITCHEN_RUN_WIDTH = 2.06;

const COFFEE_STEAM_COUNT = 7;
/** Uniform scale for the whole espresso prop (geometry + FX children). ~2× reads vs sink / faucet. */
const COFFEE_MACHINE_SCALE = 2.02;

/** POI id for nav + `walkToPoi` / hover — keep in sync with `NpcAgentDriver` coffee prefix. */
export const COFFEE_MACHINE_POI_ID = 'coffee-machine';

/**
 * World-space translation baked from former default `loungeKitchen.layout.worldNudge` +
 * `loungeKitchen.groupPlacement` for shipped `office.glb`. Theme defaults are zero; optional
 * `groupPlacement` / layout nudges still apply on top for custom floors.
 */
const DEFAULT_OFFICE_KITCHEN_WORLD_BIAS = new THREE.Vector3(-0.64, 0, -1.34);
/**
 * Applied after {@link resolveKitchenPose}: wall snap can pick the inward normal opposite to how this
 * GLB is authored; π flips the run so the counter / appliances face the open lounge instead of the partition.
 * Set to `0` if a future office orientation makes the default snap correct.
 */
const KITCHEN_FACING_Y_OFFSET_RAD = Math.PI;

function applyAlongRunWorldNudge(
  pos: THREE.Vector3,
  rotY: number,
  distance: number,
  bounds: THREE.Box3,
  floorY: number,
  runW: number,
  wallInset: number
): void {
  const wx = Math.cos(rotY);
  const wz = -Math.sin(rotY);
  pos.x += wx * distance;
  pos.z += wz * distance;
  pos.y = floorY;
  const alongHalf = runW * 0.5 + 0.2;
  const zMin = bounds.min.z + alongHalf + wallInset;
  const zMax = bounds.max.z - alongHalf - wallInset;
  const xMin = bounds.min.x + alongHalf + wallInset;
  const xMax = bounds.max.x - alongHalf - wallInset;
  if (Math.abs(wz) >= Math.abs(wx)) {
    pos.z = THREE.MathUtils.clamp(pos.z, zMin, zMax);
  } else {
    pos.x = THREE.MathUtils.clamp(pos.x, xMin, xMax);
  }
}

/** Local +Z opens toward the room — nudge off the back wall plane (reduces fridge / partition clip). */
function applyIntoRoomWorldNudge(
  pos: THREE.Vector3,
  rotY: number,
  distance: number,
  bounds: THREE.Box3,
  floorY: number,
  runW: number,
  wallInset: number
): void {
  const wx = Math.sin(rotY);
  const wz = Math.cos(rotY);
  pos.x += wx * distance;
  pos.z += wz * distance;
  pos.y = floorY;
  const alongHalf = runW * 0.5 + 0.2;
  const zMin = bounds.min.z + alongHalf + wallInset;
  const zMax = bounds.max.z - alongHalf - wallInset;
  const xMin = bounds.min.x + alongHalf + wallInset;
  const xMax = bounds.max.x - alongHalf - wallInset;
  if (Math.abs(wz) >= Math.abs(wx)) {
    pos.z = THREE.MathUtils.clamp(pos.z, zMin, zMax);
  } else {
    pos.x = THREE.MathUtils.clamp(pos.x, xMin, xMax);
  }
}

function linspace(a: number, b: number, n: number): number[] {
  if (n <= 1) return [THREE.MathUtils.clamp((a + b) * 0.5, Math.min(a, b), Math.max(a, b))];
  const out: number[] = [];
  for (let i = 0; i < n; i++) out.push(a + ((b - a) * i) / (n - 1));
  return out;
}

function wallSamples(hintCoord: number, minV: number, maxV: number): number[] {
  const c = THREE.MathUtils.clamp(hintCoord, minV, maxV);
  const merged = [c, ...linspace(minV, maxV, 12)];
  const seen = new Set<number>();
  const out: number[] = [];
  for (const v of merged) {
    const k = Math.round(v * 1000);
    if (seen.has(k)) continue;
    seen.add(k);
    out.push(v);
  }
  return out;
}

type WallDef = {
  id: 'minX' | 'maxX' | 'minZ' | 'maxZ';
  rotY: number;
  inward: THREE.Vector3;
  align: number;
};

/**
 * Prefer the **breakout / living** footprint (sofa + café tables) so the run doesn’t snap to the
 * full office shell — that was pulling the kitchen onto the manager / desk side when we nudged “back”.
 */
function kitchenPlacementBounds(office: THREE.Object3D, officeShell: THREE.Box3, runW: number, baseD: number): THREE.Box3 {
  const b = getBreakoutFurnitureBox(office);
  if (!b || b.isEmpty()) return officeShell;
  const sx = b.max.x - b.min.x;
  const sz = b.max.z - b.min.z;
  const longEdge = Math.max(sx, sz);
  const shortEdge = Math.min(sx, sz);
  if (longEdge < runW + 0.5 || shortEdge < baseD + 0.42) return officeShell;
  return b;
}

/**
 * Rank walls by desk→lounge alignment, then sweep along each edge and pick the pose with
 * least mesh overlap. Placement perimeter uses **breakout bounds** when available.
 */
function resolveKitchenPose(
  office: THREE.Object3D,
  officeShell: THREE.Box3,
  floorY: number,
  loungeHint: THREE.Vector3,
  workC: THREE.Vector3 | null,
  baseD: number,
  runW: number,
  group: THREE.Group,
  layout: LoungeKitchenLayoutConfig
): void {
  const pb = kitchenPlacementBounds(office, officeShell, runW, baseD);
  const backZ = cabinetBackOffsetZ(baseD);
  const inset = 0.1;
  const alongHalf = runW * 0.5 + 0.2;
  const cx = (pb.min.x + pb.max.x) * 0.5;
  const cz = (pb.min.z + pb.max.z) * 0.5;
  const roomCenter = new THREE.Vector3(cx, floorY, cz);

  const midMinX = new THREE.Vector3(pb.min.x + inset, floorY, cz);
  const midMaxX = new THREE.Vector3(pb.max.x - inset, floorY, cz);
  const midMinZ = new THREE.Vector3(cx, floorY, pb.min.z + inset);
  const midMaxZ = new THREE.Vector3(cx, floorY, pb.max.z - inset);

  const workAnchor = workC ?? roomCenter;
  const toLounge = new THREE.Vector3().subVectors(loungeHint, workAnchor);
  toLounge.y = 0;
  if (toLounge.lengthSq() < 1e-8) {
    toLounge.subVectors(loungeHint, roomCenter);
    toLounge.y = 0;
  }
  if (toLounge.lengthSq() < 1e-8) toLounge.set(1, 0, 0);
  else toLounge.normalize();

  const raw: WallDef[] = [
    { id: 'minX', rotY: Math.PI / 2, inward: new THREE.Vector3(1, 0, 0), align: 0 },
    { id: 'maxX', rotY: -Math.PI / 2, inward: new THREE.Vector3(-1, 0, 0), align: 0 },
    { id: 'minZ', rotY: 0, inward: new THREE.Vector3(0, 0, 1), align: 0 },
    { id: 'maxZ', rotY: Math.PI, inward: new THREE.Vector3(0, 0, -1), align: 0 },
  ];
  for (const w of raw) {
    w.align = w.inward.dot(toLounge);
    const mid =
      w.id === 'minX'
        ? midMinX
        : w.id === 'maxX'
          ? midMaxX
          : w.id === 'minZ'
            ? midMinZ
            : midMaxZ;
    const dx = workAnchor.x - mid.x;
    const dz = workAnchor.z - mid.z;
    w.align += (dx * dx + dz * dz) * 1e-6;
  }
  raw.sort((a, b) => b.align - a.align);

  const zMin = pb.min.z + alongHalf + inset;
  const zMax = pb.max.z - alongHalf - inset;
  const xMin = pb.min.x + alongHalf + inset;
  const xMax = pb.max.x - alongHalf - inset;

  let bestScore = Infinity;
  let bestPen = Infinity;
  let bestPos = new THREE.Vector3(0, floorY, 0);
  let bestRotY = 0;
  let bestWi = 999;
  let bestSi = 999;

  for (let wi = 0; wi < raw.length; wi++) {
    const w = raw[wi]!;
    const samples =
      w.id === 'minX' || w.id === 'maxX'
        ? wallSamples(loungeHint.z, zMin, zMax)
        : wallSamples(loungeHint.x, xMin, xMax);

    for (let si = 0; si < samples.length; si++) {
      const u = samples[si]!;
      const pos = new THREE.Vector3();
      pos.y = floorY;
      switch (w.id) {
        case 'minX':
          pos.set(pb.min.x + inset + backZ, floorY, u);
          break;
        case 'maxX':
          pos.set(pb.max.x - inset - backZ, floorY, u);
          break;
        case 'minZ':
          pos.set(u, floorY, pb.min.z + inset + backZ);
          break;
        case 'maxZ':
          pos.set(u, floorY, pb.max.z - inset - backZ);
          break;
      }
      group.position.copy(pos);
      group.rotation.y = w.rotY;
      group.updateMatrixWorld(true);
      const kb = kitchenCollisionFootprint(group, floorY);
      const pen = kitchenMeshOverlapPenalty(office, kb);
      const dx = pos.x - loungeHint.x;
      const dz = pos.z - loungeHint.z;
      const distSq = dx * dx + dz * dz;
      const score = pen + LOUNGE_HINT_DISTANCE_WEIGHT * distSq;
      const tieBreak =
        Math.abs(score - bestScore) <= 1e-6 &&
        (pen < bestPen - 1e-6 || (Math.abs(pen - bestPen) <= 1e-6 && (wi < bestWi || (wi === bestWi && si < bestSi))));
      if (score < bestScore - 1e-6 || tieBreak) {
        bestScore = score;
        bestPen = pen;
        bestPos.copy(pos);
        bestRotY = w.rotY;
        bestWi = wi;
        bestSi = si;
      }
    }
  }

  group.position.copy(bestPos);
  group.rotation.y = bestRotY;
  const loc = layout.afterWallSnapLocal;
  applyAlongRunWorldNudge(group.position, bestRotY, loc.position.x, pb, floorY, runW, inset);
  applyIntoRoomWorldNudge(group.position, bestRotY, loc.position.z, pb, floorY, runW, inset);
  group.position.y += loc.position.y;
  group.rotation.y += THREE.MathUtils.degToRad(loc.rotationYDegrees);

  const wn = layout.worldNudge;
  group.position.x += wn.position.x;
  group.position.y += wn.position.y;
  group.position.z += wn.position.z;
  group.rotation.y += THREE.MathUtils.degToRad(wn.rotationYDegrees);
}

/**
 * Procedural kitchen for the breakout zone. Wall snap prefers `getBreakoutFurnitureBox` over the full
 * office shell so the run stays in the living / café area instead of sliding along the global perimeter.
 *
 * Layout overrides: {@link LoungeKitchenLayoutConfig} + `groupPlacement` (optional; defaults are identity in
 * `loungeKitchen.config.ts`). Stock-office world offset: `DEFAULT_OFFICE_KITCHEN_WORLD_BIAS` in this file.
 */
export class LoungeKitchenVisual implements ISimulationWorldResource {
  readonly id = 'lounge-kitchen';
  readonly group = new THREE.Group();
  private readonly geometries: THREE.BufferGeometry[] = [];
  private readonly materials: THREE.MeshStandardNodeMaterial[] = [];
  private readonly steamMaterials: THREE.MeshStandardNodeMaterial[] = [];
  private steamGeometry: THREE.BufferGeometry | null = null;
  private readonly steamMeshes: THREE.Mesh[] = [];
  private readonly steamBase: THREE.Vector3[] = [];
  private readonly steamPhase: number[] = [];
  private steamTime = 0;
  private readonly kitchenFxTheme: Pick<LoungeKitchenVisualTheme, 'coffeeSteam' | 'coffeeSteamOpacity'>;

  constructor(
    private readonly scene: THREE.Scene,
    getOffice: () => THREE.Group | null,
    getLoungePoiWorld: () => THREE.Vector3 | null,
    getWorkClusterAnchor: () => THREE.Vector3 | null,
    private readonly poiManager: PoiManager,
    simulationTheme: SimulationTheme = DEFAULT_SIMULATION_THEME,
    kitchenLayout: LoungeKitchenLayoutConfig = defaultLoungeKitchenLayout
  ) {
    const kt = simulationTheme.loungeKitchen;
    this.kitchenFxTheme = {
      coffeeSteam: kt.coffeeSteam,
      coffeeSteamOpacity: kt.coffeeSteamOpacity,
    };

    const office = getOffice();
    if (!office) {
      throw new Error('LoungeKitchenVisual: office scene not loaded');
    }

    const ob = new THREE.Box3().setFromObject(office);
    const floorY = ob.min.y;

    const loungePoi = getLoungePoiWorld();
    const workC = getWorkClusterAnchor();
    const loungeHint = pickLoungeHintWorld(office, ob, floorY, loungePoi, workC);

    const baseD = 0.58;
    const runW = KITCHEN_RUN_WIDTH;

    const cab = new THREE.MeshStandardNodeMaterial({
      color: kt.cabinet,
      roughness: kt.roughnessCabinet,
      metalness: kt.metalnessCabinet,
    });
    const top = new THREE.MeshStandardNodeMaterial({
      /** Match café / breakout table wood from the office palette even if `loungeKitchen` is overridden. */
      color: simulationTheme.office.cafeWood,
      roughness: kt.roughnessCounter,
      metalness: kt.metalnessCounter,
    });
    const steel = new THREE.MeshStandardNodeMaterial({
      color: kt.steel,
      roughness: kt.roughnessSteel,
      metalness: kt.metalnessSteel,
    });
    const dark = new THREE.MeshStandardNodeMaterial({
      color: kt.dark,
      roughness: kt.roughnessDark,
      metalness: kt.metalnessDark,
    });
    const cook = new THREE.MeshStandardNodeMaterial({
      color: kt.cooktop,
      roughness: kt.roughnessCook,
      metalness: kt.metalnessCook,
      emissive: kt.cooktopEmissive,
      emissiveIntensity: kt.cooktopEmissiveIntensity,
    });
    const sinkSteel = new THREE.MeshStandardNodeMaterial({
      color: kt.steel,
      roughness: 0.4,
      metalness: 0.52,
    });
    const burner = new THREE.MeshStandardNodeMaterial({
      color: 0x1a1a1e,
      roughness: 0.55,
      metalness: 0.35,
      emissive: 0x2a1810,
      emissiveIntensity: 0.22,
    });
    const coffeeBody = new THREE.MeshStandardNodeMaterial({
      color: 0x1c1c22,
      roughness: 0.52,
      metalness: 0.32,
    });
    const coffeeLed = new THREE.MeshStandardNodeMaterial({
      color: 0x2a1810,
      emissive: kt.coffeeLightColor,
      emissiveIntensity: 0.62,
      roughness: 0.45,
      metalness: 0.2,
    });
    this.materials.push(cab, top, steel, dark, cook, sinkSteel, burner, coffeeBody, coffeeLed);

    /** Local space: group origin on floor; `bottom` = local y of mesh bottom. */
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

    /** Base cabinets along +Z as “front”; depth into −Z. */
    const baseH = 0.9;
    addBox(cab, runW, baseH, baseD, 0, 0, -baseD * 0.5 + 0.02);

    const topT = 0.038;
    const topSlabZ = -baseD * 0.5 + 0.02;
    const topSlabD = baseD + 0.08;
    addBox(top, runW + 0.06, topT, topSlabD, 0, baseH, topSlabZ);

    /** World Y of the **top surface** of the counter — `addBox(..., bottom, ...)` places mesh bottom at `bottom`. */
    const counterTop = baseH + topT;

    /**
     * Local Z for small props: must stay **inside** the countertop slab. Values like 0.07 sat past the
     * front edge and read as floating beside the unit from most angles.
     */
    const deckZ = topSlabZ + topSlabD * 0.28;

    /** Low backsplash rising from the counter plane (not a floating vertical slab). */
    const splashZ = -baseD + 0.012;
    addBox(cab, runW * 0.94, 0.26, 0.016, 0, counterTop, splashZ);

    const runLeftX = -runW * 0.5;
    const runRightX = runW * 0.5;

    const fridgeW = 0.66;
    const fridgeD = 0.62;
    /** Shorter than before so it reads as a counter-height / compact unit, not a full-height tower. */
    const fridgeH = 1.44;
    /**
     * Sit the fridge **past** the left end of the cabinet run with a small gap so the long counter
     * slab does not cut through the fridge box (reads as “beside” the kitchen, not inside it).
     */
    const fridgeGapFromRun = 0.06;
    const fx = runLeftX - fridgeGapFromRun - fridgeW * 0.5;
    addBox(steel, fridgeW, fridgeH, fridgeD, fx, 0, -baseD * 0.5 + 0.02);

    const cookW = 0.5;
    const cookD = 0.36;
    /** First appliance in from the left run edge — clears the fridge entirely. */
    const stoveInset = 0.12;
    const stoveX = runLeftX + stoveInset + cookW * 0.5;
    const cooktopT = 0.022;
    addBox(cook, cookW, cooktopT, cookD, stoveX, counterTop, deckZ);
    const burnerBottom = counterTop + cooktopT;
    for (const [ox, oz] of [
      [-0.14, -0.09],
      [0.14, -0.09],
      [-0.14, 0.09],
      [0.14, 0.09],
    ] as const) {
      addBox(burner, 0.095, 0.014, 0.095, stoveX + ox, burnerBottom, deckZ + oz);
    }

    const sinkW = 0.38;
    const sinkD = 0.28;
    /** Pull sink toward the right run end so there is clear counter between stove / coffee / sink. */
    const sinkInsetFromRight = 0.2;
    const sinkX = runRightX - sinkInsetFromRight - sinkW * 0.5;
    const sinkRimH = 0.038;
    addBox(sinkSteel, sinkW, sinkRimH, sinkD, sinkX, counterTop, deckZ);
    addBox(dark, 0.32, 0.022, 0.2, sinkX, counterTop + 0.01, deckZ + 0.006);
    const faucetH = 0.19;
    addBox(steel, 0.032, faucetH, 0.032, sinkX - 0.12, counterTop, deckZ);
    addBox(steel, 0.1, 0.026, 0.034, sinkX - 0.09, counterTop + faucetH, deckZ + 0.006);

    /**
     * Coffee machine + steam: **kitchen group local space** (metres, Y up).
     * Fridge is left of `runLeftX`; stove / coffee / sink are ordered left→right with explicit gaps.
     * `coffeeHalfX` is a conservative world half-width after {@link COFFEE_MACHINE_SCALE}.
     */
    const stoveRightX = stoveX + cookW * 0.5;
    const sinkLeftX = sinkX - sinkW * 0.5;
    const gapAppliance = 0.09;
    const coffeeHalfX = 0.19;
    const coffeeXMin = stoveRightX + gapAppliance + coffeeHalfX;
    const coffeeXMax = sinkLeftX - gapAppliance - coffeeHalfX;
    const coffeeX = THREE.MathUtils.clamp((stoveRightX + sinkLeftX) * 0.5, coffeeXMin, coffeeXMax);
    const coffeeRoot = new THREE.Group();
    coffeeRoot.position.set(coffeeX, counterTop, deckZ);
    this.group.add(coffeeRoot);

    const addCoffeeBox = (
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
      coffeeRoot.add(m);
    };

    const addCoffeeGeo = (g: THREE.BufferGeometry, mat: THREE.MeshStandardNodeMaterial): void => {
      this.geometries.push(g);
      const m = new THREE.Mesh(g, mat);
      m.castShadow = true;
      m.receiveShadow = true;
      coffeeRoot.add(m);
    };

    /**
     * Compact countertop espresso read: drip grate, open cup bay between pillars, top bridge,
     * chrome group head, portafilter sticking forward-left, steam wand on the right.
     * Local +Z = toward the room; keep steam / light forward so the fridge at −X does not eat the glow.
     */
    const trayB = 0.006;
    addCoffeeBox(steel, 0.168, 0.012, 0.2, 0, trayB, 0.028);
    for (let i = 0; i < 5; i++) {
      const gx = -0.062 + i * 0.031;
      addCoffeeBox(steel, 0.012, 0.004, 0.14, gx, trayB + 0.008, 0.03);
    }

    addCoffeeBox(coffeeBody, 0.118, 0.178, 0.026, 0, trayB + 0.006, -0.068);
    addCoffeeBox(coffeeBody, 0.026, 0.162, 0.132, -0.064, trayB + 0.012, -0.018);
    addCoffeeBox(coffeeBody, 0.026, 0.162, 0.132, 0.064, trayB + 0.012, -0.018);
    addCoffeeBox(coffeeBody, 0.124, 0.024, 0.15, 0, trayB + 0.168, -0.012);

    addCoffeeBox(coffeeLed, 0.082, 0.014, 0.008, 0, trayB + 0.172, 0.056);
    addCoffeeBox(dark, 0.034, 0.018, 0.034, 0.072, trayB + 0.158, 0.02);

    const groupHeadY = trayB + 0.118;
    const groupGeo = new THREE.CylinderGeometry(0.032, 0.034, 0.048, 12);
    groupGeo.rotateX(Math.PI / 2);
    groupGeo.translate(0, groupHeadY, 0.056);
    addCoffeeGeo(groupGeo, steel);

    addCoffeeBox(steel, 0.048, 0.022, 0.04, -0.012, trayB + 0.104, 0.052);
    addCoffeeBox(dark, 0.092, 0.026, 0.036, -0.058, trayB + 0.098, 0.056);

    addCoffeeBox(steel, 0.02, 0.088, 0.02, 0.076, trayB + 0.078, 0.012);
    addCoffeeBox(steel, 0.018, 0.024, 0.042, 0.084, trayB + 0.162, 0.032);

    /** Mug on the grate — sells “coffee” at a glance without extra materials. */
    addCoffeeBox(sinkSteel, 0.044, 0.038, 0.044, 0.028, trayB + 0.012, 0.048);
    addCoffeeBox(dark, 0.036, 0.028, 0.036, 0.028, trayB + 0.044, 0.048);

    const steamY = trayB + 0.174;
    const steamZ = 0.038;
    const steamX = 0.088;
    this.steamGeometry = new THREE.SphereGeometry(0.055, 7, 6);
    const steamOrigin = new THREE.Vector3(steamX, steamY, steamZ);
    for (let i = 0; i < COFFEE_STEAM_COUNT; i++) {
      const smMat = new THREE.MeshStandardNodeMaterial({
        color: kt.coffeeSteam,
        roughness: 1,
        metalness: 0,
        transparent: true,
        opacity: kt.coffeeSteamOpacity * 0.85,
        depthWrite: false,
      });
      this.steamMaterials.push(smMat);
      const sm = new THREE.Mesh(this.steamGeometry, smMat);
      sm.castShadow = false;
      sm.receiveShadow = false;
      const puffBase = steamOrigin
        .clone()
        .add(
          new THREE.Vector3(
            (Math.random() - 0.5) * 0.1,
            0,
            (Math.random() - 0.5) * 0.08
          )
        );
      this.steamBase.push(puffBase);
      this.steamPhase.push(Math.random() * Math.PI * 2);
      sm.position.copy(puffBase);
      coffeeRoot.add(sm);
      this.steamMeshes.push(sm);
    }

    const brewGlow = new THREE.PointLight(kt.coffeeLightColor, kt.coffeeLightIntensity * 0.92, 1.55, 2);
    brewGlow.position.set(0.02, groupHeadY + 0.02, 0.078);
    coffeeRoot.add(brewGlow);

    coffeeRoot.scale.setScalar(COFFEE_MACHINE_SCALE);

    resolveKitchenPose(office, ob, floorY, loungeHint, workC, baseD, runW, this.group, kitchenLayout);
    this.group.rotation.y += KITCHEN_FACING_Y_OFFSET_RAD;

    const gpAtt = new Placement3Attachment(simulationTheme.loungeKitchen.groupPlacement);
    this.group.rotation.y = gpAtt.composeWorldDelta(this.group, this.group.rotation.y);
    this.group.position.add(DEFAULT_OFFICE_KITCHEN_WORLD_BIAS);

    this.scene.add(this.group);
    this.group.updateMatrixWorld(true);
    this.registerCoffeeMachinePoi(coffeeRoot);
  }

  /** Stand point in front of the espresso machine; facing derived from coffee root +Z (toward room). */
  private registerCoffeeMachinePoi(coffeeRoot: THREE.Object3D): void {
    this.group.updateMatrixWorld(true);
    const machine = new THREE.Vector3();
    coffeeRoot.getWorldPosition(machine);
    const rootQuat = new THREE.Quaternion();
    coffeeRoot.getWorldQuaternion(rootQuat);
    const intoRoom = new THREE.Vector3(0, 0, 1).applyQuaternion(rootQuat);
    intoRoom.y = 0;
    if (intoRoom.lengthSq() < 1e-8) intoRoom.set(0, 0, 1);
    intoRoom.normalize();

    const footY = this.group.position.y;
    const stand = new THREE.Vector3().copy(machine).addScaledVector(intoRoom, 0.6);
    stand.y = footY;

    const lookTarget = machine.clone();
    lookTarget.y = footY + 0.95;
    const dir = new THREE.Vector3().subVectors(lookTarget, stand);
    dir.y = 0;
    if (dir.lengthSq() < 1e-8) dir.copy(intoRoom).multiplyScalar(-1);
    else dir.normalize();
    const faceQ = new THREE.Quaternion().setFromAxisAngle(new THREE.Vector3(0, 1, 0), Math.atan2(dir.x, dir.z));

    this.poiManager.addPoi({
      id: COFFEE_MACHINE_POI_ID,
      position: stand,
      quaternion: faceQ,
      arrivalState: 'pick',
      occupiedBy: null,
      label: 'Coffee',
    });
  }

  update(delta: number): void {
    this.steamTime += delta;
    const T = this.kitchenFxTheme;
    for (let i = 0; i < this.steamMeshes.length; i++) {
      const sm = this.steamMeshes[i]!;
      const base = this.steamBase[i]!;
      const ph = this.steamPhase[i]!;
      const rise = (this.steamTime * 0.26 + ph) % 2.2;
      const spread = rise * 0.16;
      sm.position.set(
        base.x + Math.sin(ph + this.steamTime * 0.9) * spread,
        base.y + rise * 0.28,
        base.z + Math.cos(ph * 0.9 + this.steamTime * 0.65) * spread
      );
      const s = 0.65 + rise * 0.55;
      sm.scale.setScalar(s);
      const mat = sm.material as THREE.MeshStandardNodeMaterial;
      mat.opacity = Math.max(0.08, T.coffeeSteamOpacity * (0.95 - rise * 0.28));
    }
  }

  dispose(): void {
    this.poiManager.removePoi(COFFEE_MACHINE_POI_ID);
    this.scene.remove(this.group);
    for (const g of this.geometries) g.dispose();
    for (const m of this.materials) m.dispose();
    for (const m of this.steamMaterials) m.dispose();
    this.steamGeometry?.dispose();
  }
}
