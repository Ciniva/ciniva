export function pref(name, fallback) {
  try { return localStorage.getItem('ciniva.' + name) ?? fallback; } catch { return fallback; }
}

export function setPref(name, value) {
  try { localStorage.setItem('ciniva.' + name, value); } catch {}
}

export class AuthError extends Error {}

export async function api(path, opts = {}) {
  const headers = { ...(opts.headers || {}) };
  let body = opts.body;
  if (body !== undefined && typeof body !== 'string') {
    body = JSON.stringify(body);
    headers['Content-Type'] = 'application/json';
  }
  const r = await fetch('/api' + path, { ...opts, headers, body });
  if (r.status === 401) throw new AuthError('unauthorized');
  if (!r.ok) {
    let msg = r.statusText;
    try { msg = (await r.json()).error || msg; } catch {}
    throw new Error(msg);
  }
  if (r.status === 202 || r.status === 204) return null;
  return r.json();
}

export function url(path, params = {}) {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) if (v !== undefined && v !== null) q.set(k, v);
  const s = q.toString();
  return '/api' + path + (s ? '?' + s : '');
}

export function img(path, size = 'w342') {
  return path ? url(`/images/${size}/${path.replace(/^\//, '')}`) : '';
}
