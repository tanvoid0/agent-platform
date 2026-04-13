/** 1×1 PNG — valid file for “mock image” without calling Gemini. */
export const MOCK_DELIVERABLE_IMAGE_BASE64 =
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';

/**
 * Stored in `finalAssetContent` when the user chose a mock audio/video deliverable without cloud generation.
 * The results UI renders a clear placeholder instead of a broken player.
 */
export const MOCK_MEDIA_DELIVERABLE_SENTINEL = '__THE_DELEGATION_MOCK_MEDIA__';

export function isMockMediaSentinel(content: string | null | undefined): boolean {
  return content === MOCK_MEDIA_DELIVERABLE_SENTINEL;
}
