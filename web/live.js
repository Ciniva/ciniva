import { api, url, pref, setPref } from './api.js';
import { h, icon, toast } from './dom.js';

const probe = document.createElement('video');
const can = (t) => probe.canPlayType(t) !== '' || (() => { try { return !!window.MediaSource?.isTypeSupported(t); } catch { return false; } })();
const isApple = /Apple/.test(navigator.vendor);

function liveRequest() {
  return {
    max_height: 1080,
    bitrate_kbps: 6000,
    video_copy: ['h264', can('video/mp4; codecs="hvc1.1.6.L150.B0"') && 'hevc'].filter(Boolean).join(','),
    audio_copy: ['aac', can('audio/mp4; codecs="ac-3"') && 'ac3', can('audio/mp4; codecs="ec-3"') && 'eac3'].filter(Boolean).join(','),
    audio_channels: 2,
  };
}

/// Live channel player. `channels` is the list the channel was picked from, for channel up/down.
export function openLive({ plugin, stream, title, image, channels }) {
  const list = (channels || []).filter((c) => c.stream);
  let index = Math.max(0, list.findIndex((c) => c.stream === stream));
  let current = { stream, title, image };
  let hls = null;
  let closed = false;
  let attempt = 0;
  let generation = 0;

  const video = h('video', { playsinline: true, autoplay: true });
  video.volume = Number(pref('volume', '1'));
  video.muted = pref('muted', '0') === '1';
  const titleEl = h('div.player-title');
  const logo = h('img.live-logo', { alt: '', onerror: () => { logo.hidden = true; } });
  const message = h('div.player-message');
  const playBtn = h('button.ctl', { 'aria-label': 'Pause' }, icon('pause'));
  const muteBtn = h('button.ctl', { 'aria-label': 'Mute' }, icon(video.muted ? 'mute' : 'volume'));
  const volume = h('input.volume', { type: 'range', min: 0, max: 1, step: 0.05, value: video.muted ? 0 : video.volume, 'aria-label': 'Volume' });
  const prevBtn = h('button.ctl', { 'aria-label': 'Previous channel', title: 'Previous channel (↑)', disabled: list.length < 2 }, icon('prev'));
  const nextBtn = h('button.ctl', { 'aria-label': 'Next channel', title: 'Next channel (↓)', disabled: list.length < 2 }, icon('next'));
  const root = h('div.player.live.loading',
    video,
    h('div.player-top',
      h('button.ctl', { 'aria-label': 'Back', onclick: () => close() }, icon('back')),
      logo,
      h('div.player-titles', titleEl, h('div.player-subtitle', h('span.live-badge', 'LIVE'))),
    ),
    h('div.spinner'),
    message,
    h('div.player-bottom',
      h('div.controls',
        prevBtn, playBtn, nextBtn,
        h('div.spacer'),
        muteBtn, volume,
        h('button.ctl', { 'aria-label': 'Full screen', onclick: () => fullscreen() }, icon('fullscreen')),
      ),
    ),
  );

  function teardown() {
    hls?.destroy();
    hls = null;
    video.removeAttribute('src');
    video.load();
  }

  function fail(text) {
    root.classList.remove('loading');
    message.replaceChildren(h('p', text), h('button.btn', { onclick: () => start() }, 'Try again'));
  }

  async function start() {
    const mine = ++generation;
    message.replaceChildren();
    root.classList.add('loading');
    titleEl.textContent = current.title || '';
    logo.hidden = !current.image;
    if (current.image) logo.src = current.image;
    teardown();
    let r;
    try {
      r = await api(`/plugins/${plugin}/live/${encodeURIComponent(current.stream)}`, { method: 'POST', body: liveRequest() });
    } catch (e) {
      if (mine === generation) fail(e.message);
      return;
    }
    if (closed || mine !== generation) return;
    if (r.title) titleEl.textContent = r.title;
    const src = url(r.playlist.replace(/^\/api/, ''));
    const Hls = window.Hls;
    if (!(isApple && probe.canPlayType('application/vnd.apple.mpegurl')) && Hls?.isSupported()) {
      hls = new Hls({ liveSyncDurationCount: 3, manifestLoadingMaxRetry: 4, fragLoadingMaxRetry: 6 });
      hls.on(Hls.Events.ERROR, (_, d) => {
        if (!d.fatal || mine !== generation) return;
        if (d.type === Hls.ErrorTypes.MEDIA_ERROR) hls.recoverMediaError();
        else retry();
      });
      hls.loadSource(src);
      hls.attachMedia(video);
    } else if (probe.canPlayType('application/vnd.apple.mpegurl')) {
      video.src = src;
    } else {
      fail('This browser cannot play live streams.');
      return;
    }
    video.play().catch(() => {});
  }

  /// A dropped connection or a stopped session: start over a few times before giving up.
  function retry() {
    if (closed) return;
    if (++attempt > 5) return fail('The channel stopped sending video.');
    setTimeout(start, Math.min(8000, 1000 * attempt));
  }

  function zap(step) {
    if (list.length < 2) return;
    index = (index + step + list.length) % list.length;
    current = list[index];
    attempt = 0;
    start();
  }

  function toggle() {
    if (video.paused) video.play().catch(() => {}); else video.pause();
  }

  function fullscreen() {
    if (document.fullscreenElement) document.exitFullscreen();
    else if (root.requestFullscreen) root.requestFullscreen().catch(() => {});
    else video.webkitEnterFullscreen?.();
  }

  let hideTimer = null;
  function poke() {
    root.classList.remove('idle');
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => { if (!video.paused) root.classList.add('idle'); }, 3000);
  }

  function onKey(e) {
    if (e.target.closest?.('input, select, textarea')) return;
    const keys = {
      ' ': toggle, k: toggle, Escape: () => close(), f: fullscreen, m: () => { video.muted = !video.muted; },
      ArrowUp: () => zap(-1), ArrowDown: () => zap(1), PageUp: () => zap(-1), PageDown: () => zap(1),
    };
    const fn = keys[e.key];
    if (fn) { e.preventDefault(); fn(); poke(); }
  }

  function close({ fromHistory } = {}) {
    if (closed) return;
    closed = true;
    generation++;
    teardown();
    root.remove();
    document.body.classList.remove('playing');
    document.removeEventListener('keydown', onKey);
    window.removeEventListener('popstate', onPop);
    if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
    if (!fromHistory && history.state?.live) history.back();
  }

  const onPop = () => close({ fromHistory: true });

  video.addEventListener('playing', () => { root.classList.remove('loading'); attempt = 0; });
  video.addEventListener('waiting', () => root.classList.add('loading'));
  video.addEventListener('play', () => { root.classList.remove('paused'); playBtn.replaceChildren(icon('pause')); });
  video.addEventListener('pause', () => { root.classList.add('paused'); playBtn.replaceChildren(icon('play')); poke(); });
  video.addEventListener('error', () => { if (!hls) retry(); });
  video.addEventListener('click', toggle);
  video.addEventListener('volumechange', () => {
    muteBtn.replaceChildren(icon(video.muted || video.volume === 0 ? 'mute' : 'volume'));
    volume.value = video.muted ? 0 : video.volume;
    setPref('volume', video.volume);
    setPref('muted', video.muted ? '1' : '0');
  });
  volume.addEventListener('input', () => { video.volume = Number(volume.value); video.muted = video.volume === 0; });
  muteBtn.addEventListener('click', () => { video.muted = !video.muted; });
  playBtn.addEventListener('click', toggle);
  prevBtn.addEventListener('click', () => zap(-1));
  nextBtn.addEventListener('click', () => zap(1));
  root.addEventListener('pointermove', poke);
  document.addEventListener('keydown', onKey);
  window.addEventListener('popstate', onPop);

  document.body.append(root);
  document.body.classList.add('playing');
  history.pushState({ live: true }, '');
  poke();
  start().catch((e) => toast(e.message, 'error'));
}
