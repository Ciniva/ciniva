use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaInfo {
    pub container: Option<String>,
    pub duration: Option<f64>,
    pub bitrate: Option<i64>,
    pub video: Option<VideoInfo>,
    pub audio: Vec<AudioInfo>,
    pub subtitles: Vec<SubtitleInfo>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VideoInfo {
    pub codec: String,
    pub profile: Option<String>,
    pub width: i64,
    pub height: i64,
    pub bit_depth: Option<i64>,
    pub hdr: Option<String>,
    pub dolby_vision_profile: Option<i64>,
    pub fps: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AudioInfo {
    pub index: i64,
    pub codec: String,
    pub profile: Option<String>,
    pub channels: Option<i64>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub atmos: bool,
    pub default: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubtitleInfo {
    pub index: i64,
    pub codec: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub forced: bool,
    pub default: bool,
}

#[derive(Deserialize)]
struct Probe {
    #[serde(default)]
    streams: Vec<Stream>,
    format: Option<Format>,
}

#[derive(Deserialize)]
struct Format {
    format_name: Option<String>,
    duration: Option<String>,
    bit_rate: Option<String>,
}

#[derive(Deserialize, Default)]
struct Disposition {
    #[serde(default)]
    default: i64,
    #[serde(default)]
    forced: i64,
    #[serde(default)]
    attached_pic: i64,
}

#[derive(Deserialize)]
struct Stream {
    index: i64,
    codec_type: Option<String>,
    codec_name: Option<String>,
    profile: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    channels: Option<i64>,
    pix_fmt: Option<String>,
    bits_per_raw_sample: Option<String>,
    color_transfer: Option<String>,
    avg_frame_rate: Option<String>,
    #[serde(default)]
    side_data_list: Vec<serde_json::Value>,
    #[serde(default)]
    tags: std::collections::HashMap<String, String>,
    #[serde(default)]
    disposition: Disposition,
}

fn fps(rate: &Option<String>) -> Option<f64> {
    let (n, d) = rate.as_deref()?.split_once('/')?;
    let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
    (d > 0.0 && n > 0.0).then(|| (n / d * 1000.0).round() / 1000.0)
}

fn tag(tags: &std::collections::HashMap<String, String>, key: &str) -> Option<String> {
    tags.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.clone())
}

pub async fn probe(ffprobe: &str, path: &Path) -> Result<MediaInfo> {
    probe_input(ffprobe, &[], path.as_os_str()).await
}

/// Probe any ffmpeg input (a file or a URL); `extra` goes before the input (timeouts, HTTP headers).
pub async fn probe_input(ffprobe: &str, extra: &[String], input: &std::ffi::OsStr) -> Result<MediaInfo> {
    let out = tokio::process::Command::new(ffprobe)
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .args(extra)
        .arg(input)
        .kill_on_drop(true)
        .output()
        .await?;
    if !out.status.success() {
        bail!("ffprobe failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let p: Probe = serde_json::from_slice(&out.stdout)?;
    let mut info = MediaInfo {
        container: p.format.as_ref().and_then(|f| f.format_name.clone()),
        duration: p.format.as_ref().and_then(|f| f.duration.as_deref()?.parse().ok()),
        bitrate: p.format.as_ref().and_then(|f| f.bit_rate.as_deref()?.parse().ok()),
        ..Default::default()
    };

    for s in p.streams {
        let codec = s.codec_name.clone().unwrap_or_default();
        match s.codec_type.as_deref() {
            Some("video") if info.video.is_none() && s.disposition.attached_pic == 0 => {
                let dv = s
                    .side_data_list
                    .iter()
                    .find(|d| d.get("dv_profile").is_some())
                    .and_then(|d| d.get("dv_profile")?.as_i64());
                let hdr = match (dv, s.color_transfer.as_deref()) {
                    (Some(_), _) => Some("dolby_vision".to_string()),
                    (_, Some("smpte2084")) => {
                        let plus = s.side_data_list.iter().any(|d| {
                            d.get("side_data_type").and_then(|t| t.as_str()).is_some_and(|t| t.contains("HDR10+") || t.contains("SMPTE2094-40"))
                        });
                        Some(if plus { "hdr10+" } else { "hdr10" }.to_string())
                    }
                    (_, Some("arib-std-b67")) => Some("hlg".to_string()),
                    _ => None,
                };
                let bit_depth = s
                    .bits_per_raw_sample
                    .as_deref()
                    .and_then(|b| b.parse().ok())
                    .or_else(|| s.pix_fmt.as_deref().map(|f| if f.contains("10") { 10 } else if f.contains("12") { 12 } else { 8 }));
                info.video = Some(VideoInfo {
                    codec,
                    profile: s.profile,
                    width: s.width.unwrap_or(0),
                    height: s.height.unwrap_or(0),
                    bit_depth,
                    hdr,
                    dolby_vision_profile: dv,
                    fps: fps(&s.avg_frame_rate),
                });
            }
            Some("audio") => {
                let title = tag(&s.tags, "title");
                let atmos = s.profile.as_deref().is_some_and(|p| p.to_lowercase().contains("atmos"))
                    || title.as_deref().is_some_and(|t| t.to_lowercase().contains("atmos"));
                info.audio.push(AudioInfo {
                    index: s.index,
                    codec,
                    profile: s.profile,
                    channels: s.channels,
                    language: tag(&s.tags, "language"),
                    title,
                    atmos,
                    default: s.disposition.default == 1,
                });
            }
            Some("subtitle") => info.subtitles.push(SubtitleInfo {
                index: s.index,
                codec,
                language: tag(&s.tags, "language"),
                title: tag(&s.tags, "title"),
                forced: s.disposition.forced == 1,
                default: s.disposition.default == 1,
            }),
            _ => {}
        }
    }
    Ok(info)
}
