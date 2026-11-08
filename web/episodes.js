import { api } from './api.js';

const shows = new Map();
const seasons = new Map();

export async function show(showId) {
  if (!shows.has(showId)) shows.set(showId, api(`/shows/${showId}`).catch((e) => { shows.delete(showId); throw e; }));
  return shows.get(showId);
}

export async function season(showId, number) {
  const key = `${showId}/${number}`;
  if (!seasons.has(key)) seasons.set(key, api(`/shows/${showId}/seasons/${number}`).catch((e) => { seasons.delete(key); throw e; }));
  return seasons.get(key);
}

export function label(e) {
  return `S${e.season_number} · E${e.episode_number}${e.title ? ' — ' + e.title : ''}`;
}

export function item(s, e) {
  return { fileId: e.files[0].id, title: s.title, subtitle: label(e), showId: s.id };
}

/// Seasons in viewing order: regular seasons first, specials last.
export function seasonOrder(s) {
  return [...s.seasons].sort((a, b) => (a.season_number === 0) - (b.season_number === 0) || a.season_number - b.season_number);
}

/// The playable episode `step` places away from `fileId` (1 = next, -1 = previous), crossing seasons but not
/// into or out of specials.
export async function neighbour(showId, fileId, step) {
  const s = await show(showId);
  const order = seasonOrder(s);
  const all = [];
  let at = -1;
  for (const x of order) {
    const eps = (await season(showId, x.season_number)).filter((e) => e.files.length);
    for (const e of eps) {
      if (e.files.some((f) => f.id === fileId)) at = all.length;
      all.push(e);
    }
    if (at >= 0 && step > 0 && all.length > at + 1) break;
  }
  if (at < 0) return null;
  const here = all[at];
  const target = all[at + step];
  if (!target || (target.season_number === 0) !== (here.season_number === 0)) return null;
  return item(s, target);
}

/// Season and episode number of a file, for opening the episode list where the viewer is.
export async function locate(showId, fileId) {
  const s = await show(showId);
  for (const x of seasonOrder(s)) {
    const eps = await season(showId, x.season_number);
    const e = eps.find((ep) => ep.files.some((f) => f.id === fileId));
    if (e) return { season: x.season_number, episode: e.episode_number };
  }
  return null;
}
