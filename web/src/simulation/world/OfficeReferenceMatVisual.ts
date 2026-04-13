import * as THREE from 'three/webgpu';
import { DEFAULT_SIMULATION_THEME, type OfficeReferenceMatTheme, type SimulationTheme } from '../visual/SimulationTheme';
import type { ISimulationWorldResource } from '../visual/WorldResource';

const TEX = 768;

/**
 * Canvas UV on the mesh: **U → +X**, **V → +Z** (see `OfficeReferenceMatTheme` / `SimulationTheme`).
 * Cardinals fixed for this scene: **E +X**, **W −X**, **N −Z**, **S +Z**, **Y+ up**.
 */
function paintReferenceMatCanvas(opts: OfficeReferenceMatTheme): HTMLCanvasElement {
  const canvas = document.createElement('canvas');
  canvas.width = TEX;
  canvas.height = TEX;
  const ctx = canvas.getContext('2d');
  if (!ctx) return canvas;

  ctx.fillStyle = '#e4e2dc';
  ctx.fillRect(0, 0, TEX, TEX);

  ctx.strokeStyle = '#9c968a';
  ctx.lineWidth = 10;
  ctx.strokeRect(24, 24, TEX - 48, TEX - 48);

  const cx = TEX * 0.5;
  const cy = TEX * 0.5;
  const tick = TEX * 0.42;

  ctx.strokeStyle = '#c45c5c';
  ctx.lineWidth = 4;
  ctx.beginPath();
  ctx.moveTo(cx, cy);
  ctx.lineTo(cx + tick, cy);
  ctx.stroke();

  ctx.strokeStyle = '#5c7fc4';
  ctx.beginPath();
  ctx.moveTo(cx, cy);
  ctx.lineTo(cx, cy - tick);
  ctx.stroke();

  ctx.strokeStyle = 'rgba(80,80,90,0.35)';
  ctx.lineWidth = 2;
  for (let i = 1; i < 8; i++) {
    const g = (TEX / 8) * i;
    ctx.beginPath();
    ctx.moveTo(g, 24);
    ctx.lineTo(g, TEX - 24);
    ctx.stroke();
    ctx.beginPath();
    ctx.moveTo(24, g);
    ctx.lineTo(TEX - 24, g);
    ctx.stroke();
  }

  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.fillStyle = '#2a2d33';

  const main = 'bold 44px system-ui, "Segoe UI", sans-serif';
  const sub = '600 22px system-ui, "Segoe UI", sans-serif';
  const edge = TEX * 0.1;

  ctx.font = main;
  ctx.fillText('+X', TEX - edge, cy);
  ctx.fillText('−X', edge, cy);
  ctx.fillText('+Z', cx, edge);
  ctx.fillText('−Z', cx, TEX - edge);

  if (opts.showCardinalLetters) {
    ctx.font = sub;
    ctx.fillStyle = '#5a5d66';
    ctx.fillText('E', TEX - edge, cy + 34);
    ctx.fillText('W', edge, cy + 34);
    ctx.fillText('N', cx, edge + 30);
    ctx.fillText('S', cx, TEX - edge + 30);
  }

  ctx.font = '600 18px system-ui, "Segoe UI", sans-serif';
  ctx.fillStyle = '#6a6e78';
  ctx.fillText('World XZ  ·  Y+ is up', cx, cy - 8);
  ctx.font = '500 15px system-ui, "Segoe UI", sans-serif';
  ctx.fillText('Red → +X   Blue → +Z', cx, cy + 18);

  return canvas;
}

/**
 * Floor decoration: axis reference mat at office centroid so placement tweaks can be described
 * consistently (+X / −Z / “toward N”, etc.).
 */
export class OfficeReferenceMatVisual implements ISimulationWorldResource {
  readonly id = 'office-reference-mat';
  readonly group = new THREE.Group();
  private planeGeometry: THREE.PlaneGeometry | null = null;
  private material: THREE.MeshStandardNodeMaterial | null = null;
  private texture: THREE.CanvasTexture | null = null;

  constructor(
    private readonly scene: THREE.Scene,
    getOffice: () => THREE.Group | null,
    simulationTheme: SimulationTheme = DEFAULT_SIMULATION_THEME
  ) {
    const opts = simulationTheme.referenceMat;
    if (!opts.enabled) {
      return;
    }

    const office = getOffice();
    if (!office) {
      throw new Error('OfficeReferenceMatVisual: office scene not loaded');
    }

    const ob = new THREE.Box3().setFromObject(office);
    if (ob.isEmpty()) {
      return;
    }

    const floorY = ob.min.y;
    const cx = (ob.min.x + ob.max.x) * 0.5 + opts.worldOffsetX;
    const cz = (ob.min.z + ob.max.z) * 0.5 + opts.worldOffsetZ;

    const canvas = paintReferenceMatCanvas(opts);
    this.texture = new THREE.CanvasTexture(canvas);
    this.texture.colorSpace = THREE.SRGBColorSpace;
    this.texture.anisotropy = 4;

    this.material = new THREE.MeshStandardNodeMaterial({
      map: this.texture,
      roughness: opts.roughness,
      metalness: opts.metalness,
      transparent: opts.opacity < 1,
      opacity: opts.opacity,
    });

    this.planeGeometry = new THREE.PlaneGeometry(opts.width, opts.depth);
    const mesh = new THREE.Mesh(this.planeGeometry, this.material);
    mesh.rotation.x = -Math.PI / 2;
    mesh.position.set(cx, floorY + opts.elevation, cz);
    mesh.receiveShadow = true;
    mesh.castShadow = false;
    this.group.add(mesh);
    this.scene.add(this.group);
  }

  dispose(): void {
    this.scene.remove(this.group);
    this.planeGeometry?.dispose();
    this.planeGeometry = null;
    this.texture?.dispose();
    this.texture = null;
    this.material?.dispose();
    this.material = null;
  }
}
