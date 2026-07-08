import { PA_CHAR_COUNT } from "./pixelSpriteConstants";

function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.decoding = "async";
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error(`Failed to load ${src}`));
    img.src = src;
  });
}

/** Load pixel-agents character PNGs from `public/pixel-agents/characters/` (Vite base-aware). */
export function loadPixelAgentImages(): Promise<HTMLImageElement[]> {
  const base = import.meta.env.BASE_URL;
  return Promise.all(
    Array.from({ length: PA_CHAR_COUNT }, (_, i) =>
      loadImage(`${base}pixel-agents/characters/char_${i}.png`),
    ),
  );
}

/** DESK_FRONT from pixel-agents furniture pack (MIT). */
export function loadDeskFrontImage(): Promise<HTMLImageElement> {
  const base = import.meta.env.BASE_URL;
  return loadImage(`${base}pixel-agents/assets/furniture/DESK/DESK_FRONT.png`);
}
