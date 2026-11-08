//! Download & watch: plugins report files that are still downloading (in order) together with the byte ranges already
//! on disk; clients can play them through a stream that waits for data that hasn't arrived yet.

use crate::{
    api::{ApiError, Shared},
    auth::Identity,
    parser::{self, Guess},
    probe::{self, MediaInfo},
};
use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{header, HeaderValue, StatusCode},
    response::Response,
    routing::{get, post},
    Extension, Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    sync::RwLock,
};

/// Bytes that must be on disk from the start before playback is offered.
const PLAYABLE_PREFIX: u64 = 48 << 20;
/// How long a stream waits for missing data before giving up.
const STALL_LIMIT: Duration = Duration::from_secs(180);
const FORGET_AFTER: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct Download {
    pub id: String,
    pub plugin: String,
    pub title: String,
    pub path: PathBuf,
    pub size: u64,
    /// Half-open byte ranges of the file that are complete, sorted.
    pub ranges: Vec<(u64, u64)>,
    pub progress: f64,
    seen: Instant,
}

impl Download {
    fn prefix(&self) -> u64 {
        self.ranges.first().filter(|r| r.0 == 0).map_or(0, |r| r.1).min(self.size)
    }

    /// Bytes available starting at `pos`.
    fn available_from(&self, pos: u64) -> u64 {
        self.ranges.iter().find(|(s, e)| *s <= pos && pos < *e).map_or(0, |(_, e)| e - pos)
    }
}

#[derive(Default)]
pub struct Store {
    items: RwLock<HashMap<String, Download>>,
    probes: RwLock<HashMap<String, MediaInfo>>,
}

impl Store {
    async fn get(&self, id: &str) -> Option<Download> {
        let mut items = self.items.write().await;
        items.retain(|_, d| d.seen.elapsed() < FORGET_AFTER);
        items.get(id).cloned()
    }

    async fn all(&self) -> Vec<Download> {
        let mut items = self.items.write().await;
        items.retain(|_, d| d.seen.elapsed() < FORGET_AFTER);
        let mut v: Vec<Download> = items.values().cloned().collect();
        v.sort_by(|a, b| a.title.cmp(&b.title));
        v
    }
}

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/downloads", get(list))
        .route("/downloads/report", post(report))
        .route("/downloads/{id}", get(detail))
        .route("/downloads/{id}/stream", get(stream))
        .route("/downloads/{id}/transcode", post(transcode))
}

#[derive(Deserialize)]
struct ReportItem {
    id: String,
    title: Option<String>,
    path: String,
    size: u64,
    #[serde(default)]
    ranges: Vec<(u64, u64)>,
    #[serde(default)]
    progress: f64,
}

#[derive(Deserialize)]
struct ReportBody {
    plugin: String,
    items: Vec<ReportItem>,
}

