use crate::{
    config::Config,
    hwaccel::Hardware,
    transcode::{trickplay, Transcoder},
    parser::{self, Guess},
    probe,
    tmdb::{MovieHit, Scored, TvDetails, TvHit, Tmdb},
};
use anyhow::Result;
use futures::StreamExt;
use serde::Serialize;
use sqlx::{types::Json, SqlitePool};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::UNIX_EPOCH,
};
use tokio::sync::{Mutex, RwLock};

pub struct Ctx {
    pub db: SqlitePool,
    pub tmdb: Option<Tmdb>,
    pub cfg: Config,
    pub hw: Arc<Hardware>,
    pub transcoder: Arc<Transcoder>,
    pub keyframes: Arc<crate::transcode::Keyframes>,
    scan_lock: Mutex<()>,
    trickplay_lock: Mutex<()>,
    pub status: RwLock<ScanStatus>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ScanStatus {
    pub running: bool,
    pub phase: String,
    pub done: usize,
    pub total: usize,
    pub last_finished: Option<chrono::DateTime<chrono::Utc>>,
    pub last_report: Option<ScanReport>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ScanReport {
    pub files: usize,
    pub added: usize,
    pub changed: usize,
    pub removed: usize,
    pub probed: usize,
    pub matched: usize,
    pub unmatched: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    Movie(i64),
    Episodes { tv: i64, episodes: Vec<(i64, i64)> },
    Unmatched(String),
}

const MOVIE_MIN: f64 = 0.88;
const TV_MIN: f64 = 0.85;
const SEQUEL_MIN: f64 = 0.92;

impl Ctx {
    pub fn new(db: SqlitePool, cfg: Config, hw: Arc<Hardware>) -> Arc<Self> {
        let tmdb = cfg.tmdb_key.clone().map(|k| Tmdb::new(k, cfg.language.clone()));
        let keyframes = crate::transcode::Keyframes::new(cfg.ffprobe.clone(), cfg.data_dir.join("keyframes"));
        let transcoder = Transcoder::new(hw.clone(), cfg.data_dir.join("transcode"), keyframes.clone());
        Arc::new(Self {
            db,
            tmdb,
            cfg,
            hw,
            transcoder,
            keyframes,
            scan_lock: Mutex::new(()),
            trickplay_lock: Mutex::new(()),
            status: Default::default(),
        })
    }

    async fn set_phase(&self, phase: &str, done: usize, total: usize) {
        let mut s = self.status.write().await;
        s.phase = phase.into();
        s.done = done;
        s.total = total;
    }
}

fn walk(dirs: &[PathBuf]) -> Vec<(PathBuf, i64, i64)> {
    let mut out = vec![];
    for dir in dirs {
        for entry in walkdir::WalkDir::new(dir).follow_links(true).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            let hidden = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'));
            if !entry.file_type().is_file() || hidden || !parser::is_video(path) {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
            out.push((path.to_path_buf(), meta.len() as i64, mtime));
        }
    }
    out
}

pub async fn scan(ctx: &Arc<Ctx>) -> Result<Option<ScanReport>> {
    let Ok(_guard) = ctx.scan_lock.try_lock() else { return Ok(None) };
    ctx.status.write().await.running = true;
    let result = scan_inner(ctx).await;
    let mut s = ctx.status.write().await;
    s.running = false;
    s.phase = match &result {
        Ok(_) => "idle".into(),
        Err(e) => format!("failed: {e:#}"),
    };
    s.last_finished = Some(chrono::Utc::now());
    if let Ok(r) = &result {
        s.last_report = Some(r.clone());
    }
    drop(s);
    if ctx.cfg.trickplay {
        let ctx = ctx.clone();
        tokio::spawn(async move { trickplay_pass(&ctx).await });
    }
    result.map(Some)
}

/// Generate seek thumbnails for every matched file that lacks them, one at a time in the background.
pub async fn trickplay_pass(ctx: &Arc<Ctx>) {
    let Ok(_guard) = ctx.trickplay_lock.try_lock() else { return };
    let files: Vec<(i64, String, Json<probe::MediaInfo>)> = match sqlx::query_as(
        "SELECT id, path, probe FROM files WHERE kind IN ('movie', 'episode') AND probe IS NOT NULL ORDER BY added_at DESC",
    )
    .fetch_all(&ctx.db)
    .await
    {
        Ok(f) => f,
        Err(e) => return tracing::warn!("trickplay query failed: {e:#}"),
    };
    for (id, path, media) in files {
        let video = media.video.as_ref();
        if video.is_some_and(|v| v.codec == "h264" && v.bit_depth.unwrap_or(8) <= 8 && v.hdr.is_none()) {
            if let Err(e) = ctx.keyframes.extract(id, std::path::Path::new(&path)).await {
                tracing::warn!(path, "keyframe index failed: {e:#}");
            }
        }
        if trickplay::dir(&ctx.cfg.data_dir, id).join("manifest.json").exists() {
            continue;
        }
        if let Err(e) = trickplay::generate(&ctx.hw, &ctx.cfg.data_dir, id, std::path::Path::new(&path), &media).await {
            tracing::warn!(path, "trickplay failed: {e:#}");
        }
    }
}

async fn scan_inner(ctx: &Arc<Ctx>) -> Result<ScanReport> {
    let mut report = ScanReport::default();
    ctx.set_phase("walking", 0, 0).await;
    let dirs = ctx.cfg.media_dirs.clone();
    let found = tokio::task::spawn_blocking(move || walk(&dirs)).await?;
    report.files = found.len();

    let existing: HashMap<String, (i64, i64, i64, String)> =
        sqlx::query_as::<_, (i64, String, i64, i64, String)>("SELECT id, path, size, mtime, guess FROM files")
            .fetch_all(&ctx.db)
            .await?
            .into_iter()
            .map(|(id, path, size, mtime, guess)| (path, (id, size, mtime, guess)))
            .collect();

    let present: HashSet<String> = found.iter().map(|(p, ..)| p.to_string_lossy().into_owned()).collect();
    for (path, (id, ..)) in &existing {
        if !present.contains(path) {
            sqlx::query("DELETE FROM files WHERE id = ?").bind(id).execute(&ctx.db).await?;
            report.removed += 1;
        }
    }

    let paths: Vec<PathBuf> = found.iter().map(|(p, ..)| p.clone()).collect();
    let guesses = parser::parse_batch(&paths);
    let mut disc_main: HashMap<&std::path::Path, (&PathBuf, i64)> = HashMap::new();
    for (path, size, _) in &found {
        if let Some(root) = parser::disc_root(path) {
            let e = disc_main.entry(root).or_insert((path, *size));
            if *size > e.1 {
                *e = (path, *size);
            }
        }
    }
    let is_disc_extra = |path: &PathBuf| parser::disc_root(path).is_some_and(|root| disc_main.get(root).is_some_and(|(main, _)| *main != path));
    let mut tx = ctx.db.begin().await?;
    for ((path, size, mtime), guess) in found.iter().zip(&guesses) {
        let key = path.to_string_lossy().into_owned();
        let guess_json = serde_json::to_string(guess)?;
        let kind = if parser::is_extra(path) || is_disc_extra(path) { "extra" } else { "pending" };
        match existing.get(&key) {
            None => {
                sqlx::query("INSERT INTO files (path, size, mtime, guess, kind) VALUES (?, ?, ?, ?, ?)")
                    .bind(&key).bind(size).bind(mtime).bind(&guess_json).bind(kind)
                    .execute(&mut *tx).await?;
                report.added += 1;
            }
            Some((id, s, m, _)) if s != size || m != mtime => {
                sqlx::query("UPDATE files SET size = ?, mtime = ?, guess = ?, kind = ?, probe = NULL, duration = NULL, movie_id = NULL WHERE id = ?")
                    .bind(size).bind(mtime).bind(&guess_json).bind(kind).bind(id)
                    .execute(&mut *tx).await?;
                sqlx::query("DELETE FROM file_episodes WHERE file_id = ?").bind(id).execute(&mut *tx).await?;
                report.changed += 1;
            }
            Some((id, _, _, g)) if *g != guess_json => {
                sqlx::query("UPDATE files SET guess = ?, kind = ?, movie_id = NULL WHERE id = ?")
                    .bind(&guess_json).bind(kind).bind(id)
                    .execute(&mut *tx).await?;
                sqlx::query("DELETE FROM file_episodes WHERE file_id = ?").bind(id).execute(&mut *tx).await?;
                report.changed += 1;
            }
            _ => {}
        }
    }
    tx.commit().await?;

    let to_probe: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, path FROM files WHERE probe IS NULL AND kind != 'extra'").fetch_all(&ctx.db).await?;
    let total = to_probe.len();
    let probed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    futures::stream::iter(to_probe)
        .for_each_concurrent(4, |(id, path)| {
            let probed = probed.clone();
            async move {
                match probe::probe(&ctx.cfg.ffprobe, std::path::Path::new(&path)).await {
                    Ok(info) => {
                        let _ = sqlx::query("UPDATE files SET probe = ?, duration = ? WHERE id = ?")
                            .bind(Json(&info)).bind(info.duration).bind(id)
                            .execute(&ctx.db).await;
                    }
                    Err(e) => tracing::warn!(path, "probe failed: {e:#}"),
                }
                let n = probed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if n % 10 == 0 || n == total {
                    ctx.set_phase("probing", n, total).await;
                }
            }
        })
        .await;
    report.probed = total;

    if ctx.tmdb.is_some() {
        let pending: Vec<(i64, String, String, Option<f64>)> =
            sqlx::query_as("SELECT id, path, guess, duration FROM files WHERE kind = 'pending'").fetch_all(&ctx.db).await?;
        let total = pending.len();
        let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let results: Vec<bool> = futures::stream::iter(pending)
            .map(|(id, path, guess, duration)| {
                let done = done.clone();
                async move {
                    let ok = match match_file(ctx, id, &path, &guess, duration).await {
                        Ok(matched) => matched,
                        Err(e) => {
                            tracing::warn!(path, "match failed: {e:#}");
                            false
                        }
                    };
                    let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    if n % 10 == 0 || n == total {
                        ctx.set_phase("matching", n, total).await;
                    }
                    ok
                }
            })
            .buffer_unordered(6)
            .collect()
            .await;
        report.matched = results.iter().filter(|r| **r).count();
        report.unmatched = results.len() - report.matched;
    }

    sqlx::query("DELETE FROM movies WHERE id NOT IN (SELECT movie_id FROM files WHERE movie_id IS NOT NULL)").execute(&ctx.db).await?;
    sqlx::query(
        "DELETE FROM shows WHERE id NOT IN (SELECT e.show_id FROM episodes e JOIN file_episodes fe ON fe.episode_id = e.id)",
    )
    .execute(&ctx.db)
    .await?;
    tracing::info!(?report, "scan finished");
    Ok(report)
}

async fn match_file(ctx: &Ctx, id: i64, path: &str, guess: &str, duration: Option<f64>) -> Result<bool> {
    let tmdb = ctx.tmdb.as_ref().expect("tmdb configured");
    let manual: Option<(String, i64, Option<i64>, Option<i64>)> =
        sqlx::query_as("SELECT kind, tmdb_id, season, episode FROM manual_matches WHERE path = ?")
            .bind(path)
            .fetch_optional(&ctx.db)
            .await?;
    let resolution = match manual {
        Some((kind, tmdb_id, _, _)) if kind == "movie" => Resolution::Movie(tmdb_id),
        Some((_, tmdb_id, Some(season), Some(episode))) => Resolution::Episodes { tv: tmdb_id, episodes: vec![(season, episode)] },
        _ => {
            let guess: Guess = serde_json::from_str(guess)?;
            resolve(tmdb, &guess, duration, std::path::Path::new(path)).await?
        }
    };
    apply(ctx, id, resolution).await
}

async fn best_movie(tmdb: &Tmdb, variants: &[String], year: Option<i32>) -> Result<Option<Scored<MovieHit>>> {
    for v in variants {
        if let Some(best) = tmdb.search_movie(v, year).await?.into_iter().next().filter(|m| m.score >= MOVIE_MIN) {
            return Ok(Some(best));
        }
    }
    Ok(None)
}

async fn best_tv(tmdb: &Tmdb, variants: &[String], year: Option<i32>) -> Result<Option<Scored<TvHit>>> {
    for v in variants {
        if let Some(best) = tmdb.search_tv(v, year).await?.into_iter().next().filter(|s| s.score >= TV_MIN) {
            return Ok(Some(best));
        }
        if year.is_some() {
            if let Some(best) = tmdb.search_tv(v, None).await?.into_iter().next().filter(|s| s.score >= TV_MIN) {
                return Ok(Some(best));
            }
        }
    }
    Ok(None)
}

/// `Shrek 3` style names: the nth released film of the base title's TMDB collection, else an exact `Title N` film.
async fn numbered_movie(tmdb: &Tmdb, base: &str, year: Option<i32>, n: u32) -> Result<Option<(i64, f64)>> {
    if let Some(b) = tmdb.search_movie(base, None).await?.into_iter().next().filter(|m| m.score >= MOVIE_MIN) {
        if let Some(c) = tmdb.movie(b.hit.id).await?.belongs_to_collection {
            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
            let mut parts: Vec<_> = tmdb
                .collection(c.id)
                .await?
                .parts
                .into_iter()
                .filter(|p| p.release_date.as_deref().is_some_and(|d| !d.is_empty() && d <= today.as_str()))
                .collect();
            parts.sort_by(|a, b| a.release_date.cmp(&b.release_date));
            if let Some(p) = n.checked_sub(1).and_then(|i| parts.get(i as usize)) {
                return Ok(Some((p.id, b.score)));
            }
        }
    }
    Ok(tmdb
        .search_movie(&format!("{base} {n}"), year)
        .await?
        .into_iter()
        .next()
        .filter(|m| m.score >= SEQUEL_MIN)
        .map(|m| (m.hit.id, m.score)))
}

pub async fn resolve(tmdb: &Tmdb, guess: &Guess, duration: Option<f64>, path: &std::path::Path) -> Result<Resolution> {
    let variants = parser::title_variants(guess.title(), path);
    match guess {
        Guess::Movie { title, year } => {
            if let Some(best) = best_movie(tmdb, &variants, *year).await? {
                let details = tmdb.movie(best.hit.id).await?;
                let too_short = match (duration, details.runtime) {
                    (Some(d), Some(rt)) if rt > 0 => d < rt as f64 * 60.0 * 0.5,
                    _ => false,
                };
                if !too_short {
                    return Ok(Resolution::Movie(best.hit.id));
                }
                return Ok(Resolution::Unmatched(format!(
                    "Runtime {}m is too short for the movie \"{}\" ({}m); probably an episode",
                    duration.unwrap_or(0.0) as i64 / 60,
                    details.title,
                    details.runtime.unwrap_or(0)
                )));
            }
            if let Some(s) = best_tv(tmdb, &variants, *year).await?.filter(|s| s.score >= 0.9) {
                return Ok(Resolution::Unmatched(format!(
                    "Looks like the show \"{}\" but the file name has no episode number",
                    s.hit.name
                )));
            }
            Ok(Resolution::Unmatched(format!("No TMDB match for \"{title}\"")))
        }
        Guess::Episode { show, year, season, episodes } => {
            let tv = best_tv(tmdb, &variants, *year).await?;
            if season.is_none() && episodes.len() == 1 {
                if let Some((id, score)) = numbered_movie(tmdb, show, *year, episodes[0]).await? {
                    if tv.as_ref().is_none_or(|t| score > t.score + 0.02) {
                        return Ok(Resolution::Movie(id));
                    }
                }
            }
            let Some(tv) = tv else {
                return Ok(Resolution::Unmatched(format!("No TMDB show matches \"{show}\"")));
            };
            let details = tmdb.tv(tv.hit.id).await?;
            let mapped = map_episodes(&details, *season, episodes);
            if matches!(mapped, Resolution::Unmatched(_)) {
                if let Some(found) = episode_by_title(tmdb, &details, path).await? {
                    return Ok(found);
                }
            }
            Ok(mapped)
        }
        Guess::DatedEpisode { show, date } => {
            let Some(tv) = best_tv(tmdb, &variants, None).await? else {
                return Ok(Resolution::Unmatched(format!("No TMDB show matches \"{show}\"")));
            };
            let details = tmdb.tv(tv.hit.id).await?;
            let year = &date[..4];
            let mut seasons: Vec<_> = details
                .seasons
                .iter()
                .filter(|s| s.season_number > 0 && s.air_date.as_deref().is_none_or(|d| d[..4.min(d.len())] <= *year))
                .collect();
            seasons.sort_by_key(|s| std::cmp::Reverse(s.season_number));
            for s in seasons.into_iter().take(4) {
                let season = tmdb.season(details.id, s.season_number).await?;
                if let Some(e) = season.episodes.iter().find(|e| e.air_date.as_deref() == Some(date)) {
                    return Ok(Resolution::Episodes { tv: details.id, episodes: vec![(e.season_number, e.episode_number)] });
                }
            }
            Ok(Resolution::Unmatched(format!("\"{}\" has no episode that aired on {date}", details.name)))
        }
    }
}

/// Fallback when numbering disagrees with TMDB: find the episode by the title in the file name, specials included.
async fn episode_by_title(tmdb: &Tmdb, details: &TvDetails, path: &std::path::Path) -> Result<Option<Resolution>> {
    let Some(title) = parser::episode_title(path) else { return Ok(None) };
    let wanted = parser::normalize(&title);
    let mut best: Option<(f64, i64, i64)> = None;
    for s in &details.seasons {
        let season = tmdb.season(details.id, s.season_number).await?;
        for e in &season.episodes {
            let Some(name) = &e.name else { continue };
            let score = strsim::jaro_winkler(&wanted, &parser::normalize(name));
            if score >= 0.93 && best.is_none_or(|b| score > b.0) {
                best = Some((score, e.season_number, e.episode_number));
            }
        }
    }
    Ok(best.map(|(_, s, e)| Resolution::Episodes { tv: details.id, episodes: vec![(s, e)] }))
}

pub fn map_episodes(details: &TvDetails, season: Option<u32>, episodes: &[u32]) -> Resolution {
    let tv = details.id;
    let mut regular: Vec<_> = details.seasons.iter().filter(|s| s.season_number > 0).collect();
    regular.sort_by_key(|s| s.season_number);
    let count = |n: i64| details.seasons.iter().find(|s| s.season_number == n).and_then(|s| s.episode_count);

    // Explicit season: overflow into following seasons when TMDB splits a season differently.
    let in_season = |s: i64, e: i64| -> Option<(i64, i64)> {
        let mut season = s;
        let mut left = e;
        loop {
            let c = count(season)?;
            if left <= c {
                return Some((season, left));
            }
            left -= c;
            season += 1;
        }
    };
    let absolute = |n: i64| -> Option<(i64, i64)> {
        let mut left = n;
        for s in &regular {
            let c = s.episode_count?;
            if left <= c {
                return Some((s.season_number, left));
            }
            left -= c;
        }
        None
    };

    let mapped: Option<Vec<(i64, i64)>> = episodes
        .iter()
        .map(|&e| {
            let e = e as i64;
            if e < 1 {
                return None;
            }
            match season {
                Some(1) => in_season(1, e).or_else(|| absolute(e)),
                Some(s) => in_season(s as i64, e),
                None => absolute(e),
            }
        })
        .collect();
    match mapped {
        Some(eps) if !eps.is_empty() => Resolution::Episodes { tv, episodes: eps },
        _ => Resolution::Unmatched(format!(
            "\"{}\" has no {}episode {}",
            details.name,
            season.map(|s| format!("season {s} ")).unwrap_or_default(),
            episodes.iter().map(|e| e.to_string()).collect::<Vec<_>>().join(", ")
        )),
    }
}

async fn apply(ctx: &Ctx, file_id: i64, resolution: Resolution) -> Result<bool> {
    match resolution {
        Resolution::Movie(tmdb_id) => {
            let movie_id = store_movie(ctx, tmdb_id).await?;
            sqlx::query("DELETE FROM file_episodes WHERE file_id = ?").bind(file_id).execute(&ctx.db).await?;
            sqlx::query("UPDATE files SET kind = 'movie', movie_id = ?, match_note = NULL WHERE id = ?")
                .bind(movie_id).bind(file_id)
                .execute(&ctx.db).await?;
            Ok(true)
        }
        Resolution::Episodes { tv, episodes } => {
            let ids = store_episodes(ctx, tv, &episodes).await?;
            let mut tx = ctx.db.begin().await?;
            sqlx::query("DELETE FROM file_episodes WHERE file_id = ?").bind(file_id).execute(&mut *tx).await?;
            for eid in ids {
                sqlx::query("INSERT OR IGNORE INTO file_episodes (file_id, episode_id) VALUES (?, ?)")
                    .bind(file_id).bind(eid)
                    .execute(&mut *tx).await?;
            }
            sqlx::query("UPDATE files SET kind = 'episode', movie_id = NULL, match_note = NULL WHERE id = ?")
                .bind(file_id)
                .execute(&mut *tx).await?;
            tx.commit().await?;
            Ok(true)
        }
        Resolution::Unmatched(note) => {
            sqlx::query("DELETE FROM file_episodes WHERE file_id = ?").bind(file_id).execute(&ctx.db).await?;
            sqlx::query("UPDATE files SET kind = 'unmatched', movie_id = NULL, match_note = ? WHERE id = ?")
                .bind(note).bind(file_id)
                .execute(&ctx.db).await?;
            Ok(false)
        }
    }
}

fn genre_names(g: &[crate::tmdb::Genre]) -> Vec<String> {
    g.iter().map(|g| g.name.clone()).collect()
}

async fn store_movie(ctx: &Ctx, tmdb_id: i64) -> Result<i64> {
    if let Some((id,)) = sqlx::query_as::<_, (i64,)>("SELECT id FROM movies WHERE tmdb_id = ?").bind(tmdb_id).fetch_optional(&ctx.db).await? {
        return Ok(id);
    }
    let m = ctx.tmdb.as_ref().unwrap().movie(tmdb_id).await?;
    sqlx::query(
        "INSERT INTO movies (tmdb_id, title, original_title, year, overview, tagline, poster_path, backdrop_path, runtime, rating, genres, release_date, cast_json)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (tmdb_id) DO NOTHING",
    )
    .bind(m.id).bind(&m.title).bind(&m.original_title).bind(crate::tmdb::year_of(&m.release_date))
    .bind(&m.overview).bind(m.tagline.as_deref().filter(|t| !t.is_empty())).bind(&m.poster_path).bind(&m.backdrop_path)
    .bind(m.runtime).bind(m.vote_average).bind(Json(genre_names(&m.genres))).bind(&m.release_date)
    .bind(Json(m.credits.cast.iter().take(20).collect::<Vec<_>>()))
    .execute(&ctx.db)
    .await?;
    let (id,): (i64,) = sqlx::query_as("SELECT id FROM movies WHERE tmdb_id = ?").bind(tmdb_id).fetch_one(&ctx.db).await?;
    Ok(id)
}

async fn store_episodes(ctx: &Ctx, tv_id: i64, episodes: &[(i64, i64)]) -> Result<Vec<i64>> {
    let tmdb = ctx.tmdb.as_ref().unwrap();
    let show_id = match sqlx::query_as::<_, (i64,)>("SELECT id FROM shows WHERE tmdb_id = ?").bind(tv_id).fetch_optional(&ctx.db).await? {
        Some((id,)) => id,
        None => {
            let t = tmdb.tv(tv_id).await?;
            sqlx::query(
                "INSERT INTO shows (tmdb_id, title, original_title, year, overview, poster_path, backdrop_path, rating, genres, first_air_date, status, cast_json)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (tmdb_id) DO NOTHING",
            )
            .bind(t.id).bind(&t.name).bind(&t.original_name).bind(crate::tmdb::year_of(&t.first_air_date))
            .bind(&t.overview).bind(&t.poster_path).bind(&t.backdrop_path).bind(t.vote_average)
            .bind(Json(genre_names(&t.genres))).bind(&t.first_air_date).bind(&t.status)
            .bind(Json(t.credits.cast.iter().take(20).collect::<Vec<_>>()))
            .execute(&ctx.db)
            .await?;
            sqlx::query_as::<_, (i64,)>("SELECT id FROM shows WHERE tmdb_id = ?").bind(tv_id).fetch_one(&ctx.db).await?.0
        }
    };

    let seasons: HashSet<i64> = episodes.iter().map(|(s, _)| *s).collect();
    for season in seasons {
        let known: Option<(i64,)> = sqlx::query_as("SELECT id FROM seasons WHERE show_id = ? AND season_number = ?")
            .bind(show_id).bind(season)
            .fetch_optional(&ctx.db).await?;
        if known.is_some() {
            continue;
        }
        let s = tmdb.season(tv_id, season).await?;
        let mut tx = ctx.db.begin().await?;
        sqlx::query(
            "INSERT INTO seasons (show_id, season_number, name, overview, poster_path, air_date, episode_count)
             VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT (show_id, season_number) DO NOTHING",
        )
        .bind(show_id).bind(s.season_number).bind(&s.name).bind(&s.overview).bind(&s.poster_path).bind(&s.air_date)
        .bind(s.episodes.len() as i64)
        .execute(&mut *tx).await?;
        for e in &s.episodes {
            sqlx::query(
                "INSERT INTO episodes (show_id, season_number, episode_number, title, overview, still_path, air_date, runtime, rating)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT (show_id, season_number, episode_number) DO UPDATE SET
                   title = excluded.title, overview = excluded.overview, still_path = excluded.still_path,
                   air_date = excluded.air_date, runtime = excluded.runtime, rating = excluded.rating",
            )
            .bind(show_id).bind(e.season_number).bind(e.episode_number).bind(&e.name).bind(&e.overview)
            .bind(&e.still_path).bind(&e.air_date).bind(e.runtime).bind(e.vote_average)
            .execute(&mut *tx).await?;
        }
        tx.commit().await?;
    }

    let mut ids = vec![];
    for (season, episode) in episodes {
        let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM episodes WHERE show_id = ? AND season_number = ? AND episode_number = ?")
            .bind(show_id).bind(season).bind(episode)
            .fetch_optional(&ctx.db).await?;
        let id = match row {
            Some((id,)) => id,
            None => {
                sqlx::query("INSERT INTO episodes (show_id, season_number, episode_number) VALUES (?, ?, ?) ON CONFLICT DO NOTHING")
                    .bind(show_id).bind(season).bind(episode)
                    .execute(&ctx.db).await?;
                sqlx::query_as::<_, (i64,)>("SELECT id FROM episodes WHERE show_id = ? AND season_number = ? AND episode_number = ?")
                    .bind(show_id).bind(season).bind(episode)
                    .fetch_one(&ctx.db).await?.0
            }
        };
        ids.push(id);
    }
    Ok(ids)
}

pub async fn manual_match(ctx: &Arc<Ctx>, file_id: i64, kind: &str, tmdb_id: i64, season: Option<i64>, episode: Option<i64>) -> Result<bool> {
    let (path,): (String,) = sqlx::query_as("SELECT path FROM files WHERE id = ?").bind(file_id).fetch_one(&ctx.db).await?;
    sqlx::query(
        "INSERT INTO manual_matches (path, kind, tmdb_id, season, episode) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (path) DO UPDATE SET kind = excluded.kind, tmdb_id = excluded.tmdb_id, season = excluded.season, episode = excluded.episode",
    )
    .bind(&path).bind(kind).bind(tmdb_id).bind(season).bind(episode)
    .execute(&ctx.db)
    .await?;
    let resolution = if kind == "movie" {
        Resolution::Movie(tmdb_id)
    } else {
        Resolution::Episodes { tv: tmdb_id, episodes: vec![(season.unwrap_or(1), episode.unwrap_or(1))] }
    };
    apply(ctx, file_id, resolution).await
}
