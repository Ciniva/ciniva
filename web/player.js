import { api, url, pref, setPref } from './api.js';
import { h, icon, clock, audioLabel, langName, toast } from './dom.js';

const QUALITIES = [
  { id: 'auto', label: 'Original' },
  { id: '2160', label: '4K · 40 Mbps', height: 2160, kbps: 40000 },
  { id: '1080', label: '1080p · 10 Mbps', height: 1080, kbps: 10000 },
  { id: '720', label: '720p · 4 Mbps', height: 720, kbps: 4000 },
  { id: '480', label: '480p · 1.5 Mbps', height: 480, kbps: 1500 },
];

const TEXT_SUBS = new Set(['subrip', 'srt', 'ass', 'ssa', 'mov_text', 'webvtt', 'text']);

const testVideo = document.createElement('video');
const mse = (t) => { try { return !!window.MediaSource?.isTypeSupported(t); } catch { return false; } };
const can = (t) => testVideo.canPlayType(t) !== '' || mse(t);
const isApple = /Apple/.test(navigator.vendor);
const caps = {
  h264: can('video/mp4; codecs="avc1.640033"'),
  hevc: can('video/mp4; codecs="hvc1.1.6.L150.B0"'),
  hevc10: can('video/mp4; codecs="hvc1.2.4.L153.B0"'),
  ac3: can('audio/mp4; codecs="ac-3"'),
  eac3: can('audio/mp4; codecs="ec-3"'),
  nativeHls: testVideo.canPlayType('application/vnd.apple.mpegurl') !== '',
};
const hdrDisplay = () => matchMedia('(dynamic-range: high)').matches;

function videoCodec(v) {
  const ten = (v.bit_depth || 8) > 8;
  switch (v.codec) {
    case 'h264': return ten ? null : 'avc1.640033';
    case 'hevc': return ten ? 'hvc1.2.4.L153.B0' : 'hvc1.1.6.L150.B0';
    case 'av1': return ten ? 'av01.0.12M.10' : 'av01.0.12M.08';
    case 'vp9': return ten ? 'vp09.02.51.10' : 'vp09.00.51.08';
    default: return null;
  }
}

function audioCodec(a) {
  return ({ aac: 'mp4a.40.2', mp3: 'mp4a.69', ac3: 'ac-3', eac3: 'ec-3', opus: 'opus', flac: 'flac' })[a.codec] || null;
}

function defaultAudio(probe) {
  return probe.audio.find((a) => a.default) || probe.audio[0] || null;
}

function directPlayable(file, audio) {
  const p = file.probe;
  if (!p?.video) return false;
  const ext = file.path.split('.').pop().toLowerCase();
  const mime = { mp4: 'video/mp4', m4v: 'video/mp4', mov: 'video/mp4', webm: 'video/webm' }[ext];
  if (!mime) return false;
  const v = p.video;
  if (v.dolby_vision_profile === 5 && !(isApple && can('video/mp4; codecs="dvh1.05.06"'))) return false;
  if (v.hdr && !hdrDisplay()) return false;
  if (audio && audio !== defaultAudio(p)) return false;
  const vc = videoCodec(v);
  const ac = audio ? audioCodec(audio) : null;
  if (!vc || (audio && !ac)) return false;
  return testVideo.canPlayType(`${mime}; codecs="${[vc, ac].filter(Boolean).join(',')}"`) !== '';
}

function transcodeRequest(file, quality, audio) {
  const v = file.probe.video || {};
  const srcHeight = v.height || 1080;
  const q = QUALITIES.find((x) => x.id === quality);
  const height = Math.min(q?.height || srcHeight, Math.max(srcHeight, 480));
  const kbps = q?.kbps || (srcHeight >= 1600 ? 40000 : srcHeight >= 1000 ? 12000 : 5000);
  const hdr = !!v.hdr && hdrDisplay() && caps.hevc10 && height >= 1080;
  return {
    max_height: height,
    bitrate_kbps: caps.hevc ? Math.round(kbps * 0.7) : kbps,
    codec: caps.hevc ? 'hevc' : 'h264',
    hdr,
    audio_stream: audio?.index ?? null,
    audio_channels: (audio?.channels || 2) >= 6 ? 6 : 2,
    audio_copy: ['aac', caps.ac3 && 'ac3', caps.eac3 && 'eac3'].filter(Boolean).join(','),
    video_copy: quality === 'auto' && caps.h264 ? 'h264' : undefined,
  };
}

