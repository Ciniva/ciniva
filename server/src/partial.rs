//! Files that are still downloading. A plugin reports which byte ranges are on disk; library streams wait for the
//! rest instead of serving holes, and the scanner and background jobs leave these files alone until they finish.

use crate::{
    api::{ApiError, Shared},
    auth::Identity,
};
use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::Response,
    routing::post,
    Extension, Json, Router,
};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    sync::RwLock,
};

/// How long a stream waits for missing data before giving up.
const STALL_LIMIT: Duration = Duration::from_secs(180);
const FORGET_AFTER: Duration = Duration::from_secs(60);
const CHUNK: u64 = 512 * 1024;

#[derive(Debug, Clone)]
struct Partial {
    plugin: String,
    size: u64,
    /// Half-open byte ranges that are complete, sorted and merged.
    ranges: Vec<(u64, u64)>,
    seen: Instant,
}

impl Partial {
    fn available_from(&self, pos: u64) -> u64 {
        self.ranges.iter().find(|(s, e)| *s <= pos && pos < *e).map_or(0, |(_, e)| e - pos)
    }
}

#[derive(Default)]
pub struct Store {
    items: RwLock<HashMap<PathBuf, Partial>>,
}

impl Store {
    async fn get(&self, path: &Path) -> Option<Partial> {
        let items = self.items.read().await;
        items.get(path).filter(|p| p.seen.elapsed() < FORGET_AFTER).cloned()
    }

    pub async fn contains(&self, path: &Path) -> bool {
        self.get(path).await.is_some()
    }

    pub async fn paths(&self) -> HashSet<PathBuf> {
        let items = self.items.read().await;
        items.iter().filter(|(_, p)| p.seen.elapsed() < FORGET_AFTER).map(|(k, _)| k.clone()).collect()
    }
}

pub fn routes() -> Router<Shared> {
    Router::new().route("/downloads/report", post(report))
}

#[derive(Deserialize)]
struct ReportItem {
    path: String,
    size: u64,
    #[serde(default)]
    ranges: Vec<(u64, u64)>,
}

#[derive(Deserialize)]
struct ReportBody {
    plugin: String,
    items: Vec<ReportItem>,
}

/// The name a download will have once complete (qBittorrent can append `.!qB` while downloading).
fn final_name(path: &str) -> PathBuf {
    PathBuf::from(path.strip_suffix(".!qB").unwrap_or(path))
}

fn merge(size: u64, ranges: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    let mut ranges: Vec<(u64, u64)> = ranges.into_iter().map(|(s, e)| (s, e.min(size))).filter(|(s, e)| s < e).collect();
    ranges.sort();
    let mut merged: Vec<(u64, u64)> = vec![];
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.0 <= last.1 => last.1 = last.1.max(r.1),
            _ => merged.push(r),
        }
    }
    merged
}

