//! Live streams from plugins (IPTV): ffmpeg pulls an upstream URL into a rolling HLS playlist on disk. Viewers asking
//! for the same stream with the same plan share one session, so a provider sees one connection per channel.

use crate::{
    api::{ApiError, Shared},
    auth::Identity,
    hwaccel::{self, Api, DecodeCodec, EncodeCodec, Hardware, Lease, Purpose},
    probe::MediaInfo,
    transcode::{Plan, Request, TargetCodec},
};
use anyhow::{bail, Result};
use axum::{
    extract::{Path, RawQuery, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Extension, Json, Router,
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{process::Child, sync::Mutex};

const SEGMENT_SECS: u32 = 4;
const IDLE: Duration = Duration::from_secs(45);

/// What a plugin answers for `GET ui/streams/{id}`. The URL and headers stay on the server.
#[derive(Debug, Clone, Deserialize)]
pub struct Source {
    pub url: String,
    #[serde(default)]
    pub title: String,
    pub image: Option<String>,
    pub user_agent: Option<String>,
    pub referrer: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

pub struct LiveSession {
    pub id: String,
    key: String,
    pub plugin: String,
    pub stream: String,
    pub title: String,
    pub image: Option<String>,
    dir: PathBuf,
    pub plan: Plan,
    child: Mutex<Child>,
    _lease: Lease,
    last_access: Mutex<Instant>,
}

pub struct Live {
    hw: Arc<Hardware>,
    root: PathBuf,
    ffprobe: String,
    sessions: Mutex<HashMap<String, Arc<LiveSession>>>,
    starting: Mutex<()>,
}

impl Live {
    pub fn new(hw: Arc<Hardware>, root: PathBuf, ffprobe: String) -> Arc<Self> {
        let _ = std::fs::remove_dir_all(&root);
        let live = Arc::new(Self { hw, root, ffprobe, sessions: Default::default(), starting: Mutex::new(()) });
        let reaper = live.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                reaper.reap().await;
            }
        });
        live
    }

    async fn reap(&self) {
        let mut stale = vec![];
        for (id, s) in self.sessions.lock().await.iter() {
            if s.last_access.lock().await.elapsed() > IDLE {
                stale.push(id.clone());
            }
        }
        for id in stale {
            self.stop(&id).await;
        }
    }

    async fn stop(&self, id: &str) {
        if let Some(s) = self.sessions.lock().await.remove(id) {
            let _ = s.child.lock().await.kill().await;
            let _ = tokio::fs::remove_dir_all(&s.dir).await;
            tracing::info!(session = id, stream = %s.key, "live stream stopped");
        }
    }

    pub async fn get(&self, id: &str) -> Option<Arc<LiveSession>> {
        let s = self.sessions.lock().await.get(id).cloned()?;
        *s.last_access.lock().await = Instant::now();
        Some(s)
    }

    pub async fn active(&self) -> Vec<serde_json::Value> {
        self.sessions
            .lock()
            .await
            .values()
            .map(|s| serde_json::json!({ "id": s.id, "plugin": s.plugin, "stream": s.stream, "title": s.title, "plan": s.plan }))
            .collect()
    }

    pub async fn start(&self, plugin: &str, stream: &str, source: Source, request: Request) -> Result<Arc<LiveSession>> {
        let input = input_args(&source);
        let mut probe_args = input.clone();
        probe_args.extend(["-analyzeduration".into(), "5000000".into()]);
        let media = match tokio::time::timeout(
            Duration::from_secs(25),
            crate::probe::probe_input(&self.ffprobe, &probe_args, std::ffi::OsStr::new(&source.url)),
        )
        .await
        {
            Ok(Ok(m)) => m,
            Ok(Err(e)) => bail!("the channel isn't responding: {}", short(&e.to_string())),
            Err(_) => bail!("the channel didn't answer within 25 seconds"),
        };
        let Some(video) = media.video.clone() else { bail!("the channel has no video") };

        let copy = request.video_copy.as_deref().is_some_and(|l| l.split(',').any(|c| c.trim() == video.codec))
            && crate::transcode::REMUX_CODECS.contains(&video.codec.as_str())
            && video.height as u32 <= request.max_height;
        let container = container(&request, if copy { &video.codec } else { "h264" });
        let audio_copy = audio_copyable(&media, &request);
        let key = format!("{plugin}/{stream}/{}/{}/{}/{}", if copy { "copy" } else { "h264" }, request.max_height, container, audio_copy);
        let _one_at_a_time = self.starting.lock().await;

        {
            let sessions = self.sessions.lock().await;
            if let Some(s) = sessions.values().find(|s| s.key == key) {
                if s.child.lock().await.try_wait()?.is_none() {
                    *s.last_access.lock().await = Instant::now();
                    return Ok(s.clone());
                }
            }
        }
        let dead: Vec<String> = self.sessions.lock().await.values().filter(|s| s.key == key).map(|s| s.id.clone()).collect();
        for id in dead {
            self.stop(&id).await;
        }

        let lease = if copy {
            self.hw.copy_lease()
        } else {
            self.hw.acquire(DecodeCodec::from_probe(&video.codec, video.bit_depth), EncodeCodec::H264, Purpose::Stream, false)
        };
        let codec = if copy {
            match video.codec.as_str() {
                "hevc" => TargetCodec::Hevc,
                "av1" => TargetCodec::Av1,
                _ => TargetCodec::H264,
            }
        } else {
            TargetCodec::H264
        };
        let plan = Plan {
            device: if copy { format!("Live ({} copied)", video.codec) } else { lease.device.label() },
            codec,
            container,
            hw_decode: false,
            encoder: if copy { "copy" } else { hwaccel::encoder_name(lease.device.api, EncodeCodec::H264) },
            tonemap: None,
            width_limit: request.max_height,
            hdr_out: false,
            remux: copy,
        };

        let id = format!("{:016x}", rand::random::<u64>());
        let dir = self.root.join(&id);
        tokio::fs::create_dir_all(&dir).await?;
        let mut args: Vec<String> = vec!["-hide_banner".into(), "-nostdin".into(), "-v".into(), "warning".into(), "-y".into()];
        if !copy {
            args.extend(hwaccel::init_args(&lease.device));
        }
        args.extend(input);
        args.extend(["-fflags".into(), "+genpts+discardcorrupt".into(), "-i".into(), source.url.clone()]);
        if copy {
            args.extend(["-c:v".into(), "copy".into()]);
            if video.codec == "hevc" && container == "fmp4" {
                args.extend(["-tag:v".into(), "hvc1".into()]);
            }
        } else {
            args.extend(encode_args(&lease, &video, &request));
        }
        if audio_copy {
            args.extend(["-c:a".into(), "copy".into()]);
        } else {
            args.extend(["-c:a".into(), "aac".into(), "-ac".into(), "2".into(), "-b:a".into(), "192k".into()]);
        }
        args.extend(["-sn".into(), "-dn".into(), "-f".into(), "hls".into(), "-hls_time".into(), SEGMENT_SECS.to_string()]);
        args.extend(["-hls_list_size".into(), "8".into(), "-hls_delete_threshold".into(), "4".into()]);
        args.extend(["-hls_flags".into(), "delete_segments+independent_segments+temp_file+program_date_time".into()]);
        let ext = if container == "fmp4" {
            args.extend(["-hls_segment_type".into(), "fmp4".into(), "-hls_fmp4_init_filename".into(), "init.mp4".into()]);
            "m4s"
        } else {
            args.extend(["-hls_segment_type".into(), "mpegts".into()]);
            "ts"
        };
        args.extend([
            "-hls_segment_filename".into(),
            dir.join(format!("%d.{ext}")).display().to_string(),
            dir.join("live.m3u8").display().to_string(),
        ]);
        let child = tokio::process::Command::new(&self.hw.ffmpeg)
            .args(&args)
            .env("CUDA_DEVICE_ORDER", "PCI_BUS_ID")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::from(std::fs::File::create(dir.join("ffmpeg.log"))?))
            .kill_on_drop(true)
            .spawn()?;
        tracing::info!(session = %id, stream = %key, title = %source.title, ?plan, "live stream started");
        let session = Arc::new(LiveSession {
            id: id.clone(),
            key,
            plugin: plugin.to_string(),
            stream: stream.to_string(),
            title: source.title,
            image: source.image,
            dir,
            plan,
            child: Mutex::new(child),
            _lease: lease,
            last_access: Mutex::new(Instant::now()),
        });
        self.sessions.lock().await.insert(id, session.clone());
        Ok(session)
    }
}

