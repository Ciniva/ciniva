import { api, img, token, setToken, pref, setPref, AuthError } from './api.js';
import { h, append, clear, icon, duration, bytes, fileName, progressBar, poster, movieCard, showCard, row, techBadges, toast, clock, modal } from './dom.js';
import { openPlayer, seekSteps, SEEK_DEFAULTS } from './player.js';
import { fixView, matchDialog } from './fix.js';

const app = document.getElementById('app');
const main = h('main');
const search = h('input.search', { type: 'search', placeholder: 'Search', 'aria-label': 'Search library' });
const fixBadge = h('span.nav-badge');
const nav = h('header.nav',
  h('a.brand', { href: '#/' }, h('img', { src: 'icon.svg', alt: '' }), 'Ciniva'),
  h('nav.links',
    h('a', { href: '#/', 'data-route': 'home' }, 'Home'),
    h('a', { href: '#/movies', 'data-route': 'movies' }, 'Movies'),
    h('a', { href: '#/shows', 'data-route': 'shows' }, 'Shows'),
  ),
  h('div.nav-right',
    h('label.search-box', icon('search'), search),
    h('a.nav-icon', { href: '#/fix', title: 'Fix matches', 'data-route': 'fix' }, icon('wrench'), fixBadge),
    h('a.nav-icon', { href: '#/settings', title: 'Settings', 'data-route': 'settings' }, icon('gear')),
  ),
);

let searchTimer;
search.addEventListener('input', () => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => {
    const q = search.value.trim();
    const target = q ? `#/search?q=${encodeURIComponent(q)}` : '#/';
    if (location.hash.startsWith('#/search')) location.replace(target); else location.hash = target;
  }, 250);
});

const routes = [
  [/^\/$/, 'home', homeView],
  [/^\/movies$/, 'movies', (el, m, q) => libraryView(el, 'movies', q)],
  [/^\/shows$/, 'shows', (el, m, q) => libraryView(el, 'shows', q)],
  [/^\/movie\/(\d+)$/, 'movies', (el, m) => movieView(el, Number(m[1]))],
  [/^\/show\/(\d+)$/, 'shows', (el, m, q) => showView(el, Number(m[1]), q)],
  [/^\/search$/, 'search', searchView],
  [/^\/fix$/, 'fix', (el) => fixView(el, refreshStatus)],
  [/^\/settings$/, 'settings', settingsView],
];

let seq = 0;
async function route() {
  const raw = location.hash.slice(1) || '/';
  const [path, qs] = raw.split('?');
  const params = new URLSearchParams(qs || '');
  const found = routes.find(([re]) => re.test(path)) || routes[0];
  const match = path.match(found[0]) || [];
  const mine = ++seq;
  for (const a of nav.querySelectorAll('[data-route]')) a.classList.toggle('active', a.dataset.route === found[1]);
  if (found[1] !== 'search') search.value = '';
  const view = h('div.view');
  main.classList.add('busy');
  try {
    await found[2](view, match, params);
    if (mine !== seq) return;
    main.replaceChildren(view);
    window.scrollTo(0, 0);
  } catch (e) {
    if (mine !== seq) return;
    if (e instanceof AuthError) return login();
    main.replaceChildren(h('div.view', h('div.empty', h('h2', 'Something went wrong'), h('p', e.message))));
  } finally {
    if (mine === seq) main.classList.remove('busy');
  }
}

async function rerender() {
  const y = window.scrollY;
  await route();
  window.scrollTo(0, y);
}

function login(error) {
  app.replaceChildren(h('div.login',
    h('form.login-card', {
      onsubmit: async (e) => {
        e.preventDefault();
        setToken(e.target.token.value.trim());
        try {
          await api('/status');
          start();
        } catch (err) {
          login(err instanceof AuthError ? 'That token was not accepted.' : err.message);
        }
      },
    },
      h('img.login-logo', { src: 'icon.svg', alt: '' }),
      h('h1', 'Ciniva'),
      h('p', 'Enter the access token from the server’s CINIVA_TOKEN setting.'),
      h('input', { name: 'token', type: 'password', placeholder: 'Access token', autocomplete: 'current-password', required: true, autofocus: true }),
      error ? h('p.error', error) : null,
      h('button.btn.primary', { type: 'submit' }, 'Sign in'),
    ),
  ));
}