/// Replaces everything this plugin reported before. Files the library doesn't know yet get their folder scanned, so
/// a new download shows up right away instead of at the next periodic scan.
async fn report(State(ctx): State<Shared>, Extension(id): Extension<Identity>, Json(b): Json<ReportBody>) -> Result<StatusCode, ApiError> {
    if !matches!(id, Identity::Service) {
        return Err(ApiError(StatusCode::FORBIDDEN, "only plugins report downloads".into()));
    }
    let mut accepted = vec![];
    for item in b.items {
        let path = final_name(&item.path);
        let inside = ctx.cfg.media_dirs.iter().any(|m| path.starts_with(m));
        let clean = !path.components().any(|c| matches!(c, std::path::Component::ParentDir));
        if inside && clean && item.size > 0 {
            accepted.push((path, Partial { plugin: b.plugin.clone(), size: item.size, ranges: merge(item.size, item.ranges), seen: Instant::now() }));
        }
    }
    let new: Vec<PathBuf> = {
        let mut items = ctx.partial.items.write().await;
        items.retain(|_, p| p.plugin != b.plugin && p.seen.elapsed() < FORGET_AFTER);
        let mut new = vec![];
        for (path, p) in accepted {
            if items.insert(path.clone(), p).is_none() {
                new.push(path);
            }
        }
        new
    };
    if !new.is_empty() {
        let known: HashSet<String> = sqlx::query_scalar::<_, String>("SELECT path FROM files").fetch_all(&ctx.db).await?.into_iter().collect();
        let unknown: Vec<PathBuf> = new.into_iter().filter(|p| !known.contains(p.to_string_lossy().as_ref())).collect();
        if !unknown.is_empty() {
            let ctx = ctx.clone();
            tokio::spawn(async move {
                if let Err(e) = crate::library::scan_paths(&ctx, unknown).await {
                    tracing::warn!("scan of new downloads failed: {e:#}");
                }
            });
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Bytes `start..=end` of a file that is still downloading, waiting for data that hasn't arrived yet, so a player (or
/// ffmpeg) reading in order simply sees a slow source. Once the file is no longer reported it's read as is.
fn waiting_body(ctx: Shared, path: PathBuf, file: tokio::fs::File, start: u64, end: u64) -> axum::body::Body {
    let state = (file, start, Instant::now());
    let body = futures::stream::unfold(state, move |(mut file, pos, mut last)| {
        let (ctx, path) = (ctx.clone(), path.clone());
        async move {
            if pos > end {
                return None;
            }
            loop {
                let available = match ctx.partial.get(&path).await {
                    Some(p) => p.available_from(pos),
                    None => end + 1 - pos,
                };
                if available > 0 {
                    let n = available.min(end + 1 - pos).min(CHUNK) as usize;
                    let mut buf = vec![0u8; n];
                    if file.seek(std::io::SeekFrom::Start(pos)).await.is_err() || file.read_exact(&mut buf).await.is_err() {
                        return Some((Err(std::io::Error::other("read failed")), (file, end + 1, last)));
                    }
                    last = Instant::now();
                    return Some((Ok::<_, std::io::Error>(axum::body::Bytes::from(buf)), (file, pos + n as u64, last)));
                }
                if last.elapsed() > STALL_LIMIT {
                    return Some((Err(std::io::Error::other("download stalled")), (file, end + 1, last)));
                }
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
        }
    });
    axum::body::Body::from_stream(body)
}

fn parse_range(headers: &HeaderMap, size: u64) -> Option<(u64, u64)> {
    let v = headers.get(header::RANGE)?.to_str().ok()?.strip_prefix("bytes=")?;
    let (a, b) = v.split_once('-')?;
    if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        return Some((size.saturating_sub(n), size - 1));
    }
    let start: u64 = a.parse().ok()?;
    let end = if b.is_empty() { size - 1 } else { b.parse::<u64>().ok()?.min(size - 1) };
    (start <= end).then_some((start, end))
}

/// The library stream for a file that is still downloading, or `None` when it is complete.
pub async fn serve(ctx: &Shared, path: &Path, mime: &'static str, headers: &HeaderMap) -> Option<Result<Response, ApiError>> {
    let size = ctx.partial.get(path).await?.size;
    Some(
        async {
            let range = parse_range(headers, size);
            let (start, end) = range.unwrap_or((0, size - 1));
            if start >= size {
                return Err(ApiError(StatusCode::RANGE_NOT_SATISFIABLE, "range outside the file".into()));
            }
            let file = tokio::fs::File::open(path).await.map_err(|_| crate::api::not_found())?;
            let mut resp = Response::new(waiting_body(ctx.clone(), path.to_path_buf(), file, start, end));
            let h = resp.headers_mut();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
            h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            h.insert(header::CONTENT_LENGTH, HeaderValue::from(end - start + 1));
            if range.is_some() {
                h.insert(header::CONTENT_RANGE, HeaderValue::from_str(&format!("bytes {start}-{end}/{size}")).unwrap());
                *resp.status_mut() = StatusCode::PARTIAL_CONTENT;
            }
            Ok(resp)
        }
        .await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_and_clamps_ranges() {
        assert_eq!(merge(100, vec![(50, 60), (0, 10), (10, 20), (55, 200), (90, 90)]), vec![(0, 20), (50, 100)]);
    }

    #[test]
    fn available_only_inside_a_range() {
        let p = Partial { plugin: "q".into(), size: 100, ranges: vec![(0, 20), (50, 100)], seen: Instant::now() };
        assert_eq!(p.available_from(5), 15);
        assert_eq!(p.available_from(20), 0);
        assert_eq!(p.available_from(60), 40);
    }

    #[test]
    fn parses_ranges() {
        let h = |v: &str| {
            let mut m = HeaderMap::new();
            m.insert(header::RANGE, HeaderValue::from_str(v).unwrap());
            m
        };
        assert_eq!(parse_range(&h("bytes=10-"), 100), Some((10, 99)));
        assert_eq!(parse_range(&h("bytes=-10"), 100), Some((90, 99)));
        assert_eq!(parse_range(&h("bytes=0-500"), 100), Some((0, 99)));
        assert_eq!(parse_range(&HeaderMap::new(), 100), None);
    }

    #[test]
    fn strips_qbittorrent_suffix() {
        assert_eq!(final_name("/media/Downloads/a.mkv.!qB"), PathBuf::from("/media/Downloads/a.mkv"));
        assert_eq!(final_name("/media/Downloads/a.mkv"), PathBuf::from("/media/Downloads/a.mkv"));
    }
}