impl LiveSession {
    /// The rolling playlist once ffmpeg has written a couple of segments; an error with ffmpeg's last words if it died.
    async fn playlist(&self) -> Result<String> {
        let path = self.dir.join("live.m3u8");
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(text) = tokio::fs::read_to_string(&path).await {
                if text.lines().filter(|l| !l.is_empty() && !l.starts_with('#')).count() >= 2 {
                    return Ok(text);
                }
            }
            if self.child.lock().await.try_wait()?.is_some() {
                let log = tokio::fs::read_to_string(self.dir.join("ffmpeg.log")).await.unwrap_or_default();
                bail!("the channel stopped: {}", short(log.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no output")));
            }
            if Instant::now() > deadline {
                bail!("the channel sent no video within 30 seconds");
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

fn short(s: &str) -> String {
    let s = s.trim();
    s.chars().take(200).collect()
}

/// ffmpeg input options for an HTTP(S) source: timeouts, reconnects and the headers the playlist asked for.
fn input_args(source: &Source) -> Vec<String> {
    let mut a: Vec<String> = vec!["-rw_timeout".into(), "15000000".into()];
    if source.url.starts_with("http") {
        a.extend(["-reconnect".into(), "1".into(), "-reconnect_streamed".into(), "1".into(), "-reconnect_delay_max".into(), "10".into()]);
    }
    if let Some(ua) = source.user_agent.as_deref().filter(|u| !u.is_empty()) {
        a.extend(["-user_agent".into(), ua.into()]);
    }
    let mut headers = String::new();
    if let Some(r) = source.referrer.as_deref().filter(|r| !r.is_empty()) {
        headers.push_str(&format!("Referer: {r}\r\n"));
    }
    for (k, v) in &source.headers {
        if !k.contains(['\r', '\n', ':']) && !v.contains(['\r', '\n']) {
            headers.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    if !headers.is_empty() {
        a.extend(["-headers".into(), headers]);
    }
    a
}

fn container(req: &Request, codec: &str) -> &'static str {
    match req.container.as_deref().map(|c| c.trim().to_ascii_lowercase()).as_deref() {
        Some("mp4" | "fmp4") => "fmp4",
        Some("ts" | "mpegts") if matches!(codec, "h264" | "hevc") => "mpegts",
        _ if codec == "h264" => "mpegts",
        _ => "fmp4",
    }
}

fn audio_copyable(media: &MediaInfo, req: &Request) -> bool {
    let Some(a) = media.audio.first() else { return true };
    let ok = match req.audio_copy.as_deref() {
        Some(list) => list.split(',').any(|x| x.trim() == a.codec),
        None => a.codec == "aac",
    };
    ok && a.channels.unwrap_or(2) as u32 <= req.audio_channels.max(2)
}

/// H.264 at the client's height and bitrate. Live channels are mostly SD/HD, so the CPU decodes and the GPU encodes.
fn encode_args(lease: &Lease, video: &crate::probe::VideoInfo, req: &Request) -> Vec<String> {
    let d = &lease.device;
    let h = req.max_height.min((video.height as u32).max(1));
    let mut vf = vec![format!("scale=w=-2:h={h}:flags=bicubic")];
    match d.api {
        Api::Nvenc | Api::Vaapi | Api::Qsv => vf.push(hwaccel::upload_filter(d.api, false)),
        _ => vf.push("format=yuv420p".into()),
    }
    let b = req.bitrate_kbps.clamp(1000, 20000);
    let mut a: Vec<String> = vec!["-vf".into(), vf.join(","), "-c:v".into(), hwaccel::encoder_name(d.api, EncodeCodec::H264).into()];
    a.extend(["-b:v".into(), format!("{b}k"), "-maxrate".into(), format!("{}k", b * 3 / 2), "-bufsize".into(), format!("{}k", b * 2)]);
    match d.api {
        Api::Nvenc => a.extend(["-preset".into(), "p4".into(), "-bf".into(), "0".into()]),
        Api::Qsv => a.extend(["-preset".into(), "veryfast".into()]),
        Api::Software => a.extend(["-preset".into(), "veryfast".into(), "-tune".into(), "zerolatency".into()]),
        _ => {}
    }
    a.extend(["-force_key_frames".into(), format!("expr:gte(t,n_forced*{SEGMENT_SECS})")]);
    a
}

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/plugins/{plugin}/live/{stream}", post(start))
        .route("/live/{sid}", delete(stop))
        .route("/live/{sid}/{file}", get(file))
}

fn valid(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

async fn start(
    State(ctx): State<Shared>,
    Extension(id): Extension<Identity>,
    Path((plugin, stream)): Path<(String, String)>,
    Json(request): Json<Request>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !valid(&plugin) || !valid(&stream) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "bad stream id".into()));
    }
    let source = crate::plugins::resolve_stream(&ctx, &plugin, &stream, &id).await?;
    let s = ctx.live.start(&plugin, &stream, source, request).await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, format!("{e:#}")))?;
    Ok(Json(serde_json::json!({
        "session": s.id,
        "playlist": format!("/api/live/{}/index.m3u8", s.id),
        "plan": s.plan,
        "live": true,
        "title": s.title,
        "image": s.image,
    })))
}

/// Sessions are shared and end on their own once nobody fetches them.
async fn stop() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn file(
    State(ctx): State<Shared>,
    Path((sid, file)): Path<(String, String)>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let not_found = || ApiError(StatusCode::NOT_FOUND, "not found".into());
    let s = ctx.live.get(&sid).await.ok_or_else(not_found)?;
    if file == "index.m3u8" {
        let text = s.playlist().await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, format!("{e:#}")))?;
        let token = query.as_deref().and_then(|q| q.split('&').find(|kv| kv.starts_with("token="))).map(|kv| format!("?{kv}")).unwrap_or_default();
        let body: String = text
            .lines()
            .map(|l| {
                if !token.is_empty() && !l.is_empty() && !l.starts_with('#') {
                    format!("{l}{token}\n")
                } else if !token.is_empty() && l.starts_with("#EXT-X-MAP:URI=\"") {
                    l.replacen(".mp4\"", &format!(".mp4{token}\""), 1) + "\n"
                } else {
                    format!("{l}\n")
                }
            })
            .collect();
        return Ok(([(header::CONTENT_TYPE, "application/vnd.apple.mpegurl"), (header::CACHE_CONTROL, "no-store")], body).into_response());
    }
    if !valid(&file) || file.starts_with('.') {
        return Err(not_found());
    }
    let ctype = match file.rsplit('.').next() {
        Some("ts") => "video/mp2t",
        Some("m4s" | "mp4") => "video/mp4",
        _ => return Err(not_found()),
    };
    let bytes = tokio::fs::read(s.dir.join(&file)).await.map_err(|_| not_found())?;
    Ok(crate::api::ranged(&headers, ctype, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(json: serde_json::Value) -> Source {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn input_headers() {
        let a = input_args(&source(serde_json::json!({
            "url": "https://x/a.m3u8", "user_agent": "VLC", "referrer": "https://site/", "headers": { "X-Key": "1", "Bad\r\nHeader": "x" }
        })));
        assert!(a.windows(2).any(|w| w[0] == "-user_agent" && w[1] == "VLC"));
        let h = a.iter().position(|x| x == "-headers").map(|i| a[i + 1].clone()).unwrap();
        assert_eq!(h, "Referer: https://site/\r\nX-Key: 1\r\n");
        assert!(a.contains(&"-reconnect".to_string()));
    }

    #[test]
    fn containers() {
        let req = |c: Option<&str>| -> Request { serde_json::from_value(serde_json::json!({ "container": c })).unwrap() };
        assert_eq!(container(&req(None), "h264"), "mpegts");
        assert_eq!(container(&req(None), "hevc"), "fmp4");
        assert_eq!(container(&req(Some("ts")), "hevc"), "mpegts");
        assert_eq!(container(&req(Some("fmp4")), "h264"), "fmp4");
    }
}
