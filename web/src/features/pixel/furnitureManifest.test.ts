import { describe, expect, it } from "vitest";

import { collectAssetsFromManifest } from "./furnitureManifest";

describe("collectAssetsFromManifest", () => {
  it("collects flat DESK assets", () => {
    const manifest = {
      id: "DESK",
      members: [
        {
          type: "asset",
          id: "DESK_FRONT",
          file: "DESK_FRONT.png",
          width: 48,
          height: 32,
        },
      ],
    };
    const m = collectAssetsFromManifest(manifest);
    expect(m.get("DESK_FRONT")).toEqual({ file: "DESK_FRONT.png", width: 48, height: 32 });
  });

  it("collects nested PC assets", () => {
    const manifest = {
      id: "PC",
      members: [
        {
          type: "group",
          members: [
            {
              type: "asset",
              id: "PC_FRONT_OFF",
              file: "PC_FRONT_OFF.png",
              width: 16,
              height: 32,
            },
          ],
        },
      ],
    };
    const m = collectAssetsFromManifest(manifest);
    expect(m.get("PC_FRONT_OFF")).toEqual({ file: "PC_FRONT_OFF.png", width: 16, height: 32 });
  });
});
