use std::{env, path::PathBuf};

#[derive(Debug, Clone)]
pub struct Config {
    pub media_dirs: Vec<PathBuf>,
    pub data_dir: PathBuf,
    pub port: u16,
    pub tmdb_key: Option<String>,
    pub language: String,
    pub token: Option<String>,
    pub plugin_secret: Option<String>,
    pub scan_interval_mins: u64,
    pub web_dir: Option<PathBuf>,
    pub ffmpeg: String,
    pub ffprobe: String,
    pub hwaccel: String,
    pub trickplay: bool,
}

/// `NAME`, or the contents of the file named by `NAME_FILE` (Docker/Compose secrets).
fn var(name: &str) -> Option<String> {
    let direct = env::var(name).ok();
    let from_file = || env::var(format!("{name}_FILE")).ok().and_then(|path| std::fs::read_to_string(path).ok());
    direct.or_else(from_file).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
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
            plugin_secret: var("CINIVA_PLUGIN_SECRET"),
            scan_interval_mins: var("CINIVA_SCAN_INTERVAL").and_then(|p| p.parse().ok()).unwrap_or(15),
            web_dir: var("CINIVA_WEB").map(PathBuf::from),
            ffmpeg: var("CINIVA_FFMPEG").unwrap_or_else(|| "ffmpeg".into()),
            ffprobe: var("CINIVA_FFPROBE").unwrap_or_else(|| "ffprobe".into()),
            hwaccel: var("CINIVA_HWACCEL").unwrap_or_else(|| "auto".into()).to_lowercase(),
            trickplay: var("CINIVA_TRICKPLAY").map(|v| v != "0" && v != "false" && v != "off").unwrap_or(true),
        }
    }
}
