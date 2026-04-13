/**
 * Gemini credentials must come from Vite env at build time — never from localStorage / BYOK UI.
 */
export function getGeminiApiKeyFromEnv(): string {
  const k = import.meta.env.VITE_GEMINI_API_KEY;
  return typeof k === 'string' ? k.trim() : '';
}
