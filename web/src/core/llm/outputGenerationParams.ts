/**
 * Params for final cloud asset generation (output-review modal, `pendingOutputParams`, and `processFinalAsset`).
 * Extend this interface when new modality controls are added — avoids ad-hoc object shapes across store and UI.
 */
export interface OutputGenerationParams {
  model?: string;
  aspectRatio?: string;
  imageSize?: string;
  resolution?: string;
  durationSeconds?: number;
}
