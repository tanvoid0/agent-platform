import { describe, expect, it } from "vitest";

import { buildWallMask, TILE_WALL, TILE_VOID } from "./pixelTileSprites";

describe("buildWallMask", () => {
  it("returns 0 for isolated wall surrounded by non-walls", () => {
    const cols = 3;
    const tiles = [1, 1, 1, 1, TILE_WALL, 1, 1, 1, 1];
    expect(buildWallMask(1, 1, tiles, cols, 3)).toBe(0);
  });

  it("sets N bit when north neighbor is wall", () => {
    const cols = 2;
    const tiles = [TILE_WALL, TILE_WALL, TILE_WALL, 1];
    expect(buildWallMask(0, 1, tiles, cols, 2) & 1).toBe(1);
  });

  it("treats out-of-bounds as not wall", () => {
    const cols = 1;
    const tiles = [TILE_WALL];
    expect(buildWallMask(0, 0, tiles, cols, 1)).toBe(0);
  });

  it("treats void as not wall for neighbor checks", () => {
    const cols = 3;
    const tiles = [TILE_VOID, TILE_VOID, TILE_VOID, TILE_VOID, TILE_WALL, TILE_VOID, TILE_VOID, TILE_VOID, TILE_VOID];
    expect(buildWallMask(1, 1, tiles, cols, 3)).toBe(0);
  });
});
