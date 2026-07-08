import { describe, expect, it } from "vitest";

import { PA_ROW_DOWN, PA_ROW_SIDE, PA_ROW_UP } from "./pixelSpriteConstants";
import { spriteFacingFromDelta } from "./pixelOfficeFoot";

describe("spriteFacingFromDelta", () => {
  it("uses side row for mostly horizontal motion and mirrors for left", () => {
    expect(spriteFacingFromDelta(8, 1)).toEqual({ spriteRow: PA_ROW_SIDE, mirrorWalk: false });
    expect(spriteFacingFromDelta(-8, 1)).toEqual({ spriteRow: PA_ROW_SIDE, mirrorWalk: true });
  });

  it("uses down/up rows for vertical motion (+y is down on canvas)", () => {
    expect(spriteFacingFromDelta(0, 10)).toEqual({ spriteRow: PA_ROW_DOWN, mirrorWalk: false });
    expect(spriteFacingFromDelta(0, -10)).toEqual({ spriteRow: PA_ROW_UP, mirrorWalk: false });
  });

  it("defaults to down when motion is negligible", () => {
    expect(spriteFacingFromDelta(0, 0)).toEqual({ spriteRow: PA_ROW_DOWN, mirrorWalk: false });
  });
});
