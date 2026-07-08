import { describe, expect, it } from "vitest";
import defaultLayout from "../../../public/pixel-agents/default-layout-1.json";
import type { PixelLayoutJson } from "./layoutTypes";
import {
  buildTileFillTable,
  colorizeMidGrayToHex,
  parseTileColor,
  solidFillForTile,
} from "./pixelTileColors";

describe("parseTileColor", () => {
  it("accepts HSBC objects from layout JSON", () => {
    expect(parseTileColor({ h: 214, s: 30, b: -100, c: -55 })).toEqual({
      h: 214,
      s: 30,
      b: -100,
      c: -55,
    });
  });

  it("rejects non-objects", () => {
    expect(parseTileColor(null)).toBeNull();
    expect(parseTileColor("x")).toBeNull();
  });
});

describe("colorizeMidGrayToHex", () => {
  it("is stable for a known wood-floor tint", () => {
    const hex = colorizeMidGrayToHex({ h: 25, s: 48, b: -43, c: -88 });
    expect(hex).toMatch(/^#[0-9a-f]{6}$/i);
    expect(hex.toLowerCase()).toBe("#6c4326");
  });
});

describe("solidFillForTile", () => {
  it("uses void color for 255 regardless of color payload", () => {
    expect(solidFillForTile(255, { h: 0, s: 100, b: 0, c: 0 })).toBe("#0c0a09");
  });

  it("falls back to defaults when tileColors entry is null", () => {
    expect(solidFillForTile(7, null)).toBe("#b8956a");
  });
});

describe("default-layout-1.json", () => {
  it("produces one fill per tile with no empty entries", () => {
    const layout = defaultLayout as PixelLayoutJson;
    const fills = buildTileFillTable(layout);
    expect(fills).toHaveLength(layout.cols * layout.rows);
    for (const f of fills) {
      expect(f).toMatch(/^#[0-9a-f]{6}$/i);
    }
  });
});
