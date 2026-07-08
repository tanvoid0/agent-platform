/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** When set, prepended to API paths (e.g. `http://127.0.0.1:18410`). Unset in dev defaults to that URL. */
  readonly VITE_API_ORIGIN?: string;
  /** Must match server `AGENT_PLATFORM_MASTER_KEY` when API auth is enabled. */
  readonly VITE_AGENT_PLATFORM_MASTER_KEY?: string;
  /** Optional default `model` for POST /api/v1/chat (proxy alias). */
  readonly VITE_DEFAULT_CHAT_MODEL?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