let statusTimer;
let loadedVersion = null;
async function refreshStatus() {
  try {
    const s = await api('/status');
    // An open tab keeps running the old app after an upgrade; reload once nothing is playing.
    loadedVersion ??= s.version;
    if (s.version !== loadedVersion && !document.querySelector('.player')) location.reload();
    const n = s.counts.unmatched + s.counts.pending;
    fixBadge.textContent = n ? String(n) : '';
    clearTimeout(statusTimer);
    statusTimer = setTimeout(refreshStatus, s.scan.running ? 3000 : 60000);
    return s;
  } catch {
    return null;
  }
}

async function start() {
  try {
    await api('/status');
  } catch (e) {
    if (e instanceof AuthError) return login(token() ? 'That token was not accepted.' : null);
  }
  app.replaceChildren(nav, main);
  refreshStatus();
  route();
}

window.addEventListener('hashchange', route);
window.addEventListener('ciniva:player-closed', rerender);

function hero(backdrop, ...content) {
  return h('div.hero',
    backdrop ? h('img.hero-img', { src: img(backdrop, 'w1280'), alt: '', decoding: 'async' }) : null,
    h('div.hero-shade'),
    h('div.hero-content', content),
  );
}

async function homeView(el) {
  const home = await api('/home');
  const featured = [...home.recent_movies, ...home.recent_shows].filter((x) => x.backdrop_path);
  const pick = featured[Math.floor(Math.random() * Math.min(featured.length, 8))];
  if (pick) {
    const isMovie = 'added_at' in pick;
    append(el, hero(pick.backdrop_path,
      h('div.eyebrow', 'Recently added'),
      h('h1', pick.title),
      h('div.meta', [pick.year, pick.rating ? `★ ${pick.rating.toFixed(1)}` : null].filter(Boolean).join(' · ')),
      h('div.actions', h('a.btn.primary', { href: isMovie ? `#/movie/${pick.id}` : `#/show/${pick.id}` }, 'View details')),
    ));
  }
  append(el, 
    row('Continue watching', home.continue_watching.map(continueCard)),
    row('Recently added movies', home.recent_movies.map(movieCard), '#/movies?sort=added'),
    row('Recently added shows', home.recent_shows.map(showCard), '#/shows?sort=added'),
  );
  if (!home.recent_movies.length && !home.recent_shows.length) {
    append(el, h('div.empty', h('h2', 'Your library is empty'), h('p', 'Add files to the media folder and run a scan from ', h('a', { href: '#/settings' }, 'Settings'), '.')));
  }
}

function continueCard(c) {
  const play = () => openPlayer({
    fileId: c.file_id,
    title: c.title,
    subtitle: c.subtitle,
    showId: c.show_id || undefined,
  });
  return h('button.wide-card', { onclick: play, title: c.title },
    h('div.wide-art', poster(c.image_path, c.title, 'w780'), h('span.play-overlay', icon('play')), progressBar(c.position_ms / c.duration_ms)),
    h('div.card-title', c.title),
    h('div.card-sub', c.subtitle || `${clock((c.duration_ms - c.position_ms) / 1000)} left`),
  );
}

const SORTS = [['title', 'Title'], ['added', 'Recently added'], ['year', 'Year'], ['rating', 'Rating']];

