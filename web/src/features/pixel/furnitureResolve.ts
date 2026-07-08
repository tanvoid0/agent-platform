/**
 * Maps pixel-agents layout `furniture[].type` strings to sprite files under
 * `public/pixel-agents/assets/furniture/<dir>/<file>`.
 */

export interface ResolvedFurnitureSprite {
  dir: string;
  file: string;
  w: number;
  h: number;
  /** Mirror horizontally (e.g. `:left` variants). */
  mirrorX: boolean;
}

const T = (dir: string, file: string, w: number, h: number): Omit<ResolvedFurnitureSprite, "mirrorX"> => ({
  dir,
  file,
  w,
  h,
});

/** Base type (before `:variant`) → sprite. Exported for manifest-driven overrides. */
export const FURNITURE_BASE: Record<string, Omit<ResolvedFurnitureSprite, "mirrorX">> = {
  TABLE_FRONT: T("TABLE_FRONT", "TABLE_FRONT.png", 48, 64),
  COFFEE_TABLE: T("COFFEE_TABLE", "COFFEE_TABLE.png", 32, 32),
  SOFA_FRONT: T("SOFA", "SOFA_FRONT.png", 32, 16),
  SOFA_BACK: T("SOFA", "SOFA_BACK.png", 32, 16),
  SOFA_SIDE: T("SOFA", "SOFA_SIDE.png", 16, 32),
  HANGING_PLANT: T("HANGING_PLANT", "HANGING_PLANT.png", 16, 32),
  DOUBLE_BOOKSHELF: T("DOUBLE_BOOKSHELF", "DOUBLE_BOOKSHELF.png", 32, 32),
  SMALL_PAINTING: T("SMALL_PAINTING", "SMALL_PAINTING.png", 16, 32),
  SMALL_PAINTING_2: T("SMALL_PAINTING_2", "SMALL_PAINTING_2.png", 16, 32),
  CLOCK: T("CLOCK", "CLOCK.png", 16, 32),
  PLANT: T("PLANT", "PLANT.png", 16, 32),
  PLANT_2: T("PLANT_2", "PLANT_2.png", 16, 32),
  COFFEE: T("COFFEE", "COFFEE.png", 16, 16),
  WOODEN_CHAIR_SIDE: T("WOODEN_CHAIR", "WOODEN_CHAIR_SIDE.png", 16, 32),
  DESK_FRONT: T("DESK", "DESK_FRONT.png", 48, 32),
  CUSHIONED_BENCH: T("CUSHIONED_BENCH", "CUSHIONED_BENCH.png", 16, 16),
  PC_FRONT_OFF: T("PC", "PC_FRONT_OFF.png", 16, 32),
  PC_SIDE: T("PC", "PC_SIDE.png", 16, 32),
  LARGE_PAINTING: T("LARGE_PAINTING", "LARGE_PAINTING.png", 32, 32),
  BIN: T("BIN", "BIN.png", 16, 16),
  SMALL_TABLE_FRONT: T("SMALL_TABLE", "SMALL_TABLE_FRONT.png", 32, 32),
  SMALL_TABLE_SIDE: T("SMALL_TABLE", "SMALL_TABLE_SIDE.png", 16, 48),
};

/**
 * @param layoutType - e.g. `DESK_FRONT` or `SOFA_SIDE:left`
 * @param overrides - Optional specs from {@link buildFurnitureSpecOverridesFromManifests} (per-base; `:left` still applied here).
 */
export function resolveFurnitureType(
  layoutType: string,
  overrides?: Map<string, ResolvedFurnitureSprite> | null,
): ResolvedFurnitureSprite | null {
  const [base, variant] = layoutType.split(":") as [string, string | undefined];
  const fromManifest = overrides?.get(base);
  const spec = fromManifest ?? FURNITURE_BASE[base];
  if (!spec) return null;
  return {
    dir: spec.dir,
    file: spec.file,
    w: spec.w,
    h: spec.h,
    mirrorX: variant === "left",
  };
}
