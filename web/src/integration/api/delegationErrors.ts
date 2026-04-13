/**
 * Turn fetch error bodies (JSON, HTML, plain text) into a short message for the UI.
 */
export function formatDelegationErrorMessage(status: number, bodyText: string): string {
  const raw = (bodyText ?? '').trim();
  try {
    const j = JSON.parse(raw) as { error?: unknown; message?: unknown };
    if (typeof j.error === 'string' && j.error.trim()) {
      return augmentWithHints(status, j.error.trim());
    }
    if (typeof j.message === 'string' && j.message.trim()) {
      return augmentWithHints(status, j.message.trim());
    }
  } catch {
    /* not JSON */
  }

  if (raw.length > 0 && raw.length < 800 && !raw.startsWith('<')) {
    return augmentWithHints(status, raw);
  }

  const pre = raw.match(/<pre[^>]*>([\s\S]*?)<\/pre>/i);
  if (pre?.[1]) {
    return augmentWithHints(status, pre[1].trim());
  }

  return augmentWithHints(status, '');
}

function augmentWithHints(status: number, message: string): string {
  const generic =
    !message ||
    /^internal server error$/i.test(message) ||
    /^bad gateway$/i.test(message) ||
    /^service unavailable$/i.test(message);

  if (status === 502 || status === 503 || status === 504) {
    const head =
      message && !generic
        ? message
        : 'The dev proxy could not reach the delegation backend (connection refused or timeout).';
    return `${head}\n\nFix: in another terminal run:\n  cd server && node index.mjs\nDefault URL is http://127.0.0.1:3847 — Vite (port 3000) proxies /api there (vite.config.ts).\nThen reload this page.`;
  }

  if (status >= 500 && generic) {
    return `Server error (${status}).\n\nIf nothing appears in the delegation server terminal, the request may not be reaching it (start server on port 3847).\nIf the server is running, check MongoDB (MONGODB_URI in server/.env) and the server log for a stack trace.\nOpen /api/health in the browser to verify MongoDB.`;
  }

  if (status >= 500 && message) {
    return `${message}\n\nIf this persists, check the terminal running server/index.mjs and MongoDB.`;
  }

  return message || `Request failed (HTTP ${status})`;
}

export async function readDelegationError(response: Response): Promise<string> {
  const text = await response.text();
  return formatDelegationErrorMessage(response.status, text);
}