async function libraryView(el, kind, params) {
  const sort = params.get('sort') || pref(`sort.${kind}`, 'title');
  const [items, home] = await Promise.all([api(`/${kind}?sort=${sort}`), api('/home')]);
  const resume = home.continue_watching.filter((c) => (kind === 'movies' ? c.movie_id : c.show_id));
  const select = h('select', {
    'aria-label': 'Sort',
    onchange: (e) => { setPref(`sort.${kind}`, e.target.value); location.hash = `#/${kind}?sort=${e.target.value}`; },
  }, SORTS.map(([v, l]) => h('option', { value: v, selected: v === sort }, l)));
  append(el, 
    h('div.page-head', h('h1', kind === 'movies' ? 'Movies' : 'Shows'), h('span.count', items.length), h('div.spacer'), select),
    row('Continue watching', resume.map(continueCard)),
    items.length ? h('div.grid', items.map(kind === 'movies' ? movieCard : showCard)) : h('div.empty', h('p', 'Nothing here yet.')),
  );
}

async function searchView(el, _m, params) {
  const q = params.get('q') || '';
  if (document.activeElement !== search) search.value = q;
  const r = q ? await api(`/search?q=${encodeURIComponent(q)}`) : { movies: [], shows: [] };
  append(el, h('div.page-head', h('h1', q ? `Results for “${q}”` : 'Search')));
  if (r.movies.length) append(el, h('h2.section', 'Movies'), h('div.grid', r.movies.map(movieCard)));
  if (r.shows.length) append(el, h('h2.section', 'Shows'), h('div.grid', r.shows.map(showCard)));
  if (q && !r.movies.length && !r.shows.length) append(el, h('div.empty', h('p', 'No matches in your library.')));
}

function castRow(cast) {
  if (!Array.isArray(cast) || !cast.length) return null;
  return h('section.row',
    h('div.row-head', h('h2', 'Cast')),
    h('div.row-scroll', cast.map((c) => h('div.person',
      h('div.person-img', poster(c.profile_path, c.name, 'w185')),
      h('div.card-title', c.name),
      h('div.card-sub', c.character || ''),
    ))),
  );
}

function fileLabel(f) {
  return [techBadges(f.probe).slice(0, 3).join(' '), bytes(f.size), fileName(f.path)].filter(Boolean).join(' · ');
}

function fileDetails(f) {
  return h('div.file-details',
    h('div.badges', techBadges(f.probe).map((b) => h('span.tag', b))),
    h('div.file-path', { title: f.path }, fileName(f.path), ' · ', bytes(f.size)),
    h('div.file-path', creditsLabel(f), ' · ', h('a.link', { href: '#', onclick: (e) => { e.preventDefault(); creditsDialog(f); } }, 'Edit')),
  );
}

const CREDITS_SOURCES = { chapters: 'from chapters', audio: 'matched across the season', picture: 'detected in the picture', manual: 'set by you' };

function creditsLabel(f) {
  if (f.credits_start) return `Credits at ${clock(f.credits_start)} (${CREDITS_SOURCES[f.credits_source] || 'detected'})`;
  return f.credits_source === 'manual' ? 'No credits (set by you)' : 'Credits not detected';
}

function parseClock(text) {
  const parts = text.trim().split(':').map(Number);
  if (!parts.length || parts.some((n) => !Number.isFinite(n) || n < 0)) return null;
  return parts.reduce((acc, n) => acc * 60 + n, 0);
}

function creditsDialog(f) {
  const input = h('input', { type: 'text', value: f.credits_start ? clock(f.credits_start) : '', placeholder: 'h:mm:ss', 'aria-label': 'Credits start' });
  const error = h('div.error.small');
  const save = async (body) => {
    try {
      await api(`/files/${f.id}/credits`, { method: 'PUT', body });
      close();
      toast(body.mode === 'auto' ? 'Detecting credits again in the background' : 'Credits saved');
      rerender();
    } catch (e) {
      error.textContent = e.message;
    }
  };
  const { close } = modal(h('div.resume',
    h('h2', 'End credits'),
    h('p.muted', `${creditsLabel(f)}. Playback offers Up next or Skip credits from here, and the item counts as watched once you reach it.`),
    h('label.field', 'Credits start at ', input),
    error,
    h('div.resume-actions',
      h('button.btn.primary', {
        onclick: () => {
          const t = parseClock(input.value);
          if (t === null || t <= 0 || (f.duration && t >= f.duration)) { error.textContent = 'Enter a time within the video, like 1:42:10'; return; }
          save({ mode: 'manual', start: t });
        },
      }, 'Save'),
      h('button.btn', { onclick: () => save({ mode: 'manual', start: null }) }, 'No credits'),
      h('button.btn.ghost', { onclick: () => save({ mode: 'auto' }) }, 'Detect again'),
    ),
  ));
  input.focus();
}