/// Replaces everything this plugin reported before.
async fn report(State(ctx): State<Shared>, Extension(id): Extension<Identity>, Json(b): Json<ReportBody>) -> Result<StatusCode, ApiError> {
    if !matches!(id, Identity::Service) {
        return Err(ApiError(StatusCode::FORBIDDEN, "only plugins report downloads".into()));
    }
    let mut accepted = vec![];
    for item in b.items {
        let path = PathBuf::from(&item.path);
        let inside = ctx.cfg.media_dirs.iter().any(|m| path.starts_with(m));
        let clean = !path.components().any(|c| matches!(c, std::path::Component::ParentDir));
        let valid_id = !item.id.is_empty() && item.id.len() <= 80 && item.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !(inside && clean && valid_id) || item.size == 0 {
            continue;
        }
        let mut ranges: Vec<(u64, u64)> = item.ranges.into_iter().filter(|(s, e)| s < e).map(|(s, e)| (s, e.min(item.size))).collect();
        ranges.sort();
        let mut merged: Vec<(u64, u64)> = vec![];
        for r in ranges {
            match merged.last_mut() {
                Some(last) if r.0 <= last.1 => last.1 = last.1.max(r.1),
                _ => merged.push(r),
            }
        }
        accepted.push(Download {
            title: item.title.unwrap_or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
            id: item.id,
            plugin: b.plugin.clone(),
            path,
            size: item.size,
            ranges: merged,
            progress: item.progress.clamp(0.0, 1.0),
            seen: Instant::now(),
        });
    }
    let mut items = ctx.downloads.items.write().await;
    items.retain(|_, d| d.plugin != b.plugin);
    for d in accepted {
        items.insert(d.id.clone(), d);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// The name a download will have once complete (qBittorrent appends `.!qB` while downloading).
fn final_name(path: &std::path::Path) -> PathBuf {
    let s = path.to_string_lossy();
    PathBuf::from(s.strip_suffix(".!qB").unwrap_or(&s).to_string())
}

fn label(d: &Download) -> (String, Option<String>) {
    match parser::parse(&final_name(&d.path)) {
        Guess::Movie { title, year } => (match year {
            Some(y) => format!("{title} ({y})"),
            None => title,
        }, None),
        Guess::Episode { show, season, episodes, .. } => {
            let ep = match (season, episodes.first()) {
                (Some(s), Some(e)) => Some(format!("S{s:02}E{e:02}")),
                (None, Some(e)) => Some(format!("E{e:02}")),
                _ => None,
            };
            (show, ep)
        }
        Guess::DatedEpisode { show, date } => (show, Some(date)),
    }
}

async fn media(ctx: &Shared, d: &Download) -> Option<MediaInfo> {
    if let Some(m) = ctx.downloads.probes.read().await.get(&d.id) {
        return Some(m.clone());
    }
    if d.prefix() < (8 << 20).min(d.size) {
        return None;
    }
    let path = if d.path.exists() { d.path.clone() } else { final_name(&d.path) };
    let probed = tokio::time::timeout(Duration::from_secs(20), probe::probe(&ctx.cfg.ffprobe, &path)).await.ok()?.ok()?;
    probed.duration.filter(|x| *x > 0.0)?;
    ctx.downloads.probes.write().await.insert(d.id.clone(), probed.clone());
    Some(probed)
}

async fn describe(ctx: &Shared, d: &Download, with_probe: bool) -> Value {
    let (title, subtitle) = label(d);
    let m = media(ctx, d).await;
    let duration = m.as_ref().and_then(|m| m.duration);
    let fraction = d.prefix() as f64 / d.size as f64;
    let mut v = json!({
        "id": d.id,
        "title": title,
        "subtitle": subtitle,
        "name": d.title,
        "size": d.size,
        "progress": d.progress,
        "available_fraction": fraction,
        "available_secs": duration.map(|x| x * fraction),
        "duration": duration,
        "playable": m.is_some() && d.prefix() >= PLAYABLE_PREFIX.min(d.size),
        "path": final_name(&d.path).to_string_lossy(),
    });
    if with_probe {
        v["probe"] = serde_json::to_value(&m).unwrap_or(Value::Null);
    }
    v
}

async fn list(State(ctx): State<Shared>) -> Json<Vec<Value>> {
    let mut out = vec![];
    for d in ctx.downloads.all().await {
        out.push(describe(&ctx, &d, false).await);
    }
    Json(out)
}

async fn detail(State(ctx): State<Shared>, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    let d = ctx.downloads.get(&id).await.ok_or_else(crate::api::not_found)?;
    Ok(Json(describe(&ctx, &d, true).await))
}

fn parse_range(req: &Request, size: u64) -> Option<(u64, u64)> {
    let v = req.headers().get(header::RANGE)?.to_str().ok()?.strip_prefix("bytes=")?;
    let (a, b) = v.split_once('-')?;
    if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        return Some((size.saturating_sub(n), size - 1));
    }
    let start: u64 = a.parse().ok()?;
    let end = if b.is_empty() { size - 1 } else { b.parse::<u64>().ok()?.min(size - 1) };
    (start <= end).then_some((start, end))
}

/// Serves the file as it grows: bytes that aren't downloaded yet are waited for, so a player (or ffmpeg) reading
/// in order simply sees a slow source.
async fn stream(State(ctx): State<Shared>, Path(id): Path<String>, req: Request) -> Result<Response, ApiError> {
    let d = ctx.downloads.get(&id).await.ok_or_else(crate::api::not_found)?;
    let size = d.size;
    let range = parse_range(&req, size);
    let (start, end) = range.unwrap_or((0, size - 1));
    if start >= size {
        return Err(ApiError(StatusCode::RANGE_NOT_SATISFIABLE, "range outside the file".into()));
    }
    let file = match tokio::fs::File::open(&d.path).await {
        Ok(f) => f,
        Err(_) => tokio::fs::File::open(final_name(&d.path)).await.map_err(|_| crate::api::not_found())?,
    };
    let store_ctx = ctx.clone();
    let body = futures::stream::unfold((file, start, end, Instant::now(), store_ctx, id.clone()), |(mut file, pos, end, mut last, ctx, id)| async move {
        if pos > end {
            return None;
        }
        loop {
            let available = match ctx.downloads.get(&id).await {
                Some(d) => d.available_from(pos),
                None => end + 1 - pos,
            };
            if available > 0 {
                let n = available.min(end + 1 - pos).min(512 * 1024) as usize;
                let mut buf = vec![0u8; n];
                if file.seek(std::io::SeekFrom::Start(pos)).await.is_err() || file.read_exact(&mut buf).await.is_err() {
                    return Some((Err(std::io::Error::other("read failed")), (file, end + 1, end, last, ctx, id)));
                }
                last = Instant::now();
                return Some((Ok::<_, std::io::Error>(axum::body::Bytes::from(buf)), (file, pos + n as u64, end, last, ctx, id)));
            }
            if last.elapsed() > STALL_LIMIT {
                return Some((Err(std::io::Error::other("download stalled")), (file, end + 1, end, last, ctx, id)));
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    });
    let name = final_name(&d.path);
    let mime = match name.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("mp4" | "m4v") => "video/mp4",
        Some("mkv") => "video/x-matroska",
        Some("webm") => "video/webm",
        _ => "application/octet-stream",
    };
    let mut resp = Response::new(Body::from_stream(body));
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

async fn transcode(
    State(ctx): State<Shared>,
    Path(id): Path<String>,
    Json(request): Json<crate::transcode::Request>,
) -> Result<Json<Value>, ApiError> {
    let d = ctx.downloads.get(&id).await.ok_or_else(crate::api::not_found)?;
    let m = media(&ctx, &d).await.ok_or_else(|| ApiError(StatusCode::CONFLICT, "not enough of the file has downloaded yet".into()))?;
    let path = d.path.clone();
    let duration = m.duration.unwrap_or(0.0);
    // The container index sits at the end, which in-order downloads fetch first (first/last pieces priority).
    let keyframes = tokio::task::spawn_blocking(move || crate::index::keyframes(&path)).await.ok().flatten().filter(|k| crate::index::usable(k, duration));
    let input = format!("http://127.0.0.1:{}/api/downloads/{id}/stream?token={}", ctx.cfg.port, ctx.internal_token);
    let s = ctx
        .transcoder
        .start_with(0, PathBuf::from(input), m, request, crate::transcode::KeyframeSource::Known(keyframes))
        .await?;
    Ok(Json(json!({ "session": s.id, "playlist": format!("/api/transcode/{}/index.m3u8", s.id), "plan": s.plan })))
}
