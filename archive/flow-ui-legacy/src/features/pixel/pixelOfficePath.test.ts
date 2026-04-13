import { describe, expect, it } from "vitest";

import type { PixelLayoutJson } from "./layoutTypes";
import {
  bfsTilePath,
  buildWalkPathsPerDesk,
  isWalkableTileId,
  pointAlongPolyline,
  resolveWalkableSpawnPixels,
  tilePathToPixelWaypoints,
} from "./pixelOfficePath";

describe("isWalkableTileId", () => {
  it("treats void and wall as blocked", () => {
    expect(isWalkableTileId(255)).toBe(false);
    expect(isWalkableTileId(0)).toBe(false);
    expect(isWalkableTileId(7)).toBe(true);
    expect(isWalkableTileId(1)).toBe(true);
  });
});

describe("bfsTilePath", () => {
  it("finds a path between two floor tiles", () => {
    const layout: PixelLayoutJson = {
      version: 1,
      cols: 3,
      rows: 3,
      tiles: [
        255, 255, 255,
        255, 7, 255,
        255, 7, 255,
      ],
      tileColors: [],
      furniture: [],
    };
    const p = bfsTilePath(layout, { col: 1, row: 1 }, { col: 1, row: 2 });
    expect(p).not.toBeNull();
    expect(p!.length).toBeGreaterThanOrEqual(2);
    expect(p![0]).toEqual({ col: 1, row: 1 });
    expect(p![p!.length - 1]).toEqual({ col: 1, row: 2 });
  });

  it("returns null when goal is unreachable", () => {
    const layout: PixelLayoutJson = {
      version: 1,
      cols: 2,
      rows: 2,
      tiles: [7, 255, 255, 7],
      tileColors: [],
      furniture: [],
    };
    const p = bfsTilePath(layout, { col: 0, row: 0 }, { col: 1, row: 1 });
    expect(p).toBeNull();
  });
});

describe("pointAlongPolyline", () => {
  it("interpolates along segments", () => {
    const pts = [
      { x: 0, y: 0 },
      { x: 10, y: 0 },
    ];
    expect(pointAlongPolyline(pts, 0)).toEqual({ x: 0, y: 0 });
    expect(pointAlongPolyline(pts, 0.5)).toEqual({ x: 5, y: 0 });
    expect(pointAlongPolyline(pts, 1)).toEqual({ x: 10, y: 0 });
  });
});

describe("resolveWalkableSpawnPixels", () => {
  it("snaps spawn onto walkable floor when the heuristic lands on a wall", () => {
    const layout: PixelLayoutJson = {
      version: 1,
      cols: 5,
      rows: 5,
      tiles: Array(25).fill(255),
      tileColors: [],
      furniture: [],
    };
    for (let r = 1; r <= 3; r++) {
      for (let c = 1; c <= 3; c++) {
        layout.tiles[r * 5 + c] = 7;
      }
    }
    layout.tiles[2 * 5 + 2] = 0;

    const spawn = resolveWalkableSpawnPixels(layout);
    const col = Math.floor(spawn.x / 16);
    const row = Math.floor(spawn.y / 16);
    expect(isWalkableTileId(layout.tiles[row * 5 + col]!)).toBe(true);
  });
});

describe("buildWalkPathsPerDesk", () => {
  it("produces a waypoint polyline from spawn to desk approach", () => {
    const layout: PixelLayoutJson = {
      version: 1,
      cols: 5,
      rows: 5,
      tiles: Array(25).fill(255),
      tileColors: [],
      furniture: [],
    };
    for (let r = 1; r <= 3; r++) {
      for (let c = 1; c <= 3; c++) {
        layout.tiles[r * 5 + c] = 7;
      }
    }
    const deskSeats = [{ x: 1 * 16 + 24, y: (1 + 3) * 16 }];
    const spawn = { x: 40, y: 40 };
    const paths = buildWalkPathsPerDesk({ layout, deskSeats, spawn });
    expect(paths[0]).not.toBeNull();
    expect(paths[0]!.length).toBeGreaterThanOrEqual(2);
    expect(tilePathToPixelWaypoints(bfsTilePath(layout, { col: 2, row: 2 }, { col: 2, row: 3 })!)).toHaveLength(2);
  });
});