function playButtons(f, play) {
  const resume = f.position_ms > 0 && !f.watched;
  return [h('button.btn.primary', { onclick: () => play(undefined) }, icon('play'), resume ? ' Resume' : ' Play')];
}

function resumeLine(f) {
  if (!f || f.watched || !f.position_ms || !f.duration_ms) return null;
  return h('div.resume-line',
    h('div.resume-bar', h('div', { style: { width: `${Math.min(100, (f.position_ms / f.duration_ms) * 100)}%` } })),
    h('span.muted.small', `${clock((f.duration_ms - f.position_ms) / 1000)} left · stopped at ${clock(f.position_ms / 1000)}`),
  );
}

async function setWatched(files, watched) {
  await Promise.all(files.map((f) => api(`/files/${f.id}/watched`, { method: 'POST', body: { watched } })));
}

async function movieView(el, id) {
  const m = await api(`/movies/${id}`);
  let file = m.files[0];
  const actions = h('div.actions');
  const details = h('div');
  const watched = m.files.some((f) => f.watched);
  const renderFile = () => {
    append(clear(actions),
      ...(file ? playButtons(file, (startAt) => openPlayer({ fileId: file.id, title: m.title, subtitle: m.year ? String(m.year) : '', startAt })) : []),
      h('button.btn', { onclick: async () => { await setWatched(m.files, !watched); rerender(); } }, icon('check'), watched ? ' Mark unwatched' : ' Mark watched'),
      file ? h('button.btn.ghost', { onclick: () => matchDialog(m.files, { kind: 'movie', query: m.title, year: m.year }, rerender) }, icon('wrench'), ' Fix match') : null,
    );
    append(clear(details), resumeLine(file), file ? fileDetails(file) : h('p.muted', 'No playable file.'));
  };
  const versions = m.files.length > 1
    ? h('select.versions', { 'aria-label': 'Version', onchange: (e) => { file = m.files.find((f) => f.id === Number(e.target.value)); renderFile(); } },
      m.files.map((f) => h('option', { value: f.id }, fileLabel(f))))
    : null;
  renderFile();
  append(el, 
    hero(m.backdrop_path),
    h('div.detail',
      h('div.detail-poster', poster(m.poster_path, m.title, 'w500')),
      h('div.detail-body',
        h('h1', m.title),
        h('div.meta', [m.year, duration((m.runtime || 0) * 60), m.rating ? `★ ${m.rating.toFixed(1)}` : null, m.genres.join(', ')].filter(Boolean).join(' · ')),
        m.tagline ? h('p.tagline', m.tagline) : null,
        actions,
        versions,
        details,
        m.overview ? h('p.overview', m.overview) : null,
      ),
    ),
    castRow(m.cast),
  );
}

