import { PA_CHAR_COUNT } from "./pixelSpriteConstants";

const cache = new Map<number, Promise<HTMLImageElement | null>>();

function loadCharSheetOnce(index: number): Promise<HTMLImageElement | null> {
  const base = import.meta.env.BASE_URL;
  return new Promise((resolve) => {
    const img = new Image();
    img.decoding = "async";
    img.onload = () => resolve(img.naturalWidth > 0 ? img : null);
    img.onerror = () => resolve(null);
    img.src = `${base}pixel-agents/characters/char_${index}.png`;
  });
}

/** Cached MIT pixel-agents character sheets for strip tiles (same paths as `loadPixelAgentImages`). */
export function getPixelStripCharSheet(charIndex: number): Promise<HTMLImageElement | null> {
  const i = ((charIndex % PA_CHAR_COUNT) + PA_CHAR_COUNT) % PA_CHAR_COUNT;
  let p = cache.get(i);
  if (!p) {
    p = loadCharSheetOnce(i);
    cache.set(i, p);
  }
  return p;
}
