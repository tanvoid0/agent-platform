/**
 * Floor / wall textures aligned with pixel-agents `TileType` (0=wall, 1–9=floors, 255=void).
 * Walls use `walls/wall_0.png`: 4×4 grid of 16×32px autotile pieces (64×128 sheet).
 * Floors use `floors/floor_{n}.png` (16×16) for tile id `n+1`.
 */

import type { PixelLayoutJson } from "./layoutTypes";
import { TILE_PX } from "./layoutDeskSeats";
import { getTileFillTable } from "./pixelTileColors";

export const TILE_VOID = 255;
export const TILE_WALL = 0;
export const FLOOR_ID_MIN = 1;
export const FLOOR_ID_MAX = 9;

/** Wall sheet sprite height (extends above the 16px floor grid). */
export const WALL_SPRITE_H = 32;

/**
 * Neighbor bitmask: N=1, E=2, S=4, W=8. Out-of-bounds neighbors are not walls.
 * Matches pixel-agents `buildWallMask` / `wallTiles.ts`.
 */
export function buildWallMask(
  col: number,
  row: number,
  tiles: readonly number[],
  cols: number,
  rows: number,
): number {
  const isWall = (c: number, r: number): boolean => {
    if (c < 0 || r < 0 || c >= cols || r >= rows) return false;
    return tiles[r * cols + c] === TILE_WALL;
  };

  let mask = 0;
  if (isWall(col, row - 1)) mask |= 1;
  if (isWall(col + 1, row)) mask |= 2;
  if (isWall(col, row + 1)) mask |= 4;
  if (isWall(col - 1, row)) mask |= 8;
  return mask;
}

export function drawTexturedTileLayer(
  ctx: CanvasRenderingContext2D,
  layout: PixelLayoutJson,
  cache: Map<string, HTMLImageElement | null>,
): void {
  const { cols, tiles } = layout;
  const rows = Math.floor(tiles.length / cols);
  const fills = getTileFillTable(layout);
  ctx.imageSmoothingEnabled = false;

  for (let i = 0; i < tiles.length; i++) {
    const col = i % cols;
    const row = Math.floor(i / cols);
    const x = col * TILE_PX;
    const y = row * TILE_PX;
    const tid = tiles[i]!;
    const fill = fills[i] ?? "#57534e";

    if (tid === TILE_VOID) {
      ctx.fillStyle = fill;
      ctx.fillRect(x, y, TILE_PX, TILE_PX);
      continue;
    }

    if (tid >= FLOOR_ID_MIN && tid <= FLOOR_ID_MAX) {
      const key = `floors/floor_${tid - 1}.png`;
      const img = cache.get(key);
      if (img?.complete && img.naturalWidth > 0) {
        ctx.drawImage(img, x, y, TILE_PX, TILE_PX);
        ctx.globalCompositeOperation = "multiply";
        ctx.fillStyle = fill;
        ctx.fillRect(x, y, TILE_PX, TILE_PX);
        ctx.globalCompositeOperation = "source-over";
      } else {
        ctx.fillStyle = fill;
        ctx.fillRect(x, y, TILE_PX, TILE_PX);
      }
      continue;
    }

    if (tid === TILE_WALL) {
      const mask = buildWallMask(col, row, tiles, cols, rows);
      const sx = (mask % 4) * TILE_PX;
      const sy = Math.floor(mask / 4) * WALL_SPRITE_H;
      const wkey = "walls/wall_0.png";
      const wimg = cache.get(wkey);
      if (wimg?.complete && wimg.naturalWidth > 0) {
        const dy = y - (WALL_SPRITE_H - TILE_PX);
        ctx.drawImage(wimg, sx, sy, TILE_PX, WALL_SPRITE_H, x, dy, TILE_PX, WALL_SPRITE_H);
        ctx.globalCompositeOperation = "multiply";
        ctx.fillStyle = fill;
        ctx.fillRect(x, dy, TILE_PX, WALL_SPRITE_H);
        ctx.globalCompositeOperation = "source-over";
      } else {
        ctx.fillStyle = fill;
        ctx.fillRect(x, y, TILE_PX, TILE_PX);
      }
      continue;
    }

    ctx.fillStyle = fill;
    ctx.fillRect(x, y, TILE_PX, TILE_PX);
  }
}
