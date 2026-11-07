import { api, img } from './api.js';
import { h, append, icon, bytes, fileName, dirName, duration, toast, modal } from './dom.js';

function guessText(g) {
  if (!g) return '';
  if (g.kind === 'movie') return `Movie · ${g.title}${g.year ? ` (${g.year})` : ''}`;
  if (g.kind === 'dated_episode') return `Show · ${g.show} · ${g.date}`;
  const ep = g.episodes?.length ? `E${g.episodes.join('-E')}` : '';
  return `Show · ${g.show}${g.season != null ? ` · S${g.season}` : ''}${ep ? ` ${ep}` : ''}`;
}

function hintFrom(files) {
  const g = files.find((f) => f.guess)?.guess;
  if (!g) return { kind: 'movie', query: '' };
  return g.kind === 'movie'
    ? { kind: 'movie', query: g.title, year: g.year }
    : { kind: 'episode', query: g.show, year: g.year };
}

export async function fixView(el, refreshStatus) {
  const files = await api('/unmatched');
  const selected = new Set();
  const matchBtn = h('button.btn.primary', { disabled: true }, icon('wrench'), ' Match selected');
  const update = () => {
    matchBtn.disabled = selected.size === 0;
    matchBtn.lastChild.textContent = selected.size ? ` Match ${selected.size} selected` : ' Match selected';
  };
  const done = () => { refreshStatus?.(); window.dispatchEvent(new HashChangeEvent('hashchange')); };
  matchBtn.addEventListener('click', () => {
    const picked = files.filter((f) => selected.has(f.id));
    matchDialog(picked, hintFrom(picked), done);
  });

  append(el, h('div.page-head',
    h('h1', 'Fix matches'), h('span.count', files.length), h('div.spacer'),
    h('button.btn', { onclick: async () => { await api('/scan', { method: 'POST' }); toast('Scan started'); } }, icon('refresh'), ' Rescan'),
    matchBtn,
  ));
  if (!files.length) {
    append(el, h('div.empty', h('h2', 'Everything is matched'), h('p', 'Files the scanner can’t identify will show up here.')));
    return;
  }
  append(el, h('p.muted.lead', 'These files couldn’t be matched to TMDB automatically. Select files from the same movie or show and match them together; your choice is remembered across rescans.'));

  const groups = new Map();
  for (const f of files) {
    const d = dirName(f.path);
    if (!groups.has(d)) groups.set(d, []);
    groups.get(d).push(f);
  }
  for (const [dir, list] of groups) {
    const boxes = [];
    const all = h('input', {
      type: 'checkbox',
      'aria-label': 'Select folder',
      onchange: (e) => {
        for (const [box, f] of boxes) {
          box.checked = e.target.checked;
          e.target.checked ? selected.add(f.id) : selected.delete(f.id);
        }
        update();
      },
    });
    append(el, h('section.fix-group',
      h('div.fix-head', h('label', all, h('span.fix-dir', dir.replace(/^\/media\//, '') || '/')), h('span.count', list.length), h('div.spacer'),
        h('button.btn.small', { onclick: () => matchDialog(list, hintFrom(list), done) }, 'Match folder')),
      list.map((f) => {
        const box = h('input', {
          type: 'checkbox',
          'aria-label': 'Select file',
          onchange: (e) => { e.target.checked ? selected.add(f.id) : selected.delete(f.id); update(); },
        });
        boxes.push([box, f]);
        return h('div.fix-row',
          h('label.fix-file', box, h('span', fileName(f.path))),
          h('div.fix-guess', guessText(f.guess)),
          h('div.fix-note', [f.match_note, duration(f.duration), bytes(f.size)].filter(Boolean).join(' · ')),
          h('button.btn.small.ghost', { onclick: () => matchDialog([f], hintFrom([f]), done) }, 'Match'),
        );
      }),
    ));
  }
}

export function matchDialog(files, hint = {}, onDone) {
  files = [...files].sort((a, b) => a.path.localeCompare(b.path, undefined, { numeric: true }));
  let kind = hint.kind === 'movie' ? 'movie' : 'tv';
  let chosen = null;

  const query = h('input', { type: 'search', value: hint.query || '', placeholder: 'Title', 'aria-label': 'Title' });
  const year = h('input.year', { type: 'number', value: hint.year || '', placeholder: 'Year', 'aria-label': 'Year', min: 1880, max: 2100 });
  const results = h('div.tmdb-results');
  const kindBtns = h('div.segmented');
  const numbering = h('div.numbering');
  const submit = h('button.btn.primary', { disabled: true }, 'Match');
  const status = h('div.muted');

  const rows = files.map((f, i) => {
    const g = f.guess?.kind === 'episode' ? f.guess : null;
    const season = h('input.num', { type: 'number', min: 0, value: g?.season ?? hint.season ?? 1, 'aria-label': 'Season' });
    const episode = h('input.num', { type: 'number', min: 0, value: g?.episodes?.[0] ?? hint.episode ?? i + 1, 'aria-label': 'Episode' });
    return { f, season, episode };
  });
  const startSeason = h('input.num', { type: 'number', min: 0, value: rows[0]?.season.value || 1, 'aria-label': 'Start season' });
  const startEpisode = h('input.num', { type: 'number', min: 1, value: 1, 'aria-label': 'Start episode' });
  const renumber = h('button.btn.small', {
    onclick: () => rows.forEach((r, i) => { r.season.value = startSeason.value; r.episode.value = Number(startEpisode.value) + i; }),
  }, 'Number in order');

  const renderKind = () => {
    kindBtns.replaceChildren(
      h('button' + (kind === 'movie' ? '.active' : ''), { onclick: () => { kind = 'movie'; chosen = null; renderKind(); runSearch(); } }, 'Movie'),
      h('button' + (kind === 'tv' ? '.active' : ''), { onclick: () => { kind = 'tv'; chosen = null; renderKind(); runSearch(); } }, 'Show'),
    );
    numbering.replaceChildren(...(kind === 'tv'
      ? [
        h('div.numbering-tools', 'Season ', startSeason, ' from episode ', startEpisode, renumber),
        h('div.numbering-rows', rows.map((r) => h('div.numbering-row', h('span.fix-file', { title: r.f.path }, fileName(r.f.path)), 'S', r.season, 'E', r.episode))),
      ]
      : files.length > 1 ? [h('p.muted', `All ${files.length} files become versions of the same movie.`)] : []));
    updateSubmit();
  };

  const updateSubmit = () => {
    submit.disabled = !chosen;
    submit.textContent = chosen ? `Match ${files.length > 1 ? files.length + ' files ' : ''}to ${chosen.title}` : 'Pick a result';
  };

  let searchSeq = 0;
  async function runSearch() {
    const q = query.value.trim();
    if (!q) { results.replaceChildren(); return; }
    const mine = ++searchSeq;
    results.replaceChildren(h('div.muted', 'Searching TMDB…'));
    try {
      const params = new URLSearchParams({ q, kind });
      if (year.value) params.set('year', year.value);
      const hits = await api(`/tmdb/search?${params}`);
      if (mine !== searchSeq) return;
      results.replaceChildren(...(hits.length ? hits.map((hit) => {
        const item = h('button.tmdb-hit' + (chosen?.id === hit.id ? '.active' : ''), {
          onclick: () => {
            chosen = hit;
            results.querySelectorAll('.tmdb-hit').forEach((x) => x.classList.toggle('active', x === item));
            updateSubmit();
          },
        },
          hit.poster_path ? h('img', { src: img(hit.poster_path, 'w92'), alt: '', loading: 'lazy' }) : h('div.hit-noimg'),
          h('div.hit-body',
            h('div.hit-title', hit.title, hit.year ? h('span.muted', ` (${hit.year})`) : null),
            hit.original_title && hit.original_title !== hit.title ? h('div.muted.small', hit.original_title) : null,
            hit.overview ? h('div.hit-overview', hit.overview) : null,
            h('div.muted.small', `TMDB ${hit.id}`),
          ),
        );
        return item;
      }) : [h('div.muted', 'No results. Try another title or remove the year.')]));
    } catch (e) {
      if (mine === searchSeq) results.replaceChildren(h('div.error', e.message));
    }
  }

  submit.addEventListener('click', async () => {
    submit.disabled = true;
    const failed = [];
    let ok = 0;
    for (const [i, r] of rows.entries()) {
      status.textContent = `Matching ${i + 1} of ${rows.length}…`;
      const body = kind === 'movie'
        ? { kind: 'movie', tmdb_id: chosen.id }
        : { kind: 'tv', tmdb_id: chosen.id, season: Number(r.season.value), episode: Number(r.episode.value) };
      try {
        const res = await api(`/files/${r.f.id}/match`, { method: 'POST', body });
        if (res.matched) ok++; else failed.push(`${fileName(r.f.path)}: ${kind === 'tv' ? `no S${body.season}E${body.episode} on TMDB` : 'not found'}`);
      } catch (e) {
        failed.push(`${fileName(r.f.path)}: ${e.message}`);
      }
    }
    if (failed.length) {
      status.replaceChildren(h('div.error', `${ok} matched, ${failed.length} failed:`), h('ul.plain.small', failed.map((x) => h('li', x))));
      submit.disabled = false;
    } else {
      toast(`Matched ${ok} file${ok === 1 ? '' : 's'} to ${chosen.title}`);
      close();
    }
    if (ok) changed = true;
  });

  let changed = false;
  query.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); runSearch(); } });
  year.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); runSearch(); } });

  const { close } = modal(h('div.match',
    h('h2', files.length === 1 ? 'Match file' : `Match ${files.length} files`),
    h('div.muted.small.path', files.length === 1 ? files[0].path : dirName(files[0].path)),
    kindBtns,
    h('div.search-row', query, year, h('button.btn', { onclick: runSearch }, icon('search'), ' Search')),
    results,
    numbering,
    status,
    h('div.modal-actions', submit),
  ), { wide: true, onclose: () => { if (changed) onDone?.(); } });

  renderKind();
  runSearch();
  query.focus();
}