async function showView(el, id, params) {
  const s = await api(`/shows/${id}`);
  if (!s.seasons.length) {
    append(el, hero(s.backdrop_path, h('h1', s.title)), h('div.empty', h('p', 'No episodes in the library.')));
    return;
  }
  const wanted = params.get('season');
  const firstUnwatched = s.seasons.find((x) => x.season_number > 0 && x.watched < x.available);
  const current = s.seasons.find((x) => String(x.season_number) === wanted) || firstUnwatched || s.seasons[0];
  const episodes = await api(`/shows/${id}/seasons/${current.season_number}`);
  const label = (season) => season.name || (season.season_number === 0 ? 'Specials' : `Season ${season.season_number}`);
  const play = (e, startAt) => openPlayer({
    fileId: e.files[0].id,
    title: s.title,
    subtitle: `S${e.season_number} · E${e.episode_number}${e.title ? ' — ' + e.title : ''}`,
    startAt,
    showId: id,
  });
  const upNext = episodes.find((e) => e.files.length && !e.files[0].watched);
  const episodeRow = (e) => {
    const f = e.files[0];
    const frac = f && f.duration_ms ? f.position_ms / f.duration_ms : 0;
    return h('div.episode' + (f ? '' : '.missing'),
      h(f ? 'button.ep-art' : 'div.ep-art', f ? { onclick: () => play(e), 'aria-label': `Play episode ${e.episode_number}` } : {},
        poster(e.still_path, '', 'w300'), f ? h('span.play-overlay', icon('play')) : null, progressBar(frac),
        f?.watched ? h('span.badge-watched', icon('check')) : null),
      h('div.ep-body',
        h('div.ep-title', `${e.episode_number}. ${e.title || `Episode ${e.episode_number}`}`),
        h('div.ep-meta', [e.air_date, e.runtime ? `${e.runtime}m` : null, f ? techBadges(f.probe).slice(0, 2).join(' ') : 'Not in library',
          f && !f.watched && f.position_ms ? `${clock((f.duration_ms - f.position_ms) / 1000)} left` : null].filter(Boolean).join(' · ')),
        e.overview ? h('p.ep-overview', e.overview) : null,
      ),
      f ? h('div.ep-actions',
        h('button.icon-btn' + (f.watched ? '.on' : ''), { title: f.watched ? 'Mark unwatched' : 'Mark watched', onclick: async () => { await setWatched(e.files, !f.watched); rerender(); } }, icon('check')),
        h('button.icon-btn', { title: creditsLabel(f), onclick: () => creditsDialog(f) }, icon('credits')),
        h('button.icon-btn', { title: 'Fix match', onclick: () => matchDialog(e.files, { kind: 'episode', query: s.title, season: e.season_number, episode: e.episode_number }, rerender) }, icon('wrench')),
      ) : null,
    );
  };
  append(el, 
    hero(s.backdrop_path),
    h('div.detail',
      h('div.detail-poster', poster(s.poster_path, s.title, 'w500')),
      h('div.detail-body',
        h('h1', s.title),
        h('div.meta', [s.year, s.status, s.rating ? `★ ${s.rating.toFixed(1)}` : null, s.genres.join(', ')].filter(Boolean).join(' · ')),
        h('div.actions',
          upNext ? h('button.btn.primary', { onclick: () => play(upNext) }, icon('play'), ` ${upNext.files[0].position_ms ? 'Resume' : 'Play'} S${upNext.season_number} · E${upNext.episode_number}`) : null,
          h('button.btn', { onclick: async () => { await setWatched(episodes.flatMap((e) => e.files), current.watched < current.available); rerender(); } },
            icon('check'), current.watched < current.available ? ` Mark ${label(current)} watched` : ` Mark ${label(current)} unwatched`),
        ),
        s.overview ? h('p.overview', s.overview) : null,
      ),
    ),
    h('div.seasons', s.seasons.map((x) => h('a.pill' + (x === current ? '.active' : ''), { href: `#/show/${id}?season=${x.season_number}` },
      label(x), h('span.pill-count', `${x.available}${x.episode_count ? '/' + x.episode_count : ''}`)))),
    h('div.episodes', episodes.map(episodeRow)),
    castRow(s.cast),
  );
}


