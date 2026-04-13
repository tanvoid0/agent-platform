import * as THREE from 'three/webgpu';
import type { OfficeVisualStyle } from '../../types';
import type { OfficeSurfacePalette } from './SimulationTheme';

/**
 * Strategy for tinting office.glb surfaces from team theme + style.
 * Override by swapping palette on `SimulationTheme` or subclassing for custom rules.
 */
export class OfficeSurfaceColorResolver {
  constructor(private readonly palette: OfficeSurfacePalette) {}

  resolve(
    logicalName: string,
    base: THREE.Color,
    theme: THREE.Color,
    style: OfficeVisualStyle
  ): THREE.Color {
    const n = logicalName;
    const p = this.palette;
    const c = base.clone();

    const isComputerGear =
      n.includes('laptop') ||
      n.includes('pc') ||
      n.includes('macbook') ||
      n.includes('notebook') ||
      n.includes('imac') ||
      n.includes('monitor') ||
      n.includes('computer') ||
      n.includes('desktop') ||
      n.includes('tower') ||
      (n.includes('display') && !n.includes('white'));

    if (n.startsWith('colored') || n.includes('colored-') || n.includes('colored_')) {
      c.copy(theme);
    } else if (n.includes('plant')) {
      c.lerp(new THREE.Color(p.plantTint), p.plantLerp);
    } else if (n.includes('flexo')) {
      c.lerp(new THREE.Color(p.flexoWarm), p.flexoWarmLerp);
      c.lerp(theme, p.flexoThemeLerp);
    } else if (isComputerGear) {
      c.lerp(new THREE.Color(p.laptopDark), p.laptopLerp);
    } else if (n.includes('board') && !n.includes('keyboard')) {
      c.lerp(new THREE.Color(p.boardWood), p.boardLerp);
    } else if (n.includes('sofa')) {
      c.lerp(new THREE.Color(p.sofaFabric), p.sofaLerp);
    } else if (n.includes('work-desk')) {
      c.lerp(new THREE.Color(p.workDeskWood), p.workDeskLerp);
    } else if (n.includes('cafe-table') || n.includes('counter') || n.includes('cabinet')) {
      c.lerp(new THREE.Color(p.cafeWood), p.cafeLerp);
    } else if (n.includes('chair')) {
      c.lerp(theme, p.chairThemeLerp);
    } else if (n.includes('partition')) {
      c.lerp(new THREE.Color(p.partitionTint), p.partitionLerp);
    } else if (n.includes('floor')) {
      c.lerp(new THREE.Color(p.floorNeutral), p.floorNeutralLerp);
    }

    if (style === 'monochrome') {
      const lum = c.r * 0.2126 + c.g * 0.7152 + c.b * 0.0722;
      const gray = new THREE.Color(lum, lum, lum);
      c.lerp(gray, p.monochromeGrayLerp);
    }

    return c;
  }
}
