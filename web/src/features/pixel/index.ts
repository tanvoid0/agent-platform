export { default } from "./PixelProcessStrip";
/** Alias for docs/plan naming (“live run” task tile strip). */
export { default as PixelRunStrip } from "./PixelProcessStrip";
export { PixelChibiTile } from "./PixelChibiTile";
export { default as PixelHomeTeaser } from "./PixelHomeTeaser";
export { PixelErrorBoundary } from "./PixelErrorBoundary";
export type { PixelActivity, PixelAgent, PixelEventHints, PixelSceneState } from "./pixelViewModel";
export {
  buildPixelViewModel,
  buildWorkHintByClientUuid,
  workHintFromTypeList,
} from "./pixelViewModel";
