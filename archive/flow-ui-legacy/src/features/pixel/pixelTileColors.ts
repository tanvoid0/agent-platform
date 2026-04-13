/**
 * Per-tile fill colors for pixel-agents layout JSON (`tileColors` parallel to `tiles`).
 * Matches the extension's "colorize from mid-gray" path used for walls and floor tints;
 * see pixel-agents `webview-ui/src/office/wallTiles.ts` → `wallColorToHex`.
 */

import type { PixelLayoutJson, TileColorValue } from "./layoutTypes";

/** Fallback when `tileColors[i]` is missing (legacy flat palette). */
export const DEFAULT_TILE_FILL: Record<number, string> = {
  0: "#78716c",
  1: "#e7e5e4",
  2: "#c4a574",
  3: "#d8b4fe",
  4: "#d6d3d1",
  5: "#a8a29e",
  6: "#94a3b8",
  7: "#b8956a",
  8: "#64748b",
  9: "#93c5fd",
  255: "#0c0a09",
};

function clamp255(v: number): number {
  return Math.max(0, Math.min(255, Math.round(v)));
}

/**
 * Same algorithm as pixel-agents `wallColorToHex`: 50% gray → HSL with HSBC sliders.
 */
export function colorizeMidGrayToHex(color: TileColorValue): string {
  const { h, s, b, c } = color;
  let lightness = 0.5;

  if (c !== 0) {
    const factor = (100 + c) / 100;
    lightness = 0.5 + (lightness - 0.5) * factor;
  }

  if (b !== 0) {
    lightness += b / 200;
  }

  lightness = Math.max(0, Math.min(1, lightness));

  const satFrac = s / 100;
  const ch = (1 - Math.abs(2 * lightness - 1)) * satFrac;
  const hp = h / 60;
  const x = ch * (1 - Math.abs((hp % 2) - 1));
  let r1 = 0;
  let g1 = 0;
  let b1 = 0;

  if (hp < 1) {
    r1 = ch;
    g1 = x;
  } else if (hp < 2) {
    r1 = x;
    g1 = ch;
  } else if (hp < 3) {
    g1 = ch;
    b1 = x;
  } else if (hp < 4) {
    g1 = x;
    b1 = ch;
  } else if (hp < 5) {
    r1 = x;
    b1 = ch;
  } else {
    r1 = ch;
    b1 = x;
  }

  const m = lightness - ch / 2;
  const clamp = (v: number) => clamp255((v + m) * 255);

  return `#${clamp(r1).toString(16).padStart(2, "0")}${clamp(g1).toString(16).padStart(2, "0")}${clamp(b1).toString(16).padStart(2, "0")}`;
}

export function parseTileColor(raw: unknown): TileColorValue | null {
  if (!raw || typeof raw !== "object") return null;
  const o = raw as Record<string, unknown>;
  const h = o.h;
  const s = o.s;
  const b = o.b;
  const c = o.c;
  if (typeof h !== "number" || typeof s !== "number" || typeof b !== "number" || typeof c !== "number") {
    return null;
  }
  return { h, s, b, c };
}

/**
 * Canvas fill for one cell. VOID (255) ignores `tileColors`; other tiles use colorize when set.
 */
export function solidFillForTile(tileId: number, colorRaw: unknown | null | undefined): string {
  if (tileId === 255) {
    return DEFAULT_TILE_FILL[255] ?? "#0c0a09";
  }
  const parsed = parseTileColor(colorRaw);
  if (!parsed) {
    return DEFAULT_TILE_FILL[tileId] ?? "#57534e";
  }
  return colorizeMidGrayToHex(parsed);
}

/** @returns parallel array of fill strings (same length as `tiles`). */
export function buildTileFillTable(layout: PixelLayoutJson): string[] {
  const { tiles, tileColors } = layout;
  const out: string[] = [];
  for (let i = 0; i < tiles.length; i++) {
    const id = tiles[i]!;
    out.push(solidFillForTile(id, tileColors[i]));
  }
  return out;
}

const fillTableCache = new WeakMap<PixelLayoutJson, string[]>();

/** Cached per layout instance; safe because layout objects are immutable after fetch. */
export function getTileFillTable(layout: PixelLayoutJson): string[] {
  let t = fillTableCache.get(layout);
  if (!t) {
    t = buildTileFillTable(layout);
    fillTableCache.set(layout, t);
  }
  return t;
}
