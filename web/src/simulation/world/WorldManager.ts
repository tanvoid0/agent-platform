
import { DRACOLoader } from 'three/addons/loaders/DRACOLoader.js';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import * as THREE from 'three/webgpu';
import type { OfficeVisualStyle } from '../../types';
import { DRACO_LIB_PATH } from '../constants';
import { NavMeshManager } from '../pathfinding/NavMeshManager';
import { officeLogicalName } from '../visual/officeMeshUtils';
import { buildOfficeRoomOutlineDecor, type RoomOutlineBuildResult } from '../visual/officeRoomOutlineDecor';
import { getMeetingZoneCenter } from '../visual/officeZoneRegions';
import { OfficeSurfaceColorResolver } from '../visual/OfficeSurfaceColorResolver';
import { DEFAULT_SIMULATION_THEME, type SimulationTheme } from '../visual/SimulationTheme';
import { PoiManager } from './PoiManager';
import { TD_AI_SERVER_RACK_ROOT } from './serverRackOfficeAnchors';

export class WorldManager {
  private office: THREE.Group | null = null;
  /** Clips exported with `office.glb` (includes rack actions if authored in Blender). */
  private officeAnimations: THREE.AnimationClip[] = [];
  private readonly baseColors = new WeakMap<THREE.Mesh, THREE.Color>();
  private themeHex = '#888888';
  private visualStyle: OfficeVisualStyle = 'color';
  private readonly surfaceResolver: OfficeSurfaceColorResolver;
  private readonly simTheme: SimulationTheme;
  private roomOutlineDecor: RoomOutlineBuildResult | null = null;

  constructor(
    private scene: THREE.Scene,
    private navMesh: NavMeshManager,
    private poiManager: PoiManager,
    simulationTheme: SimulationTheme = DEFAULT_SIMULATION_THEME
  ) {
    this.simTheme = simulationTheme;
    this.surfaceResolver = new OfficeSurfaceColorResolver(simulationTheme.office);
  }

  public async load(look: { themeColor: string; style: OfficeVisualStyle }): Promise<void> {
    this.themeHex = look.themeColor;
    this.visualStyle = look.style;

    const loader = new GLTFLoader();
    const dracoLoader = new DRACOLoader();
    dracoLoader.setDecoderPath(DRACO_LIB_PATH);
    loader.setDRACOLoader(dracoLoader);
    const officeGltf = await loader.loadAsync(`${import.meta.env.BASE_URL}models/office.glb`);
    this.office = officeGltf.scene;
    this.officeAnimations = officeGltf.animations.slice();
    this.scene.add(this.office);

    const theme = new THREE.Color(this.themeHex);
    const matPreset = this.simTheme.officeMaterials;

    this.office.traverse((child) => {
      if (!(child as any).isMesh) return;
      const mesh = child as THREE.Mesh;
      const name = officeLogicalName(mesh);

      if (this.meshUnderServerRackHierarchy(mesh)) {
        this.applyOfficeShadowsAndVisibility(mesh, name, this.visualStyle);
        this.hideProceduralMeetingReplacedGlbMeshes(mesh, name);
        return;
      }

      if (name.includes('navmesh')) {
        this.navMesh.loadFromGeometry(mesh.geometry);
        mesh.visible = false;
        return;
      }

      if (mesh.material) {
        const oldMat = mesh.material as THREE.MeshStandardMaterial;
        this.baseColors.set(mesh, oldMat.color.clone());
        const color = this.surfaceResolver.resolve(name, this.baseColors.get(mesh)!, theme, this.visualStyle);
        const metal =
          this.visualStyle === 'performant' ? matPreset.metalnessPerformant : matPreset.metalnessFull;

        mesh.material = new THREE.MeshStandardNodeMaterial({
          color,
          map: oldMat.map,
          roughness: matPreset.roughness,
          metalness: metal,
        });
      }

      this.applyOfficeShadowsAndVisibility(mesh, name, this.visualStyle);
      this.hideProceduralMeetingReplacedGlbMeshes(mesh, name);
    });

    this.poiManager.loadFromGlb(this.office);
    this.applyFloorRoomZones();
    this.refreshRoomOutlineDecor();
  }

  private getLoungeWorldHint(): THREE.Vector3 | null {
    const hit =
      this.poiManager.getPoi('area-lounge') ??
      this.poiManager.getPoi('area-canteen') ??
      this.poiManager.getPoi('area-hub');
    if (hit) return hit.position.clone();
    const list = this.poiManager.getPoisByPrefix('area-').filter((p) => {
      const s = p.id.toLowerCase();
      return (
        s.includes('lounge') ||
        s.includes('canteen') ||
        s.includes('hub') ||
        s.includes('breakout')
      );
    });
    return list[0]?.position.clone() ?? null;
  }

  private getWorkClusterCentroid(): THREE.Vector3 | null {
    const w = this.poiManager.getPoisByPrefix('sit_work');
    if (w.length === 0) return null;
    const v = new THREE.Vector3();
    for (const p of w) v.add(p.position);
    return v.multiplyScalar(1 / w.length);
  }

