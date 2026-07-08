import type { PixelLayoutJson } from "./layoutTypes";
import { defaultSpawnPixels, TILE_PX } from "./layoutDeskSeats";

/** Void (outside the office). */
const TILE_VOID = 255;
/** Solid wall / non-walkable edge tiles in pixel-agents exports (see default-layout `tiles`). */
const TILE_WALL = 0;

export function isWalkableTileId(id: number): boolean {
  return id !== TILE_VOID && id !== TILE_WALL;
}

export function tileIndex(layout: PixelLayoutJson, col: number, row: number): number {
  return row * layout.cols + col;
}

export function tileIdAt(layout: PixelLayoutJson, col: number, row: number): number | null {
  if (col < 0 || row < 0 || col >= layout.cols || row >= layout.rows) return null;
  return layout.tiles[tileIndex(layout, col, row)]!;
}

function pixelToTile(x: number, y: number): { col: number; row: number } {
  return { col: Math.floor(x / TILE_PX), row: Math.floor(y / TILE_PX) };
}

function tileCenterPixel(col: number, row: number): { x: number; y: number } {
  return { x: col * TILE_PX + TILE_PX / 2, y: row * TILE_PX + TILE_PX / 2 };
}

/**
 * Find a nearby walkable tile when the sampled pixel lands on a wall/void cell (e.g. spawn in column 0).
 */
export function nearestWalkableTile(
  layout: PixelLayoutJson,
  col: number,
  row: number,
): { col: number; row: number } | null {
  if (isWalkableTileId(tileIdAt(layout, col, row) ?? TILE_VOID)) {
    return { col, row };
  }
  const maxCells = layout.cols * layout.rows;
  const seen = new Set<number>();
  const q: { col: number; row: number }[] = [{ col, row }];
  seen.add(tileIndex(layout, col, row));

  for (let qi = 0; qi < q.length && qi < maxCells; qi++) {
    const cur = q[qi]!;
    const nbs = [
      { col: cur.col - 1, row: cur.row },
      { col: cur.col + 1, row: cur.row },
      { col: cur.col, row: cur.row - 1 },
      { col: cur.col, row: cur.row + 1 },
    ];
    for (const n of nbs) {
      const id = tileIdAt(layout, n.col, n.row);
      if (id === null) continue;
      const k = tileIndex(layout, n.col, n.row);
      if (seen.has(k)) continue;
      seen.add(k);
      if (isWalkableTileId(id)) return { col: n.col, row: n.row };
      q.push(n);
    }
  }
  return null;
}

/** 4-neighbor BFS over walkable tiles; returns tile centers from start to goal (inclusive). */
export function bfsTilePath(
  layout: PixelLayoutJson,
  start: { col: number; row: number },
  goal: { col: number; row: number },
): { col: number; row: number }[] | null {
  const startOk = nearestWalkableTile(layout, start.col, start.row);
  const goalOk = nearestWalkableTile(layout, goal.col, goal.row);
  if (!startOk || !goalOk) return null;

  if (startOk.col === goalOk.col && startOk.row === goalOk.row) {
    return [startOk];
  }

  const goalKey = tileIndex(layout, goalOk.col, goalOk.row);
  const parent = new Map<number, number>();
  const q: { col: number; row: number }[] = [startOk];
  const startKey = tileIndex(layout, startOk.col, startOk.row);
  parent.set(startKey, -1);

  for (let qi = 0; qi < q.length; qi++) {
    const cur = q[qi]!;
    const curKey = tileIndex(layout, cur.col, cur.row);
    if (curKey === goalKey) {
      break;
    }
    const nbs = [
      { col: cur.col - 1, row: cur.row },
      { col: cur.col + 1, row: cur.row },
      { col: cur.col, row: cur.row - 1 },
      { col: cur.col, row: cur.row + 1 },
    ];
    for (const n of nbs) {
      const id = tileIdAt(layout, n.col, n.row);
      if (id === null || !isWalkableTileId(id)) continue;
      const nk = tileIndex(layout, n.col, n.row);
      if (parent.has(nk)) continue;
      parent.set(nk, curKey);
      q.push(n);
    }
  }

  if (!parent.has(goalKey)) return null;

  const outRev: { col: number; row: number }[] = [];
  let k = goalKey;
  while (k !== -1) {
    const row = Math.floor(k / layout.cols);
    const col = k % layout.cols;
    outRev.push({ col, row });
    const p = parent.get(k);
    if (p === undefined) break;
    k = p;
  }
  outRev.reverse();
  return outRev;
}

export function tilePathToPixelWaypoints(path: { col: number; row: number }[]): { x: number; y: number }[] {
  return path.map((t) => tileCenterPixel(t.col, t.row));
}

function polylineLength(points: readonly { x: number; y: number }[]): number {
  let len = 0;
  for (let i = 1; i < points.length; i++) {
    const a = points[i - 1]!;
    const b = points[i]!;
    len += Math.hypot(b.x - a.x, b.y - a.y);
  }
  return len;
}

/** @param t - in [0, 1] along total polyline length */
export function pointAlongPolyline(
  points: readonly { x: number; y: number }[],
  t: number,
): { x: number; y: number } {
  if (points.length === 0) return { x: 0, y: 0 };
  if (points.length === 1) return { ...points[0]! };
  const total = polylineLength(points);
  if (total <= 0) return { ...points[0]! };
  let d = Math.max(0, Math.min(1, t)) * total;
  for (let i = 1; i < points.length; i++) {
    const a = points[i - 1]!;
    const b = points[i]!;
    const seg = Math.hypot(b.x - a.x, b.y - a.y);
    if (d <= seg) {
      const u = seg > 0 ? d / seg : 0;
      return { x: a.x + (b.x - a.x) * u, y: a.y + (b.y - a.y) * u };
    }
    d -= seg;
  }
  const last = points[points.length - 1]!;
  return { x: last.x, y: last.y };
}

/**
 * Picks the heuristic bottom-center spawn, then snaps to the nearest walkable floor tile
 * so BFS paths from spawn are less likely to fail when layouts change.
 */
export function resolveWalkableSpawnPixels(layout: PixelLayoutJson): { x: number; y: number } {
  const raw = defaultSpawnPixels(layout);
  const col = Math.floor(raw.x / TILE_PX);
  const row = Math.floor(raw.y / TILE_PX);
  const fixed = nearestWalkableTile(layout, col, row);
  if (!fixed) return raw;
  return {
    x: fixed.col * TILE_PX + TILE_PX / 2,
    y: fixed.row * TILE_PX + TILE_PX / 2,
  };
}

export interface WalkPathBuildInput {
  layout: PixelLayoutJson;
  deskSeats: { x: number; y: number }[];
  spawn: { x: number; y: number };
}

/**
 * For each desk seat, build a pixel-center polyline from spawn to the approach point in front of the seat.
 * Falls back to `null` when BFS cannot connect (caller uses straight spawn→approach).
 */
export function buildWalkPathsPerDesk({ layout, deskSeats, spawn }: WalkPathBuildInput): (readonly {
  x: number;
  y: number;
}[] | null)[] {
  return deskSeats.map((seat) => {
    const approach = { x: seat.x, y: seat.y - 12 };
    const start = pixelToTile(spawn.x, spawn.y);
    const goal = pixelToTile(approach.x, approach.y);
    const tilePath = bfsTilePath(layout, start, goal);
    if (!tilePath || tilePath.length < 1) return null;
    const waypoints = tilePathToPixelWaypoints(tilePath);
    return waypoints;
  });
}
