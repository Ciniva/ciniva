use std::{env, path::PathBuf};

#[derive(Debug, Clone)]
pub struct Config {
    pub media_dirs: Vec<PathBuf>,
    pub data_dir: PathBuf,
    pub port: u16,
    pub tmdb_key: Option<String>,
    pub language: String,
    pub token: Option<String>,
    pub scan_interval_mins: u64,
    pub web_dir: Option<PathBuf>,
    pub ffmpeg: String,
    pub ffprobe: String,
    pub hwaccel: String,
    pub trickplay: bool,
}

fn var(name: &str) -> Option<String> {
    env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            media_dirs: var("CINIVA_MEDIA")
                .unwrap_or_else(|| "/media".into())
                .split(',')
                .map(|s| PathBuf::from(s.trim()))
                .collect(),
            data_dir: var("CINIVA_DATA").map(PathBuf::from).unwrap_or_else(|| "./data".into()),
            port: var("CINIVA_PORT").and_then(|p| p.parse().ok()).unwrap_or(8484),
            tmdb_key: var("TMDB_API_KEY"),
            language: var("CINIVA_LANGUAGE").unwrap_or_else(|| "en-US".into()),
            token: var("CINIVA_TOKEN"),
            scan_interval_mins: var("CINIVA_SCAN_INTERVAL").and_then(|p| p.parse().ok()).unwrap_or(15),
            web_dir: var("CINIVA_WEB").map(PathBuf::from),
            ffmpeg: var("CINIVA_FFMPEG").unwrap_or_else(|| "ffmpeg".into()),
            ffprobe: var("CINIVA_FFPROBE").unwrap_or_else(|| "ffprobe".into()),
            hwaccel: var("CINIVA_HWACCEL").unwrap_or_else(|| "auto".into()).to_lowercase(),
            trickplay: var("CINIVA_TRICKPLAY").map(|v| v != "0" && v != "false" && v != "off").unwrap_or(true),
        }
    }
}