  /**
   * Per floor mesh, nudge tint toward breakout vs work side using distance to lounge vs desk POIs.
   * (Single combined floor tile gets one dominant read.)
   */
  private applyFloorRoomZones(): void {
    if (!this.office) return;
    const lounge = this.getLoungeWorldHint();
    const work = this.getWorkClusterCentroid();
    if (!lounge || !work) return;

    const theme = new THREE.Color(this.themeHex);
    const p = this.simTheme.office;
    const fBreak = new THREE.Color(p.floorBreakoutTint);
    const fWork = new THREE.Color(p.floorWorkTint);
    const fMeet = new THREE.Color(p.floorMeetingTint);
    const officeBox = new THREE.Box3().setFromObject(this.office);
    const meeting = getMeetingZoneCenter(this.office, officeBox.min.y);

    this.office.traverse((child) => {
      if (!(child as THREE.Mesh).isMesh) return;
      const mesh = child as THREE.Mesh;
      const n = officeLogicalName(mesh);
      if (!n.includes('floor') || n.includes('navmesh')) return;
      const base = this.baseColors.get(mesh);
      if (!base || !mesh.material) return;

      const wb = new THREE.Box3().setFromObject(mesh);
      const c = new THREE.Vector3();
      wb.getCenter(c);
      const dl = (c.x - lounge.x) ** 2 + (c.z - lounge.z) ** 2;
      const dw = (c.x - work.x) ** 2 + (c.z - work.z) ** 2;
      const dm = meeting
        ? (c.x - meeting.x) ** 2 + (c.z - meeting.z) ** 2
        : Number.POSITIVE_INFINITY;

      const color = this.surfaceResolver.resolve(n, base, theme, this.visualStyle);
      if (meeting && dm <= dl && dm <= dw) {
        color.lerp(fMeet, p.floorMeetingLerp);
      } else if (dl < dw) {
        color.lerp(fBreak, p.floorBreakoutLerp);
      } else {
        color.lerp(fWork, p.floorWorkLerp);
      }

      const mat = mesh.material as THREE.MeshStandardNodeMaterial;
      if ((mat as any).color) (mat as any).color.copy(color);
    });
  }

  private refreshRoomOutlineDecor(): void {
    if (!this.office) return;
    this.roomOutlineDecor?.group.removeFromParent();
    this.roomOutlineDecor?.dispose();
    const floorY = new THREE.Box3().setFromObject(this.office).min.y;
    this.roomOutlineDecor = buildOfficeRoomOutlineDecor(
      this.office,
      floorY,
      this.getLoungeWorldHint(),
      this.simTheme.office
    );
    this.roomOutlineDecor.group.visible = this.visualStyle !== 'performant';
    this.office.add(this.roomOutlineDecor.group);
  }

  /**
   * Re-tint office props and toggle performant visibility/shadows when the team theme
   * or the user's office style preference changes.
   */
  public syncOfficeAppearance(themeColor: string, style: OfficeVisualStyle): void {
    this.themeHex = themeColor;
    this.visualStyle = style;
    if (!this.office) return;

    const theme = new THREE.Color(themeColor);
    const matPreset = this.simTheme.officeMaterials;

    this.office.traverse((child) => {
      if (!(child as any).isMesh) return;
      const mesh = child as THREE.Mesh;
      const name = officeLogicalName(mesh);
      if (name.includes('navmesh')) return;

      if (this.meshUnderServerRackHierarchy(mesh)) {
        this.applyOfficeShadowsAndVisibility(mesh, name, style);
        this.hideProceduralMeetingReplacedGlbMeshes(mesh, name);
        return;
      }

      const base = this.baseColors.get(mesh);
      if (!base || !mesh.material) return;

      const color = this.surfaceResolver.resolve(name, base, theme, style);
      const mat = mesh.material as THREE.MeshStandardNodeMaterial;
      if ((mat as any).color) (mat as any).color.copy(color);
      mat.metalness = style === 'performant' ? matPreset.metalnessPerformant : matPreset.metalnessFull;
      this.applyOfficeShadowsAndVisibility(mesh, name, style);
      this.hideProceduralMeetingReplacedGlbMeshes(mesh, name);
    });

    this.applyFloorRoomZones();
    this.refreshRoomOutlineDecor();
  }

  private applyOfficeShadowsAndVisibility(mesh: THREE.Mesh, logicalName: string, style: OfficeVisualStyle): void {
    const n = logicalName;
    if (style === 'performant') {
      mesh.visible = !n.includes('plant');
      const isFloor = n.includes('floor');
      mesh.castShadow = false;
      mesh.receiveShadow = isFloor;
    } else {
      mesh.visible = true;
      mesh.receiveShadow = true;
      mesh.castShadow = true;
    }
  }

  /**
   * Only when procedural meeting furniture is enabled: hide GLB meshes that would duplicate the runtime-placed set.
   */
  private hideProceduralMeetingReplacedGlbMeshes(mesh: THREE.Mesh, logicalName: string): void {
    if (!this.simTheme.meetingRoom.useProceduralMeetingFurniture) return;
    const n = logicalName;
    const frags = this.simTheme.meetingRoom.hideGlbMeetingMeshSubstrings;
    if (!frags.some((frag) => n.includes(frag))) return;
    mesh.visible = false;
    mesh.castShadow = false;
    mesh.receiveShadow = false;
  }

  public getOffice(): THREE.Group | null {
    return this.office;
  }

  public getOfficeAnimations(): readonly THREE.AnimationClip[] {
    return this.officeAnimations;
  }

  /** True when `mesh` is a descendant of the embedded AI rack root authored in `office.glb`. */
  public meshUnderServerRackHierarchy(mesh: THREE.Mesh): boolean {
    let o: THREE.Object3D | null = mesh;
    while (o) {
      if (o.name === TD_AI_SERVER_RACK_ROOT) return true;
      o = o.parent;
    }
    return false;
  }
}
