import type { PixelLayoutJson } from "./layoutTypes";
import type { ResolvedFurnitureSprite } from "./furnitureResolve";
import { FURNITURE_BASE } from "./furnitureResolve";

/**
 * Recursively collect pixel-agents manifest `asset` nodes (nested groups, e.g. PC on/off).
 */
export function collectAssetsFromManifest(manifest: unknown): Map<string, { file: string; width: number; height: number }> {
  const out = new Map<string, { file: string; width: number; height: number }>();
  function walk(node: unknown): void {
    if (!node || typeof node !== "object") return;
    const o = node as Record<string, unknown>;
    if (
      o.type === "asset" &&
      typeof o.id === "string" &&
      typeof o.file === "string" &&
      typeof o.width === "number" &&
      typeof o.height === "number"
    ) {
      out.set(o.id, { file: o.file, width: o.width, height: o.height });
    }
    const members = o.members;
    if (Array.isArray(members)) {
      for (const m of members) walk(m);
    }
  }
  walk(manifest);
  return out;
}

/**
 * Load `manifest.json` once per furniture folder used by the layout and build specs that
 * override the static {@link FURNITURE_BASE} map (dimensions / filenames from source of truth).
 */
export async function buildFurnitureSpecOverridesFromManifests(
  baseUrl: string,
  layout: PixelLayoutJson,
): Promise<Map<string, ResolvedFurnitureSprite>> {
  const out = new Map<string, ResolvedFurnitureSprite>();
  const bases = [...new Set(layout.furniture.map((f) => f.type.split(":")[0]!))];
  const dirsFetched = new Set<string>();

  for (const b of bases) {
    const entry = FURNITURE_BASE[b];
    if (!entry) continue;
    const { dir } = entry;
    if (dirsFetched.has(dir)) continue;
    dirsFetched.add(dir);

    try {
      const res = await fetch(`${baseUrl}pixel-agents/assets/furniture/${dir}/manifest.json`);
      if (!res.ok) continue;
      const manifest = (await res.json()) as unknown;
      const assets = collectAssetsFromManifest(manifest);
      for (const b2 of bases) {
        const e2 = FURNITURE_BASE[b2];
        if (!e2 || e2.dir !== dir) continue;
        const a = assets.get(b2);
        if (a) {
          out.set(b2, { dir, file: a.file, w: a.width, h: a.height, mirrorX: false });
        }
      }
    } catch {
      /* offline / missing manifest — keep static BASE */
    }
  }

  return out;
}