export const SEEK_DEFAULTS = { short: 10, medium: 30, long: 60 };

export function seekSteps() {
  const read = (k) => Math.max(1, Number(pref(`seek.${k}`, SEEK_DEFAULTS[k])) || SEEK_DEFAULTS[k]);
  return { short: read('short'), medium: read('medium'), long: read('long') };
}

function seekStep(e) {
  const s = seekSteps();
  if ((e.metaKey || e.ctrlKey) && e.shiftKey) return s.long;
  if (e.shiftKey) return s.medium;
  return s.short;
}

export async function openPlayer({ fileId, title, subtitle, startAt, next, reuseHistory }) {
  const file = await api(`/files/${fileId}`);
  if (!file.probe) {
    toast('This file has not been analysed yet. Try again after the scan finishes.', 'error');
    return;
  }
  const probe = file.probe;
  const duration = file.duration || probe.duration || 0;
  let start = startAt ?? (!file.watched && file.position_ms ? file.position_ms / 1000 : 0);
  if (duration && start > duration - 15) start = 0;

  const state = { mode: null, hls: null, session: null, plan: null, forceTranscode: false, closed: false };
  const audioLang = pref('audioLang', '');
  let audio = probe.audio.find((a) => audioLang && a.language === audioLang) || defaultAudio(probe);
  let quality = pref('quality', 'auto');
  const subLang = pref('subLang', '');
  let subIndex = probe.subtitles.find((s) => TEXT_SUBS.has(s.codec) && (subLang ? s.language === subLang && !s.forced : s.forced))?.index ?? null;

  const video = h('video', { playsinline: true, preload: 'auto' });
  video.volume = Number(pref('volume', '1'));
  video.muted = pref('muted', '0') === '1';

  const played = h('div.seek-played');
  const buffered = h('div.seek-buffered');
  const knob = h('div.seek-knob');
  const trick = h('div.trick', h('div.trick-img'), h('div.trick-time'));
  const seek = h('div.seek', { role: 'slider', 'aria-label': 'Seek' }, h('div.seek-track', buffered, played), knob, trick);
  const time = h('span.time');
  const playBtn = h('button.ctl', { 'aria-label': 'Play' }, icon('play'));
  const muteBtn = h('button.ctl', { 'aria-label': 'Mute' }, icon('volume'));
  const volume = h('input.volume', { type: 'range', min: 0, max: 1, step: 0.05, value: video.volume, 'aria-label': 'Volume' });
  const spinner = h('div.spinner');
  const message = h('div.player-message');
  const info = h('div.stream-info');

  const audioSelect = h('select.ctl-select', { 'aria-label': 'Audio' },
    probe.audio.map((a) => h('option', { value: a.index, selected: a === audio },
      [langName(a.language), a.title, audioLabel(a), a.atmos ? 'Atmos' : ''].filter(Boolean).join(' · '))));
  const subSelect = h('select.ctl-select', { 'aria-label': 'Subtitles' },
    h('option', { value: '' }, 'Subtitles off'),
    probe.subtitles.map((s) => h('option', { value: s.index, disabled: !TEXT_SUBS.has(s.codec), selected: s.index === subIndex },
      [langName(s.language) || 'Unknown', s.title, s.forced ? 'Forced' : '', TEXT_SUBS.has(s.codec) ? '' : `${s.codec.toUpperCase()} (image, not supported)`].filter(Boolean).join(' · '))));
  const qualitySelect = h('select.ctl-select', { 'aria-label': 'Quality' },
    QUALITIES.map((q) => h('option', { value: q.id, selected: q.id === quality }, q.label)));

  const upNext = h('div.upnext');
  const root = h('div.player.paused',
    video,
    h('div.player-top',
      h('button.ctl', { 'aria-label': 'Back', onclick: () => close() }, icon('back')),
      h('div.player-titles', h('div.player-title', title || ''), subtitle ? h('div.player-subtitle', subtitle) : null),
      info,
    ),
    spinner,
    message,
    upNext,
    h('div.player-bottom',
      seek,
      h('div.controls',
        playBtn,
        time,
        h('div.spacer'),
        probe.audio.length > 1 ? audioSelect : null,
        probe.subtitles.length ? subSelect : null,
        qualitySelect,
        muteBtn,
        volume,
        h('button.ctl', { 'aria-label': 'Full screen', onclick: () => toggleFullscreen() }, icon('fullscreen')),
      ),
    ),
  );
  document.body.append(root);
  document.body.classList.add('playing');
  if (reuseHistory) history.replaceState({ player: true }, ''); else history.pushState({ player: true }, '');

  const total = () => (isFinite(video.duration) && video.duration > 0 ? video.duration : duration);

  function showInfo() {
    const parts = [];
    if (state.mode === 'direct') parts.push('Direct play');
    else if (state.plan?.remux) parts.push('Direct stream · video untouched');
    else if (state.plan) {
      const p = state.plan;
      parts.push(`Transcode · ${p.codec.toUpperCase()} on ${p.device}`);
      if (p.tonemap) parts.push(`tone map ${p.tonemap}`);
      if (p.hdr_out) parts.push('HDR');
    }
    info.textContent = parts.join(' · ');
  }

  async function teardown() {
    state.hls?.destroy();
    state.hls = null;
    if (state.session) {
      const sid = state.session;
      state.session = null;
      api(`/transcode/${sid}`, { method: 'DELETE', keepalive: true }).catch(() => {});
    }
    video.removeAttribute('src');
    video.load();
  }

  async function load(position) {
    message.textContent = '';
    root.classList.add('loading');
    await teardown();
    if (state.closed) return;
    const direct = !state.forceTranscode && quality === 'auto' && directPlayable(file, audio);
    if (direct) {
      state.mode = 'direct';
      state.plan = null;
      video.src = url(`/files/${fileId}/stream`);
      video.addEventListener('loadedmetadata', () => { if (position) video.currentTime = position; }, { once: true });
    } else {
      state.mode = 'hls';
      let r;
      try {
        r = await api(`/files/${fileId}/transcode`, { method: 'POST', body: transcodeRequest(file, quality, audio) });
      } catch (e) {
        fail(`Could not start transcoding: ${e.message}`);
        return;
      }
      if (state.closed) {
        api(`/transcode/${r.session}`, { method: 'DELETE' }).catch(() => {});
        return;
      }
      state.session = r.session;
      state.plan = r.plan;
      const src = url(r.playlist.replace(/^\/api/, ''));
      const Hls = window.Hls;
      const useNative = caps.nativeHls && (isApple || !Hls?.isSupported());
      if (!useNative && Hls?.isSupported()) {
        const hls = new Hls({ startPosition: position || 0, maxBufferLength: 30, backBufferLength: 60, fragLoadingMaxRetry: 6 });
        state.hls = hls;
        hls.on(Hls.Events.ERROR, (_, d) => {
          if (!d.fatal) return;
          if (d.type === Hls.ErrorTypes.NETWORK_ERROR) hls.startLoad();
          else if (d.type === Hls.ErrorTypes.MEDIA_ERROR) hls.recoverMediaError();
          else fail(`Playback error: ${d.details}`);
        });
        hls.loadSource(src);
        hls.attachMedia(video);
      } else if (caps.nativeHls) {
        video.src = src;
        video.addEventListener('loadedmetadata', () => { if (position) video.currentTime = position; }, { once: true });
      } else {
        fail('This browser cannot play HLS streams.');
        return;
      }
    }
    showInfo();
    applySubtitle();
    video.play().catch(() => root.classList.add('paused'));
  }

  function fail(text) {
    root.classList.remove('loading');
    message.textContent = text;
  }

  function applySubtitle() {
    video.querySelectorAll('track').forEach((t) => t.remove());
    if (subIndex === null) return;
    const s = probe.subtitles.find((x) => x.index === subIndex);
    const track = h('track', { kind: 'subtitles', src: url(`/files/${fileId}/subtitles/${subIndex}.vtt`), srclang: s?.language || 'und', label: s?.title || langName(s?.language) || 'Subtitles', default: true });
    track.addEventListener('error', () => toast('Could not load subtitles', 'error'));
    const slow = setTimeout(() => toast('Extracting subtitles from the file, this can take a minute the first time…'), 1500);
    track.addEventListener('load', () => clearTimeout(slow));
    track.addEventListener('error', () => clearTimeout(slow));
    video.append(track);
    track.track.mode = 'showing';
  }

  let lastReport = 0;
  function report(force) {
    const t = video.currentTime;
    if (!total() || t < 1 || state.closed && !force) return;
    if (!force && Date.now() - lastReport < 10000) return;
    lastReport = Date.now();
    api(`/files/${fileId}/progress`, { method: 'PUT', keepalive: true, body: { position_ms: Math.floor(t * 1000), duration_ms: Math.floor(total() * 1000) } }).catch(() => {});
  }

  function render() {
    const d = total();
    const t = video.currentTime;
    played.style.width = d ? `${(t / d) * 100}%` : '0';
    knob.style.left = d ? `${(t / d) * 100}%` : '0';
    let end = 0;
    for (let i = 0; i < video.buffered.length; i++) if (video.buffered.start(i) <= t + 1) end = Math.max(end, video.buffered.end(i));
    buffered.style.width = d ? `${(end / d) * 100}%` : '0';
    time.textContent = `${clock(t)} / ${clock(d)}`;
  }

  function seekTo(t) {
    const d = total();
    video.currentTime = Math.max(0, Math.min(d ? d - 0.5 : t, t));
    render();
    poke();
  }

  let trickplay = null;
  api(`/files/${fileId}/trickplay`).then((m) => { trickplay = m; }).catch(() => {});

  function preview(fraction) {
    const d = total();
    const t = fraction * d;
    trick.style.left = `${fraction * 100}%`;
    trick.querySelector('.trick-time').textContent = clock(t);
    const image = trick.querySelector('.trick-img');
    if (trickplay) {
      const perSheet = trickplay.cols * trickplay.rows;
      const i = Math.min(Math.floor(t / trickplay.interval), trickplay.count - 1);
      const sheet = Math.floor(i / perSheet) + 1;
      const within = i % perSheet;
      image.style.display = '';
      image.style.width = `${trickplay.width}px`;
      image.style.height = `${trickplay.height}px`;
      image.style.backgroundImage = `url("${url(`/files/${fileId}/trickplay/${sheet}.jpg`)}")`;
      image.style.backgroundPosition = `-${(within % trickplay.cols) * trickplay.width}px -${Math.floor(within / trickplay.cols) * trickplay.height}px`;
    } else {
      image.style.display = 'none';
    }
  }

  const fractionAt = (e) => {
    const r = seek.getBoundingClientRect();
    return Math.max(0, Math.min(1, (e.clientX - r.left) / r.width));
  };
  let dragging = false;
  seek.addEventListener('pointermove', (e) => {
    preview(fractionAt(e));
    if (dragging) {
      played.style.width = knob.style.left = `${fractionAt(e) * 100}%`;
    }
  });
  seek.addEventListener('pointerdown', (e) => {
    dragging = true;
    seek.setPointerCapture(e.pointerId);
    seek.classList.add('dragging');
    preview(fractionAt(e));
  });
  seek.addEventListener('pointerup', (e) => {
    if (!dragging) return;
    dragging = false;
    seek.classList.remove('dragging');
    seekTo(fractionAt(e) * total());
  });

  let hideTimer;
  function poke() {
    root.classList.remove('idle');
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => { if (!video.paused && !dragging) root.classList.add('idle'); }, 3000);
  }
  root.addEventListener('pointermove', poke);

  function toggle() {
    if (video.paused) video.play().catch(() => {}); else video.pause();
  }

  function toggleFullscreen() {
    if (document.fullscreenElement) document.exitFullscreen();
    else if (root.requestFullscreen) root.requestFullscreen().catch(() => {});
    else video.webkitEnterFullscreen?.();
  }

  video.addEventListener('click', toggle);
  video.addEventListener('dblclick', toggleFullscreen);
  playBtn.addEventListener('click', toggle);
  video.addEventListener('play', () => { root.classList.remove('paused'); playBtn.replaceChildren(icon('pause')); poke(); });
  video.addEventListener('pause', () => { root.classList.add('paused'); playBtn.replaceChildren(icon('play')); poke(); report(true); });
  video.addEventListener('waiting', () => root.classList.add('loading'));
  video.addEventListener('seeking', () => root.classList.add('loading'));
  for (const ev of ['playing', 'seeked', 'canplay']) video.addEventListener(ev, () => root.classList.remove('loading'));
  video.addEventListener('timeupdate', () => { render(); report(false); });
  video.addEventListener('progress', render);
  video.addEventListener('durationchange', render);
  video.addEventListener('volumechange', () => {
    muteBtn.replaceChildren(icon(video.muted || video.volume === 0 ? 'mute' : 'volume'));
    volume.value = video.muted ? 0 : video.volume;
    setPref('volume', video.volume);
    setPref('muted', video.muted ? '1' : '0');
  });
  video.addEventListener('error', () => {
    if (state.mode === 'direct') {
      state.forceTranscode = true;
      load(video.currentTime || start);
    } else if (!state.hls) {
      fail('Playback failed.');
    }
  });
  video.addEventListener('ended', () => {
    api(`/files/${fileId}/progress`, { method: 'PUT', body: { position_ms: Math.floor(total() * 1000), duration_ms: Math.floor(total() * 1000) } }).catch(() => {});
    showUpNext();
  });
  muteBtn.addEventListener('click', () => { video.muted = !video.muted; });
  volume.addEventListener('input', () => { video.volume = Number(volume.value); video.muted = video.volume === 0; });
  audioSelect.addEventListener('change', () => {
    audio = probe.audio.find((a) => a.index === Number(audioSelect.value)) || audio;
    setPref('audioLang', audio.language || '');
    load(video.currentTime);
  });
  subSelect.addEventListener('change', () => {
    subIndex = subSelect.value === '' ? null : Number(subSelect.value);
    setPref('subLang', subIndex === null ? '' : probe.subtitles.find((s) => s.index === subIndex)?.language || '');
    applySubtitle();
  });
  qualitySelect.addEventListener('change', () => {
    quality = qualitySelect.value;
    setPref('quality', quality);
    state.forceTranscode = false;
    load(video.currentTime);
  });

  async function showUpNext() {
    if (!next) return;
    const n = await next().catch(() => null);
    if (!n || state.closed) return;
    let left = 10;
    const counter = h('span', String(left));
    const go = () => { clearInterval(timer); close({ keepHistory: true }).then(() => openPlayer({ ...n, startAt: 0, reuseHistory: true })); };
    const cancel = () => { clearInterval(timer); upNext.classList.remove('show'); };
    upNext.replaceChildren(
      h('div.upnext-label', 'Up next'),
      h('div.upnext-title', n.subtitle || n.title),
      h('div.upnext-actions',
        h('button.btn.primary', { onclick: go }, icon('play'), ' Play in ', counter),
        h('button.btn', { onclick: cancel }, 'Cancel')),
    );
    upNext.classList.add('show');
    const timer = setInterval(() => {
      left -= 1;
      counter.textContent = String(left);
      if (left <= 0) go();
    }, 1000);
  }

  function onKey(e) {
    if (e.target instanceof HTMLSelectElement || e.target instanceof HTMLInputElement) return;
    const step = seekStep(e);
    switch (e.key) {
      case ' ': case 'k': toggle(); break;
      case 'ArrowLeft': seekTo(video.currentTime - step); break;
      case 'ArrowRight': seekTo(video.currentTime + step); break;
      case 'ArrowUp': video.volume = Math.min(1, video.volume + 0.05); video.muted = false; break;
      case 'ArrowDown': video.volume = Math.max(0, video.volume - 0.05); break;
      case 'f': toggleFullscreen(); break;
      case 'm': video.muted = !video.muted; break;
      case 'Escape': if (!document.fullscreenElement) close(); break;
      default: return;
    }
    e.preventDefault();
    poke();
  }

  function onPop() { close({ fromHistory: true }); }
  function onUnload() { report(true); if (state.session) api(`/transcode/${state.session}`, { method: 'DELETE', keepalive: true }).catch(() => {}); }

  async function close({ fromHistory, keepHistory } = {}) {
    if (state.closed) return;
    report(true);
    state.closed = true;
    document.removeEventListener('keydown', onKey);
    window.removeEventListener('popstate', onPop);
    window.removeEventListener('pagehide', onUnload);
    clearTimeout(hideTimer);
    if (document.fullscreenElement) await document.exitFullscreen().catch(() => {});
    await teardown();
    root.remove();
    document.body.classList.remove('playing');
    if (!fromHistory && !keepHistory) history.back();
    window.dispatchEvent(new CustomEvent('ciniva:player-closed'));
  }

  document.addEventListener('keydown', onKey);
  window.addEventListener('popstate', onPop);
  window.addEventListener('pagehide', onUnload);
  poke();
  load(start);
}
