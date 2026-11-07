use crate::{
    hwaccel::{self, Api, DecodeCodec, EncodeCodec, Hardware, Lease, Purpose, Vendor},
    probe::MediaInfo,
};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{process::Child, sync::Mutex};

pub const SEGMENT_SECS: f64 = 6.0;
const REMUX_LEAD: f64 = 1.0;
const AHEAD_PAUSE: u64 = 30;
const AHEAD_RESUME: u64 = 15;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Request {
    #[serde(default = "default_height")]
    pub max_height: u32,
    #[serde(default = "default_bitrate")]
    pub bitrate_kbps: u32,
    #[serde(default)]
    pub codec: TargetCodec,
    /// Keep HDR (HEVC 10-bit) instead of tone mapping to SDR.
    #[serde(default)]
    pub hdr: bool,
    pub audio_stream: Option<i64>,
    #[serde(default = "default_channels")]
    pub audio_channels: u32,
    /// Comma-separated audio codecs the client plays as-is; others are re-encoded to AAC.
    pub audio_copy: Option<String>,
    /// Comma-separated video codecs the client decodes; a matching source at full size is remuxed, not re-encoded.
    pub video_copy: Option<String>,
}

fn default_height() -> u32 {
    1080
}
fn default_bitrate() -> u32 {
    8000
}
fn default_channels() -> u32 {
    2
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TargetCodec {
    #[default]
    H264,
    Hevc,
    Av1,
}

/// Everything decided about how a stream is produced.
#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub device: String,
    pub codec: TargetCodec,
    pub container: &'static str,
    pub hw_decode: bool,
    pub encoder: &'static str,
    pub tonemap: Option<&'static str>,
    pub width_limit: u32,
    pub hdr_out: bool,
    pub remux: bool,
}

struct Running {
    child: Child,
    start_segment: u64,
    paused: bool,
}

impl Running {
    /// SIGSTOP/SIGCONT the encoder so it never runs more than a few minutes ahead of the player.
    fn throttle(&mut self, ahead: u64) {
        let pause = if self.paused { ahead > AHEAD_RESUME } else { ahead > AHEAD_PAUSE };
        if pause == self.paused {
            return;
        }
        if let Some(pid) = self.child.id() {
            unsafe { libc::kill(pid as i32, if pause { libc::SIGSTOP } else { libc::SIGCONT }) };
            self.paused = pause;
        }
    }
}

pub struct Session {
    pub id: String,
    pub file_id: i64,
    input: PathBuf,
    dir: PathBuf,
    duration: f64,
    bounds: Vec<f64>,
    request: Request,
    media: MediaInfo,
    lease: Lease,
    pub plan: Plan,
    running: Mutex<Option<Running>>,
    playhead: std::sync::atomic::AtomicU64,
    init: Mutex<Option<Arc<Vec<u8>>>>,
    last_access: Mutex<Instant>,
    ffmpeg: String,
    hdr_source: bool,
    filters: Arc<std::collections::HashSet<String>>,
}

pub struct Transcoder {
    hw: Arc<Hardware>,
    root: PathBuf,
    keyframes: Arc<Keyframes>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    filters: Arc<std::collections::HashSet<String>>,
}

/// HDR to SDR on the CPU: Jellyfin's SIMD `tonemapx` when present, otherwise the zscale chain.
pub fn cpu_tonemap_filter(filters: &std::collections::HashSet<String>) -> String {
    if filters.contains("tonemapx") {
        "tonemapx=tonemap=bt2390:desat=0:peak=100:t=bt709:m=bt709:p=bt709:format=yuv420p".into()
    } else {
        "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv".into()
    }
}

fn is_hdr(media: &MediaInfo) -> bool {
    media.video.as_ref().and_then(|v| v.hdr.as_deref()).is_some()
}

