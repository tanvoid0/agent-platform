/** Throttle for writing live sim poses into the project session (core store → persistence). */
export const PROJECT_SESSION_SCENE_CAPTURE_MS = 850;

/** Bump when the on-disk / API shape of `PersistedProjectPayload.session` changes. */
export const PROJECT_SESSION_FORMAT_VERSION = 1 as const;