async function settingsView(el) {
  const [status, hw] = await Promise.all([api('/status'), api('/hardware')]);
  const c = status.counts;
  const scan = status.scan;
  const report = scan.last_report;
  const scanBtn = h('button.btn.primary', {
    disabled: scan.running,
    onclick: async () => {
      await api('/scan', { method: 'POST' });
      toast('Scan started');
      setTimeout(rerender, 800);
    },
  }, icon('refresh'), scan.running ? ` Scanning… ${scan.done}/${scan.total}` : ' Scan library');
  if (scan.running) setTimeout(() => { if (location.hash === '#/settings') rerender(); }, 3000);

  const stat = (n, label) => h('div.stat', h('div.stat-n', n.toLocaleString()), h('div.stat-l', label));
  append(el, 
    h('div.page-head', h('h1', 'Settings')),
    h('section.panel',
      h('div.panel-head', h('h2', 'Library'), h('div.spacer'), scanBtn),
      h('div.stats', stat(c.movies, 'Movies'), stat(c.shows, 'Shows'), stat(c.episodes, 'Episodes'), stat(c.unmatched + c.pending, 'Unmatched')),
      h('p.muted',
        scan.running ? `Scanning: ${scan.phase} (${scan.done}/${scan.total})` : scan.last_finished ? `Last scan ${new Date(scan.last_finished).toLocaleString()}` : 'Not scanned yet',
        report ? ` · ${report.files} files, ${report.added} added, ${report.removed} removed, ${report.matched} matched` : '',
        status.tmdb ? '' : ' · TMDB key missing'),
    ),
    h('section.panel',
      h('div.panel-head', h('h2', 'Hardware acceleration')),
      h('div.table-wrap', h('table',
        h('thead', h('tr', ['Device', 'API', 'Type', 'Decode', 'Encode', 'Sessions'].map((t) => h('th', t)))),
        h('tbody', hw.devices.map((d) => h('tr',
          h('td', d.name),
          h('td', d.api.toUpperCase()),
          h('td', d.kind),
          h('td.codecs', d.decode.join(', ')),
          h('td.codecs', d.encode.join(', ')),
          h('td', d.max_sessions > 1000 ? `${d.active}` : `${d.active} / ${d.max_sessions}`),
        ))),
      )),
      hw.transcodes.length
        ? h('div', h('h3', 'Active streams'), h('ul.plain', hw.transcodes.map((t) => h('li', `File ${t.file_id}: ${t.plan.codec.toUpperCase()} on ${t.plan.device}${t.plan.tonemap ? ', tone map ' + t.plan.tonemap : ''}`))))
        : h('p.muted', 'No active streams.'),
    ),
    h('section.panel',
      h('div.panel-head', h('h2', 'This browser')),
      h('label.field', 'Default quality ',
        h('select', { onchange: (e) => setPref('quality', e.target.value) },
          [['auto', 'Original'], ['2160', '4K'], ['1080', '1080p'], ['720', '720p'], ['480', '480p']].map(([v, l]) => h('option', { value: v, selected: pref('quality', 'auto') === v }, l)))),
      h('div.seek-steps',
        [['short', '← → arrow'], ['medium', '⇧ + arrow'], ['long', '⌘/Ctrl ⇧ + arrow']].map(([k, label]) => [
          h('label', { for: `seek-${k}` }, `Seek ${label} (seconds)`),
          h('input', {
            id: `seek-${k}`, type: 'number', min: 1, max: 3600, value: seekSteps()[k],
            onchange: (e) => {
              const v = Math.round(Number(e.target.value));
              if (v >= 1) setPref(`seek.${k}`, v); else e.target.value = seekSteps()[k];
            },
          }),
        ])),
      h('button.btn.small', { onclick: () => { for (const k of Object.keys(SEEK_DEFAULTS)) setPref(`seek.${k}`, SEEK_DEFAULTS[k]); rerender(); } }, 'Reset seek steps'),
      h('p.muted', `Server ${status.version}`),
      h('button.btn', { onclick: () => { setToken(''); login(); } }, 'Sign out'),
    ),
  );
}

start();
