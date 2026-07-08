/** HSBC tint from pixel-agents layout export (parallel to each tile). */
export interface TileColorValue {
  h: number;
  s: number;
  b: number;
  c: number;
}

/** Subset of pixel-agents `default-layout-1.json` (presentation-only). */
export interface PixelLayoutFurniture {
  uid: string;
  type: string;
  col: number;
  row: number;
}

export interface PixelLayoutJson {
  version: number;
  cols: number;
  rows: number;
  tiles: number[];
  /** Parallel to `tiles` (HSBC objects or `null` from export). */
  tileColors: unknown[];
  furniture: PixelLayoutFurniture[];
  layoutRevision?: number;
}
