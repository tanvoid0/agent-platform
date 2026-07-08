import { describe, expect, it } from "vitest";

import type { PixelLayoutJson } from "./layoutTypes";
import { deskIndexForPcFront, extractDeskSeatPixels } from "./layoutDeskSeats";

describe("deskIndexForPcFront", () => {
  it("maps PC to the desk immediately to the left", () => {
    const layout: PixelLayoutJson = {
      version: 1,
      cols: 10,
      rows: 10,
      tiles: Array(100).fill(7),
      tileColors: [],
      furniture: [
        { uid: "d", type: "DESK_FRONT", col: 2, row: 5 },
        { uid: "p", type: "PC_FRONT_OFF", col: 3, row: 5 },
      ],
    };
    expect(deskIndexForPcFront(layout, layout.furniture[1]!)).toBe(0);
    expect(extractDeskSeatPixels(layout)).toHaveLength(1);
  });
});
