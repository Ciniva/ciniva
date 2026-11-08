import { img } from './api.js';

const PROPS = new Set(['value', 'checked', 'disabled', 'selected', 'muted', 'volume']);

export function h(tag, attrs, ...kids) {
  const [name, ...classes] = tag.split('.');
  const el = document.createElement(name || 'div');
  if (classes.length) el.className = classes.join(' ');
  if (attrs === null || typeof attrs !== 'object' || attrs instanceof Node || Array.isArray(attrs)) {
    kids.unshift(attrs);
  } else {
    for (const [k, v] of Object.entries(attrs)) {
      if (v === undefined || v === null || v === false) continue;
      if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
      else if (k === 'class') el.className = [el.className, v].filter(Boolean).join(' ');
      else if (k === 'style' && typeof v === 'object') Object.assign(el.style, v);
      else if (PROPS.has(k)) el[k] = v;
      else el.setAttribute(k, v === true ? '' : v);
    }
  }
  append(el, kids);
  return el;
}

export function append(el, ...kids) {
  for (const k of kids.flat(Infinity)) {
    if (k === null || k === undefined || k === false || k === '') continue;
    el.append(k instanceof Node ? k : String(k));
  }
  return el;
}

export function clear(el) {
  el.replaceChildren();
  return el;
}

export function icon(name) {
  const paths = {
    play: 'M8 5v14l11-7z',
    pause: 'M7 5h4v14H7zM13 5h4v14h-4z',
    back: 'M15.5 5l-7 7 7 7',
    search: 'M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14zM20 20l-4-4',
    check: 'M5 12l5 5 9-10',
    gear: 'M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z',
    fullscreen: 'M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5',
    volume: 'M4 9h4l5-4v14l-5-4H4zM16 9a4 4 0 0 1 0 6M18.5 6.5a8 8 0 0 1 0 11',
    mute: 'M4 9h4l5-4v14l-5-4H4zM17 9l5 6M22 9l-5 6',
    cc: 'M4 6h16v12H4zM10 10.5a2 2 0 1 0 0 3M16 10.5a2 2 0 1 0 0 3',
    wrench: 'M14.7 6.3a4 4 0 0 0-5.4 5.1L4 16.7 7.3 20l5.3-5.3a4 4 0 0 0 5.1-5.4l-2.5 2.5-2.4-.6-.6-2.4z',
    x: 'M6 6l12 12M18 6L6 18',
    next: 'M6 5v14l9-7zM17 5h2v14h-2z',
    refresh: 'M20 11a8 8 0 1 0-2.3 5.7M20 4v7h-7',
    credits: 'M4 5h16v14H4zM8 9h8M8 12h8M8 15h5',
  };
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('class', 'icon');
  svg.setAttribute('aria-hidden', 'true');
  const p = document.createElementNS('http://www.w3.org/2000/svg', 'path');
  p.setAttribute('d', paths[name] || '');
  if (name === 'play' || name === 'pause' || name === 'next') p.setAttribute('class', 'fill');
  svg.append(p);
  return svg;
}

export function duration(secs) {
  if (!secs || !isFinite(secs)) return '';
  const m = Math.round(secs / 60);
  return m >= 60 ? `${Math.floor(m / 60)}h ${m % 60}m` : `${m}m`;
}

export function clock(secs) {
  if (!isFinite(secs) || secs < 0) secs = 0;
  const s = Math.floor(secs % 60), m = Math.floor(secs / 60) % 60, hr = Math.floor(secs / 3600);
  const pad = (n) => String(n).padStart(2, '0');
  return hr ? `${hr}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

export function bytes(n) {
  if (!n) return '';
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return `${n.toFixed(i >= 3 ? 1 : 0)} ${u[i]}`;
}

function discSplit(path) {
  const parts = path.split('/');
  const i = parts.findIndex((p) => p.toUpperCase() === 'BDMV');
  return i > 0 ? { dir: parts.slice(0, i - 1).join('/'), name: `${parts[i - 1]} (${parts[parts.length - 1]})` } : null;
}

export function fileName(path) {
  return discSplit(path)?.name ?? path.split('/').pop();
}

export function dirName(path) {
  return discSplit(path)?.dir ?? path.split('/').slice(0, -1).join('/');
}

export function progressBar(fraction) {
  if (!fraction || fraction <= 0) return null;
  return h('div.progress', h('div', { style: { width: `${Math.min(100, fraction * 100)}%` } }));
}

export function poster(path, alt, size = 'w342') {
  const box = h('div.poster');
  if (path) {
    const image = h('img', { src: img(path, size), alt: alt || '', loading: 'lazy', decoding: 'async' });
    image.addEventListener('error', () => image.remove());
    box.append(image);
  }
  box.append(h('span.poster-fallback', alt || ''));
  return box;
}

export function movieCard(m) {
  const frac = m.duration_ms ? m.position_ms / m.duration_ms : 0;
  return h('a.card', { href: `#/movie/${m.id}`, title: m.title },
    h('div.card-art', poster(m.poster_path, m.title), m.watched ? h('span.badge-watched', icon('check')) : null, progressBar(frac)),
    h('div.card-title', m.title),
    h('div.card-sub', m.year || ''),
  );
}

