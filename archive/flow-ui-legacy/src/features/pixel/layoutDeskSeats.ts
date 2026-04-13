import type { PixelLayoutFurniture, PixelLayoutJson } from "./layoutTypes";

export const TILE_PX = 16;

/**
 * `PC_FRONT_OFF` is placed one tile to the right of `DESK_FRONT` in default layouts.
 * Returns desk index in the same order as {@link extractDeskSeatPixels}.
 */
export function deskIndexForPcFront(layout: PixelLayoutJson, pc: PixelLayoutFurniture): number | null {
  if (!pc.type.startsWith("PC_FRONT")) return null;
  const desks = layout.furniture.filter((f) => f.type === "DESK_FRONT");
  desks.sort((a, b) => a.row - b.row || a.col - b.col);
  const i = desks.findIndex((d) => d.row === pc.row && pc.col === d.col + 1);
  return i >= 0 ? i : null;
}

/**
 * Foot anchor pixels (bottom-center of 16×32 character) in front of each DESK_FRONT,
 * ordered by row then col (stable allocation order).
 */
export function extractDeskSeatPixels(layout: PixelLayoutJson): { x: number; y: number }[] {
  const desks = layout.furniture.filter((f) => f.type === "DESK_FRONT");
  desks.sort((a, b) => a.row - b.row || a.col - b.col);
  return desks.map((d) => {
    // Desk footprint 3×2 tiles; seat tile centered below (row+2), middle column.
    const seatX = d.col * TILE_PX + 24;
    const seatY = (d.row + 2) * TILE_PX + TILE_PX;
    return { x: seatX, y: seatY };
  });
}

/** Walkable spawn in the main wood area (approx. aisle center). */
export function defaultSpawnPixels(layout: PixelLayoutJson): { x: number; y: number } {
  return {
    x: Math.floor((layout.cols * TILE_PX) / 2),
    y: (layout.rows - 3) * TILE_PX + 8,
  };
}
