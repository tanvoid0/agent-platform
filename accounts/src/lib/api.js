import { browser } from '$app/environment';

const API = '/accounts/api/v1';

export function tokens() {
  if (!browser) return { access: '', refresh: '' };
  return {
    access: localStorage.getItem('agp_access') || '',
    refresh: localStorage.getItem('agp_refresh') || ''
  };
}

export function setTokens(access, refresh) {
  if (access) localStorage.setItem('agp_access', access);
  if (refresh) localStorage.setItem('agp_refresh', refresh);
}

export function clearTokens() {
  localStorage.removeItem('agp_access');
  localStorage.removeItem('agp_refresh');
}

export async function api(path, opts = {}) {
  const headers = { 'content-type': 'application/json', ...(opts.headers || {}) };
  const { access } = tokens();
  if (access) headers.authorization = `Bearer ${access}`;
  const res = await fetch(API + path, { ...opts, headers });
  const text = await res.text();
  let body;
  try {
    body = JSON.parse(text);
  } catch {
    body = { raw: text };
  }
  if (!res.ok) {
    throw new Error(body?.error?.message || res.statusText);
  }
  return body;
}