export function showCard(s) {
  const left = s.episodes - s.watched_episodes;
  return h('a.card', { href: `#/show/${s.id}`, title: s.title },
    h('div.card-art', poster(s.poster_path, s.title),
      left === 0 && s.episodes > 0 ? h('span.badge-watched', icon('check')) : h('span.badge-count', left)),
    h('div.card-title', s.title),
    h('div.card-sub', [s.year, `${s.episodes} episode${s.episodes === 1 ? '' : 's'}`].filter(Boolean).join(' · ')),
  );
}

export function row(title, items, href) {
  if (!items.length) return null;
  return h('section.row',
    h('div.row-head', h('h2', title), href ? h('a.more', { href }, 'See all') : null),
    h('div.row-scroll', items),
  );
}

export function techBadges(probe) {
  if (!probe) return [];
  const out = [];
  const v = probe.video;
  if (v) {
    const res = v.height >= 1600 || v.width >= 3200 ? '4K' : v.height >= 1000 || v.width >= 1800 ? '1080p' : v.height >= 700 ? '720p' : `${v.height}p`;
    out.push(res);
    if (v.dolby_vision_profile) out.push(`Dolby Vision P${v.dolby_vision_profile}`);
    if (v.hdr && v.hdr !== 'dolby_vision') out.push(v.hdr === 'hdr10' ? 'HDR10' : v.hdr === 'hdr10+' ? 'HDR10+' : v.hdr.toUpperCase());
    out.push(({ hevc: 'HEVC', h264: 'H.264', av1: 'AV1', vp9: 'VP9', mpeg2video: 'MPEG-2', mpeg4: 'MPEG-4' })[v.codec] || v.codec.toUpperCase());
  }
  const a = probe.audio?.find((x) => x.default) || probe.audio?.[0];
  if (a) {
    if (a.atmos) out.push('Atmos');
    out.push(audioLabel(a));
  }
  return out;
}

export function audioLabel(a) {
  const names = { truehd: 'TrueHD', eac3: 'DD+', ac3: 'Dolby Digital', dts: 'DTS', aac: 'AAC', flac: 'FLAC', opus: 'Opus', mp3: 'MP3', pcm_s16le: 'PCM', pcm_s24le: 'PCM' };
  let name = names[a.codec] || a.codec.toUpperCase();
  if (a.codec === 'dts' && a.profile?.includes('MA')) name = 'DTS-HD MA';
  const ch = a.channels ? ({ 1: '1.0', 2: '2.0', 6: '5.1', 8: '7.1' })[a.channels] || `${a.channels}ch` : '';
  return [name, ch].filter(Boolean).join(' ');
}

export function langName(code) {
  if (!code || code === 'und') return '';
  try { return new Intl.DisplayNames([navigator.language], { type: 'language' }).of(code.length === 3 ? iso3(code) : code) || code; } catch { return code; }
}

function iso3(c) {
  const map = { eng: 'en', rus: 'ru', jpn: 'ja', kor: 'ko', geo: 'ka', kat: 'ka', fre: 'fr', fra: 'fr', ger: 'de', deu: 'de', spa: 'es', ita: 'it', por: 'pt', chi: 'zh', zho: 'zh', ukr: 'uk', pol: 'pl', tur: 'tr', ara: 'ar', hin: 'hi', dut: 'nl', nld: 'nl', swe: 'sv', nor: 'no', dan: 'da', fin: 'fi', cze: 'cs', ces: 'cs', hun: 'hu', gre: 'el', ell: 'el', heb: 'he', tha: 'th', vie: 'vi', ind: 'id', may: 'ms', msa: 'ms', rum: 'ro', ron: 'ro', bul: 'bg', hrv: 'hr', srp: 'sr', slv: 'sl', slo: 'sk', slk: 'sk', est: 'et', lav: 'lv', lit: 'lt', per: 'fa', fas: 'fa', arm: 'hy', hye: 'hy' };
  return map[c] || c;
}

export function toast(message, kind = 'info') {
  let host = document.querySelector('.toasts');
  if (!host) document.body.append(host = h('div.toasts'));
  const t = h(`div.toast.${kind}`, message);
  host.append(t);
  setTimeout(() => t.classList.add('out'), 3500);
  setTimeout(() => t.remove(), 4000);
}

export function modal(content, { onclose, wide } = {}) {
  const dialog = h('dialog.modal' + (wide ? '.wide' : ''));
  const close = () => { dialog.close(); };
  dialog.addEventListener('close', () => { dialog.remove(); onclose?.(); });
  dialog.addEventListener('click', (e) => { if (e.target === dialog) close(); });
  dialog.append(h('button.modal-close', { onclick: close, 'aria-label': 'Close' }, icon('x')), content);
  document.body.append(dialog);
  dialog.showModal();
  return { dialog, close };
}