impl Transcoder {
    pub fn new(hw: Arc<Hardware>, root: PathBuf, keyframes: Arc<Keyframes>) -> Arc<Self> {
        let _ = std::fs::remove_dir_all(&root);
        let filters = Arc::new(hw.filters.clone());
        let t = Arc::new(Self { hw, root, keyframes, sessions: Mutex::new(HashMap::new()), filters });
        let reaper = t.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(15)).await;
                reaper.reap(Duration::from_secs(90)).await;
            }
        });
        t
    }

    async fn reap(&self, idle: Duration) {
        let mut stale = vec![];
        for (id, s) in self.sessions.lock().await.iter() {
            if s.last_access.lock().await.elapsed() > idle {
                stale.push(id.clone());
            }
        }
        for id in stale {
            self.stop(&id).await;
        }
    }

    pub async fn stop(&self, id: &str) {
        if let Some(s) = self.sessions.lock().await.remove(id) {
            if let Some(mut r) = s.running.lock().await.take() {
                let _ = r.child.kill().await;
            }
            let _ = tokio::fs::remove_dir_all(&s.dir).await;
            tracing::info!(session = id, "transcode stopped");
        }
    }

    pub async fn get(&self, id: &str) -> Option<Arc<Session>> {
        let s = self.sessions.lock().await.get(id).cloned()?;
        *s.last_access.lock().await = Instant::now();
        Some(s)
    }

    pub async fn active(&self) -> Vec<serde_json::Value> {
        self.sessions
            .lock()
            .await
            .values()
            .map(|s| serde_json::json!({ "id": s.id, "file_id": s.file_id, "plan": s.plan }))
            .collect()
    }

    pub async fn start(&self, file_id: i64, input: PathBuf, media: MediaInfo, request: Request) -> Result<Arc<Session>> {
        let Some(duration) = media.duration.filter(|d| *d > 0.0) else { bail!("unknown duration; probe the file first") };
        let remux = if remuxable(&media, &request) {
            match self.keyframes.quick(file_id, &input, duration).await {
                Some(kf) => Some(segment_bounds(&kf, duration)).filter(|b| b.len() > 1 || duration <= SEGMENT_SECS * 4.0),
                None => {
                    self.keyframes.spawn(file_id, input.clone(), duration);
                    None
                }
            }
        } else {
            None
        };
        if let Some(bounds) = remux {
            let source = media.video.as_ref().map(|v| v.codec.clone()).unwrap_or_default();
            let plan = Plan {
                device: format!("Remux ({source} copied)"),
                codec: match source.as_str() {
                    "hevc" => TargetCodec::Hevc,
                    "av1" => TargetCodec::Av1,
                    _ => TargetCodec::H264,
                },
                container: "fmp4",
                hw_decode: false,
                encoder: "copy",
                tonemap: None,
                width_limit: request.max_height,
                hdr_out: false,
                remux: true,
            };
            return self.register(file_id, input, media, request, duration, bounds, self.hw.cpu_lease(), plan).await;
        }
        let video = media.video.clone().unwrap_or_default();
        let source = DecodeCodec::from_probe(&video.codec, video.bit_depth);
        let hdr_source = is_hdr(&media);
        let hdr_out = request.hdr && hdr_source && request.codec == TargetCodec::Hevc;
        let mut request = request;
        // Prefer a weaker codec on a GPU over the requested codec in software (software AV1/HEVC is far too slow).
        let wanted = match (request.codec, hdr_out) {
            (TargetCodec::Hevc, true) => EncodeCodec::Hevc10,
            (TargetCodec::Hevc, false) => EncodeCodec::Hevc,
            (TargetCodec::Av1, _) => EncodeCodec::Av1,
            (TargetCodec::H264, _) => EncodeCodec::H264,
        };
        let fallbacks: &[(EncodeCodec, TargetCodec)] = match wanted {
            EncodeCodec::Av1 => &[(EncodeCodec::Av1, TargetCodec::Av1), (EncodeCodec::Hevc, TargetCodec::Hevc), (EncodeCodec::H264, TargetCodec::H264)],
            EncodeCodec::Hevc10 => &[(EncodeCodec::Hevc10, TargetCodec::Hevc), (EncodeCodec::H264, TargetCodec::H264)],
            EncodeCodec::Hevc => &[(EncodeCodec::Hevc, TargetCodec::Hevc), (EncodeCodec::H264, TargetCodec::H264)],
            EncodeCodec::H264 => &[(EncodeCodec::H264, TargetCodec::H264)],
        };
        let (target, codec) = fallbacks
            .iter()
            .copied()
            .find(|(c, _)| self.hw.can_encode_in_hardware(*c))
            .unwrap_or((wanted, request.codec));
        request.codec = codec;
        let hdr_out = hdr_out && target == EncodeCodec::Hevc10;
        let lease = self.hw.acquire(source, target, Purpose::Stream, hdr_source && !hdr_out);
        let d = &lease.device;
        let tonemap = if hdr_source && !hdr_out {
            Some(if d.hw_tonemap && lease.hw_decode {
                "gpu"
            } else if self.filters.contains("tonemapx") || self.filters.contains("zscale") {
                "cpu"
            } else {
                "none"
            })
        } else {
            None
        };
        let plan = Plan {
            device: d.label(),
            codec: request.codec,
            container: if request.codec == TargetCodec::H264 { "mpegts" } else { "fmp4" },
            hw_decode: lease.hw_decode,
            encoder: hwaccel::encoder_name(d.api, target),
            tonemap,
            width_limit: request.max_height,
            hdr_out,
            remux: false,
        };
        let bounds = (0..(duration / SEGMENT_SECS).ceil() as u64).map(|i| i as f64 * SEGMENT_SECS).collect();
        self.register(file_id, input, media, request, duration, bounds, lease, plan).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn register(
        &self,
        file_id: i64,
        input: PathBuf,
        media: MediaInfo,
        request: Request,
        duration: f64,
        bounds: Vec<f64>,
        lease: Lease,
        plan: Plan,
    ) -> Result<Arc<Session>> {
        let hdr_source = is_hdr(&media);
        let id = format!("{:016x}", rand_id());
        let dir = self.root.join(&id);
        tokio::fs::create_dir_all(&dir).await?;
        let session = Arc::new(Session {
            id: id.clone(),
            file_id,
            input,
            dir,
            duration,
            bounds,
            request,
            media,
            lease,
            plan,
            running: Mutex::new(None),
            playhead: Default::default(),
            init: Mutex::new(None),
            last_access: Mutex::new(Instant::now()),
            ffmpeg: self.hw.ffmpeg.clone(),
            hdr_source,
            filters: self.filters.clone(),
        });
        tracing::info!(session = id, file_id, plan = ?session.plan, "transcode session");
        let weak = Arc::downgrade(&session);
        tokio::spawn(async move {
            while let Some(s) = weak.upgrade() {
                s.throttle().await;
                drop(s);
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        });
        self.sessions.lock().await.insert(id, session.clone());
        Ok(session)
    }
}

pub const REMUX_CODECS: &[&str] = &["h264", "hevc", "av1", "vp9"];

/// The client said it decodes this exact stream (codec, depth, HDR), so it can be copied instead of re-encoded.
fn remuxable(media: &MediaInfo, req: &Request) -> bool {
    let Some(v) = &media.video else { return false };
    let accepts = req.video_copy.as_deref().is_some_and(|l| l.split(',').any(|c| c.trim() == v.codec));
    // Profile 5 has no HDR10 base layer, so it only looks right on a Dolby Vision decoder.
    accepts && REMUX_CODECS.contains(&v.codec.as_str()) && v.dolby_vision_profile != Some(5) && v.height as u32 <= req.max_height
}

/// Top-level MP4 boxes as (type, offset, size).
fn mp4_boxes(b: &[u8]) -> Vec<([u8; 4], usize, usize)> {
    let mut out = vec![];
    let mut i = 0;
    while i + 8 <= b.len() {
        let mut size = u32::from_be_bytes(b[i..i + 4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = b[i + 4..i + 8].try_into().unwrap();
        if size == 1 && i + 16 <= b.len() {
            size = u64::from_be_bytes(b[i + 8..i + 16].try_into().unwrap()) as usize;
        } else if size == 0 {
            size = b.len() - i;
        }
        if size < 8 || i + size > b.len() {
            break;
        }
        out.push((kind, i, size));
        i += size;
    }
    out
}

/// Segment starts on source keyframes, each at least `SEGMENT_SECS` long.
pub fn segment_bounds(keyframes: &[f64], duration: f64) -> Vec<f64> {
    let mut out = vec![0.0];
    for &k in keyframes {
        if k >= out[out.len() - 1] + SEGMENT_SECS && duration - k >= 1.0 {
            out.push(k);
        }
    }
    out
}

/// Video keyframe times per file, read once with ffprobe (it scans the whole file) and cached on disk.
pub struct Keyframes {
    ffprobe: String,
    dir: PathBuf,
    busy: std::sync::Mutex<std::collections::HashSet<i64>>,
}

impl Keyframes {
    pub fn new(ffprobe: String, dir: PathBuf) -> Arc<Self> {
        Arc::new(Self { ffprobe, dir, busy: Default::default() })
    }

    fn cache_path(&self, file_id: i64, input: &std::path::Path) -> Option<PathBuf> {
        let meta = std::fs::metadata(input).ok()?;
        let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
        Some(self.dir.join(format!("{file_id}-{}-{mtime}.json", meta.len())))
    }

    pub async fn cached(&self, file_id: i64, input: &std::path::Path) -> Option<Vec<f64>> {
        let bytes = tokio::fs::read(self.cache_path(file_id, input)?).await.ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    async fn store(&self, file_id: i64, input: &std::path::Path, k: &[f64]) -> Result<()> {
        let path = self.cache_path(file_id, input).ok_or_else(|| anyhow::anyhow!("cannot stat input"))?;
        tokio::fs::create_dir_all(&self.dir).await?;
        let prefix = format!("{file_id}-");
        if let Ok(mut d) = tokio::fs::read_dir(&self.dir).await {
            while let Ok(Some(e)) = d.next_entry().await {
                if e.file_name().to_string_lossy().starts_with(&prefix) {
                    let _ = tokio::fs::remove_file(e.path()).await;
                }
            }
        }
        tokio::fs::write(&path, serde_json::to_vec(k)?).await?;
        Ok(())
    }

    /// Cached times, else the container's own index (milliseconds even for huge files). None means only a
    /// full scan can tell.
    pub async fn quick(&self, file_id: i64, input: &std::path::Path, duration: f64) -> Option<Vec<f64>> {
        if let Some(k) = self.cached(file_id, input).await {
            return Some(k);
        }
        let path = input.to_path_buf();
        let k = tokio::task::spawn_blocking(move || crate::index::keyframes(&path)).await.ok()??;
        if !crate::index::usable(&k, duration) {
            return None;
        }
        let _ = self.store(file_id, input, &k).await;
        Some(k)
    }

    pub async fn extract(&self, file_id: i64, input: &std::path::Path, duration: f64) -> Result<Vec<f64>> {
        if let Some(k) = self.quick(file_id, input, duration).await {
            return Ok(k);
        }
        let out = tokio::process::Command::new(&self.ffprobe)
            .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "packet=pts_time,flags", "-of", "csv=p=0"])
            .arg(input)
            .stdin(std::process::Stdio::null())
            .output()
            .await?;
        if !out.status.success() {
            bail!("ffprobe: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        let mut k: Vec<f64> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let (t, flags) = l.split_once(',')?;
                if flags.starts_with('K') { t.parse().ok() } else { None }
            })
            .collect();
        k.sort_by(|a, b| a.total_cmp(b));
        k.dedup();
        self.store(file_id, input, &k).await?;
        Ok(k)
    }

    /// Extract in the background unless that file is already being read.
    pub fn spawn(self: &Arc<Self>, file_id: i64, input: PathBuf, duration: f64) {
        if !self.busy.lock().unwrap().insert(file_id) {
            return;
        }
        let me = self.clone();
        tokio::spawn(async move {
            let started = Instant::now();
            match me.extract(file_id, &input, duration).await {
                Ok(k) => tracing::info!(file_id, keyframes = k.len(), secs = started.elapsed().as_secs_f32(), "keyframes indexed"),
                Err(e) => tracing::warn!(file_id, "keyframe index failed: {e:#}"),
            }
            me.busy.lock().unwrap().remove(&file_id);
        });
    }
}

fn rand_id() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    h.finish()
}

impl Session {
    pub fn segment_count(&self) -> u64 {
        self.bounds.len() as u64
    }

    fn segment_len(&self, i: usize) -> f64 {
        self.bounds.get(i + 1).copied().unwrap_or(self.duration) - self.bounds[i]
    }

    /// A segment is complete once ffmpeg lists it: hls runs write via temp files, segment-muxer runs record a list.
    async fn ready(&self, n: u64) -> bool {
        let path = self.segment_path(n);
        if !self.plan.remux {
            return path.exists();
        }
        if !path.exists() {
            return false;
        }
        let name = format!("{n}.{}", self.ext());
        let Ok(mut dir) = tokio::fs::read_dir(&self.dir).await else { return false };
        while let Ok(Some(e)) = dir.next_entry().await {
            if !e.file_name().to_string_lossy().ends_with(".csv") {
                continue;
            }
            if let Ok(list) = tokio::fs::read_to_string(e.path()).await {
                if list.lines().any(|l| l.split(',').next().is_some_and(|f| f == name || f.ends_with(&format!("/{name}")))) {
                    return true;
                }
            }
        }
        false
    }

    fn fmp4(&self) -> bool {
        self.plan.container == "fmp4"
    }

    fn ext(&self) -> &'static str {
        if self.fmp4() { "m4s" } else { "ts" }
    }

    pub fn playlist(&self, prefix: &str, query: &str) -> String {
        let mut out = format!(
            "#EXTM3U\n#EXT-X-VERSION:{}\n#EXT-X-TARGETDURATION:{}\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-INDEPENDENT-SEGMENTS\n",
            if self.fmp4() { 7 } else { 3 },
            (0..self.bounds.len()).map(|i| self.segment_len(i)).fold(SEGMENT_SECS, f64::max).ceil() as u64
        );
        if self.fmp4() {
            out.push_str(&format!("#EXT-X-MAP:URI=\"{prefix}init.mp4{query}\"\n"));
        }
        let ext = self.ext();
        for i in 0..self.bounds.len() {
            let len = self.segment_len(i);
            out.push_str(&format!("#EXTINF:{len:.3},\n{prefix}{i}.{ext}{query}\n"));
        }
        out.push_str("#EXT-X-ENDLIST\n");
        out
    }

    fn segment_path(&self, n: u64) -> PathBuf {
        self.dir.join(format!("{n}.{}", self.ext()))
    }

    pub fn content_type(&self) -> &'static str {
        if self.fmp4() { "video/mp4" } else { "video/mp2t" }
    }

    /// fMP4 init segment. Read once the run has written its first media segment (init is complete by then),
    /// then cached: later restarts rewrite the file with identical contents.
    pub async fn init_segment(&self) -> Result<Arc<Vec<u8>>> {
        if !self.fmp4() {
            bail!("not an fMP4 stream");
        }
        if let Some(cached) = self.init.lock().await.clone() {
            return Ok(cached);
        }
        if self.plan.remux {
            let first = {
                let running = self.running.lock().await;
                running.as_ref().map_or(0, |r| r.start_segment)
            };
            let path = self.segment(first).await?;
            let bytes = tokio::fs::read(&path).await?;
            let head: Vec<u8> = mp4_boxes(&bytes)
                .into_iter()
                .filter(|(k, ..)| k == b"ftyp" || k == b"moov")
                .flat_map(|(_, o, s)| bytes[o..o + s].to_vec())
                .collect();
            if head.is_empty() {
                bail!("segment {first} has no moov");
            }
            let head = Arc::new(head);
            *self.init.lock().await = Some(head.clone());
            return Ok(head);
        }
        let path = self.dir.join("init.mp4");
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            {
                let mut running = self.running.lock().await;
                match running.as_ref() {
                    None => *running = Some(self.spawn(0)?),
                    Some(r) if path.exists() && self.segment_path(r.start_segment).exists() => {
                        let bytes = Arc::new(tokio::fs::read(&path).await?);
                        *self.init.lock().await = Some(bytes.clone());
                        return Ok(bytes);
                    }
                    _ => {}
                }
            }
            if Instant::now() > deadline {
                bail!("timed out waiting for init segment");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Wait for segment `n`, (re)starting ffmpeg there when it is not being produced soon.
    pub async fn segment(&self, n: u64) -> Result<PathBuf> {
        if n >= self.segment_count() {
            bail!("segment out of range");
        }
        let path = self.segment_path(n);
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            if self.ready(n).await {
                self.playhead.store(n, std::sync::atomic::Ordering::Relaxed);
                self.prune(n).await;
                return Ok(path);
            }
            {
                let mut running = self.running.lock().await;
                let restart = match running.as_mut() {
                    None => true,
                    Some(r) => {
                        let exited = r.child.try_wait()?.is_some();
                        let produced = self.highest_from(r.start_segment.max(n.saturating_sub(10)));
                        let ahead = produced.map_or(r.start_segment, |p| p + 1);
                        n < r.start_segment || n > ahead + 2 || (exited && !self.ready(n).await)
                    }
                };
                if restart {
                    if let Some(mut r) = running.take() {
                        let _ = r.child.kill().await;
                    }
                    *running = Some(self.spawn(n)?);
                }
            }
            if Instant::now() > deadline {
                bail!("timed out waiting for segment {n}");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Segment payload. Remuxed segments are standalone MP4 files; serve only their fragments (the shared
    /// header goes out once as init.mp4).
    pub async fn media_segment(&self, n: u64) -> Result<Vec<u8>> {
        let path = self.segment(n).await?;
        let bytes = tokio::fs::read(&path).await?;
        if !self.plan.remux {
            return Ok(bytes);
        }
        Ok(mp4_boxes(&bytes)
            .into_iter()
            .filter(|(k, ..)| !matches!(k, b"ftyp" | b"moov" | b"mfra"))
            .flat_map(|(_, o, s)| bytes[o..o + s].to_vec())
            .collect())
    }

    async fn throttle(&self) {
        let playhead = self.playhead.load(std::sync::atomic::Ordering::Relaxed);
        let mut running = self.running.lock().await;
        let Some(r) = running.as_mut() else { return };
        let from = r.start_segment.max(playhead.saturating_sub(10));
        let ahead = self.highest_from(from).map_or(0, |p| p.saturating_sub(playhead.max(r.start_segment)));
        r.throttle(ahead);
    }

    /// Drop segments well behind the playhead so a long stream never accumulates the whole movie on disk.
    async fn prune(&self, playing: u64) {
        let Some(keep_from) = playing.checked_sub(10) else { return };
        let Ok(mut dir) = tokio::fs::read_dir(&self.dir).await else { return };
        while let Ok(Some(e)) = dir.next_entry().await {
            let name = e.file_name();
            let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(&format!(".{}", self.ext()))) else { continue };
            if stem.parse::<u64>().is_ok_and(|i| i < keep_from) {
                let _ = tokio::fs::remove_file(e.path()).await;
            }
        }
    }

    fn highest_from(&self, start: u64) -> Option<u64> {
        let mut i = start;
        let mut last = None;
        while self.segment_path(i).exists() {
            last = Some(i);
            i += 1;
        }
        last
    }

    fn spawn(&self, start_segment: u64) -> Result<Running> {
        let args = self.args(start_segment);
        tracing::debug!(session = self.id, "ffmpeg {}", args.join(" "));
        let child = tokio::process::Command::new(&self.ffmpeg)
            .args(&args)
            .env("CUDA_DEVICE_ORDER", "PCI_BUS_ID")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::from(std::fs::File::create(self.dir.join("ffmpeg.log"))?))
            .kill_on_drop(true)
            .spawn()?;
        Ok(Running { child, start_segment, paused: false })
    }

    fn audio_args(&self) -> Vec<String> {
        let req = &self.request;
        let mut a: Vec<String> = vec![];
        match req.audio_stream {
            Some(i) => a.extend(["-map".into(), format!("0:{i}")]),
            None => a.extend(["-map".into(), "0:a:0?".into()]),
        }
        let source_audio = req
            .audio_stream
            .and_then(|i| self.media.audio.iter().find(|s| s.index == i))
            .or_else(|| self.media.audio.first());
        let copyable = |c: &str| match req.audio_copy.as_deref() {
            Some(list) => list.split(',').any(|x| x.trim() == c),
            None => matches!(c, "aac" | "ac3" | "eac3"),
        };
        if source_audio.is_some_and(|s| copyable(&s.codec) && s.channels.unwrap_or(2) as u32 <= req.audio_channels.max(2)) {
            a.extend(["-c:a".into(), "copy".into()]);
        } else {
            let ch = req.audio_channels.clamp(1, 8);
            a.extend(["-c:a".into(), "aac".into(), "-ac".into(), ch.to_string(), "-b:a".into(), format!("{}k", if ch > 2 { 384 } else { 192 })]);
        }
        a
    }

    /// Video copied untouched; cuts land on the source keyframes listed in `bounds`.
    fn remux_args(&self, n: u64) -> Vec<String> {
        let i = n as usize;
        // ffmpeg seeks 3/23 s early when the stream has B-frames; aim past that so the seek lands on this
        // keyframe. Timestamps are shifted by -ss, so offsetting by the same value restores source time.
        let start = if i == 0 { 0.0 } else { self.bounds[i] + 0.2 };
        let mut a: Vec<String> = vec!["-hide_banner".into(), "-nostdin".into(), "-v".into(), "warning".into(), "-y".into()];
        a.extend(["-ss".into(), format!("{start:.6}"), "-i".into(), self.input.display().to_string()]);
        a.extend(["-map".into(), "0:v:0".into(), "-c:v".into(), "copy".into()]);
        a.extend(self.audio_args());
        a.extend(["-sn".into(), "-dn".into(), "-map_metadata".into(), "-1".into(), "-map_chapters".into(), "-1".into()]);
        // A constant 1 s lead keeps B-frame decode times positive, so every run uses identical timestamps.
        a.extend(["-output_ts_offset".into(), format!("{:.6}", start + REMUX_LEAD)]);
        a.extend(["-avoid_negative_ts".into(), "disabled".into(), "-strict".into(), "experimental".into()]);
        if self.media.video.as_ref().is_some_and(|v| v.codec == "hevc") {
            a.extend(["-tag:v".into(), "hvc1".into()]);
        }
        a.extend([
            "-f".into(), "segment".into(),
            "-segment_format".into(), "mp4".into(),
            "-segment_format_options".into(), "movflags=+frag_keyframe+empty_moov+default_base_moof+frag_discont+delay_moov".into(),
        ]);
        // Cut times are compared before the output offset is applied, so they are relative to `start`.
        let times: Vec<String> = self.bounds[i + 1..].iter().map(|t| format!("{:.3}", t - start)).collect();
        if !times.is_empty() {
            a.extend(["-segment_times".into(), times.join(","), "-segment_time_delta".into(), "0.05".into()]);
        } else {
            a.extend(["-segment_time".into(), "100000".into()]);
        }
        a.extend([
            "-segment_start_number".into(), n.to_string(),
            "-segment_list".into(), self.dir.join(format!("run-{n}.csv")).display().to_string(),
            "-segment_list_type".into(), "csv".into(),
            self.dir.join("%d.m4s").display().to_string(),
        ]);
        a
    }

    pub fn args(&self, start_segment: u64) -> Vec<String> {
        if self.plan.remux {
            return self.remux_args(start_segment);
        }
        let d = &self.lease.device;
        let hw = self.lease.hw_decode;
        let start = self.bounds[start_segment as usize];
        let req = &self.request;
        let gpu_tonemap = self.plan.tonemap == Some("gpu");
        let cpu_tonemap = self.plan.tonemap == Some("cpu");
        let ten_bit_out = self.plan.hdr_out;
        let fmt = if ten_bit_out { "p010" } else { "nv12" };
        let h = req.max_height.min(self.media.video.as_ref().map(|v| v.height as u32).unwrap_or(u32::MAX).max(1));

        let mut a: Vec<String> = vec!["-hide_banner".into(), "-nostdin".into(), "-v".into(), "warning".into(), "-y".into()];
        // Hardware frames end to end only when the whole chain can stay on the GPU.
        let gpu_chain = hw && d.hw_scale && !cpu_tonemap && d.api != Api::Amf;
        if hw {
            a.extend(hwaccel::decode_args(d, gpu_chain));
        } else {
            a.extend(hwaccel::init_args(d));
        }
        a.extend(["-ss".into(), format!("{start:.3}"), "-i".into(), self.input.display().to_string()]);

        let mut vf: Vec<String> = vec![];
        if gpu_chain {
            match d.api {
                Api::Nvenc => {
                    if gpu_tonemap {
                        vf.push("tonemap_cuda=format=nv12:p=bt709:t=bt709:m=bt709:tonemap=bt2390:peak=100:desat=0".into());
                    }
                    vf.push(format!("scale_cuda=w=-2:h={h}:format={}", if gpu_tonemap { "nv12" } else { fmt }));
                }
                Api::Vaapi => {
                    if gpu_tonemap {
                        vf.push("tonemap_vaapi=format=nv12:p=bt709:t=bt709:m=bt709".into());
                    }
                    vf.push(format!("scale_vaapi=w=-2:h={h}:format={fmt}"));
                }
                Api::Qsv => {
                    let tm = if gpu_tonemap { ":tonemap=1" } else { "" };
                    vf.push(format!("vpp_qsv=w=-2:h={h}:format={fmt}{tm}"));
                }
                Api::VideoToolbox => {
                    let tm = if gpu_tonemap { ":color_matrix=bt709:color_primaries=bt709:color_transfer=bt709" } else { "" };
                    vf.push(format!("scale_vt=w=-2:h={h}{tm}"));
                }
                _ => {}
            }
        } else {
            vf.push(format!("scale=w=-2:h={h}:flags=bicubic"));
            if cpu_tonemap {
                vf.push(cpu_tonemap_filter(&self.filters));
            }
            let ten = ten_bit_out;
            match d.api {
                Api::Nvenc | Api::Vaapi | Api::Qsv => vf.push(hwaccel::upload_filter(d.api, ten)),
                _ => vf.push(format!("format={}", if ten { "yuv420p10le" } else { "yuv420p" })),
            }
        }

        a.extend(["-map".into(), "0:v:0".into()]);
        a.extend(["-vf".into(), vf.join(",")]);

        let target = match req.codec {
            _ if ten_bit_out => EncodeCodec::Hevc10,
            TargetCodec::Hevc => EncodeCodec::Hevc,
            TargetCodec::Av1 => EncodeCodec::Av1,
            TargetCodec::H264 => EncodeCodec::H264,
        };
        let encoder = hwaccel::encoder_name(d.api, target);
        let b = req.bitrate_kbps;
        a.extend(["-c:v".into(), encoder.into()]);
        a.extend(["-b:v".into(), format!("{b}k"), "-maxrate".into(), format!("{}k", b * 3 / 2), "-bufsize".into(), format!("{}k", b * 2)]);
        match d.api {
            Api::Nvenc => a.extend(["-preset".into(), "p4".into(), "-rc".into(), "vbr".into(), "-forced-idr".into(), "1".into(), "-bf".into(), "0".into()]),
            Api::Vaapi if d.vendor == Vendor::Amd => a.extend(["-rc_mode".into(), "VBR".into()]),
            Api::Vaapi => a.extend(["-rc_mode".into(), "VBR".into(), "-low_power".into(), "0".into()]),
            Api::Qsv => a.extend(["-preset".into(), "veryfast".into(), "-look_ahead".into(), "0".into()]),
            Api::Amf => a.extend(["-quality".into(), "speed".into(), "-rc".into(), "vbr_peak".into()]),
            Api::VideoToolbox => a.extend(["-realtime".into(), "1".into(), "-allow_sw".into(), "1".into()]),
            Api::Software if target == EncodeCodec::Av1 => a.extend(["-preset".into(), "10".into()]),
            Api::Software => a.extend(["-preset".into(), "veryfast".into(), "-sc_threshold".into(), "0".into()]),
        }
        if ten_bit_out {
            a.extend(["-profile:v".into(), "main10".into()]);
        } else if target == EncodeCodec::H264 && d.api != Api::Vaapi {
            a.extend(["-profile:v".into(), "high".into()]);
        }
        a.extend(["-force_key_frames".into(), format!("expr:gte(t,n_forced*{SEGMENT_SECS})")]);

        a.extend(self.audio_args());
        a.extend(["-sn".into(), "-dn".into(), "-map_metadata".into(), "-1".into(), "-map_chapters".into(), "-1".into()]);
        a.extend(["-output_ts_offset".into(), format!("{start:.3}")]);
        if matches!(target, EncodeCodec::Hevc | EncodeCodec::Hevc10) {
            a.extend(["-tag:v".into(), "hvc1".into()]);
        }
        a.extend(["-f".into(), "hls".into(), "-hls_time".into(), format!("{SEGMENT_SECS}")]);
        if self.fmp4() {
            // frag_discont writes absolute times into each fragment (tfdt) instead of an edit list in init.mp4,
            // so fragments from a restarted run line up and one cached init segment fits every run.
            a.extend([
                "-hls_segment_type".into(), "fmp4".into(),
                "-hls_fmp4_init_filename".into(), "init.mp4".into(),
                "-hls_segment_options".into(), "movflags=+frag_discont".into(),
            ]);
        } else {
            a.extend(["-hls_segment_type".into(), "mpegts".into()]);
        }
        a.extend([
            "-hls_list_size".into(), "0".into(),
            "-start_number".into(), start_segment.to_string(),
            "-hls_flags".into(), "temp_file+independent_segments".into(),
            "-hls_segment_filename".into(), self.dir.join(format!("%d.{}", self.ext())).display().to_string(),
            self.dir.join("ffmpeg.m3u8").display().to_string(),
        ]);
        let _ = (&self.hdr_source, &self.filters);
        a
    }
}

/// Seek-preview sprite sheets, decoded on a GPU (integrated preferred) using keyframes only.
pub mod trickplay {
    use super::*;

    pub const INTERVAL: u32 = 10;
    pub const WIDTH: u32 = 320;
    pub const COLS: u32 = 10;
    pub const ROWS: u32 = 10;

    #[derive(Debug, Serialize, Deserialize)]
    pub struct Manifest {
        pub interval: u32,
        pub width: u32,
        pub height: u32,
        pub cols: u32,
        pub rows: u32,
        pub count: u32,
        pub sheets: u32,
        pub device: String,
    }

    pub fn dir(data: &Path, file_id: i64) -> PathBuf {
        data.join("trickplay").join(file_id.to_string())
    }

    pub async fn generate(hw: &Hardware, data: &Path, file_id: i64, input: &Path, media: &MediaInfo) -> Result<Manifest> {
        let video = media.video.clone().unwrap_or_default();
        let duration = media.duration.unwrap_or(0.0);
        if duration <= 0.0 || video.width == 0 {
            bail!("no video");
        }
        let lease = hw.acquire_decoder(DecodeCodec::from_probe(&video.codec, video.bit_depth), Purpose::Background);
        let out = dir(data, file_id);
        let tmp = out.with_extension("tmp");
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        tokio::fs::create_dir_all(&tmp).await?;

        let mut vf = vec![format!("fps=1/{INTERVAL}"), format!("scale={WIDTH}:-2:flags=fast_bilinear")];
        if video.hdr.is_some() && (hw.has_filter("tonemapx") || hw.has_filter("zscale")) {
            vf.push(cpu_tonemap_filter(&hw.filters));
        }
        vf.push(format!("format=yuvj420p,tile={COLS}x{ROWS}"));

        let mut args: Vec<String> = vec!["-hide_banner".into(), "-nostdin".into(), "-v".into(), "error".into()];
        if lease.hw_decode {
            args.extend(hwaccel::decode_args(&lease.device, false));
        }
        args.extend(["-skip_frame".into(), "nokey".into(), "-i".into(), input.display().to_string()]);
        args.extend(["-an".into(), "-sn".into(), "-dn".into(), "-vf".into(), vf.join(","), "-fps_mode".into(), "vfr".into()]);
        args.extend(["-q:v".into(), "5".into(), tmp.join("%d.jpg").display().to_string()]);

        let started = Instant::now();
        let output = tokio::process::Command::new(&hw.ffmpeg)
            .args(&args)
            .env("CUDA_DEVICE_ORDER", "PCI_BUS_ID")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output()
            .await?;
        if !output.status.success() {
            bail!("ffmpeg: {}", String::from_utf8_lossy(&output.stderr).lines().last().unwrap_or("failed"));
        }
        let sheets = std::fs::read_dir(&tmp)?.filter(|e| e.as_ref().is_ok_and(|e| e.path().extension().is_some_and(|x| x == "jpg"))).count() as u32;
        if sheets == 0 {
            let _ = tokio::fs::remove_dir_all(&tmp).await;
            bail!("no thumbnails produced (video too short or has too few keyframes)");
        }
        let height = ((WIDTH as f64 * video.height as f64 / video.width as f64) / 2.0).round() as u32 * 2;
        let manifest = Manifest {
            interval: INTERVAL,
            width: WIDTH,
            height,
            cols: COLS,
            rows: ROWS,
            count: (duration / INTERVAL as f64).ceil() as u32,
            sheets,
            device: if lease.hw_decode { lease.device.label() } else { "CPU".into() },
        };
        tokio::fs::write(tmp.join("manifest.json"), serde_json::to_vec(&manifest)?).await?;
        let _ = tokio::fs::remove_dir_all(&out).await;
        tokio::fs::rename(&tmp, &out).await?;
        tracing::info!(file_id, sheets, device = %manifest.device, secs = started.elapsed().as_secs_f32(), "trickplay generated");
        Ok(manifest)
    }
}
