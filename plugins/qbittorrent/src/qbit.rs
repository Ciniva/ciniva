//! qBittorrent Web API (v2). The plugin sits on Ciniva's internal network, which qBittorrent lets in without a
//! login (subnet whitelist), so no credentials are needed.

use anyhow::{bail, Result};
use serde::Deserialize;
use std::time::Duration;

#[derive(Clone)]
pub struct Qbit {
    base: String,
    http: reqwest::Client,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Torrent {
    pub hash: String,
    pub name: String,
    pub size: i64,
    pub progress: f64,
    pub dlspeed: i64,
    pub upspeed: i64,
    pub eta: i64,
    pub state: String,
    pub ratio: f64,
    pub num_seeds: i64,
    pub num_leechs: i64,
    pub save_path: String,
    #[serde(default)]
    pub content_path: String,
    #[serde(default)]
    pub seq_dl: bool,
    #[serde(default)]
    pub f_l_piece_prio: bool,
    #[serde(default)]
    pub added_on: i64,
    #[serde(default)]
    pub category: String,
}

impl Torrent {
    pub fn complete(&self) -> bool {
        self.progress >= 1.0
    }

    pub fn paused(&self) -> bool {
        self.state.starts_with("stopped") || self.state.starts_with("paused")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TorrentFile {
    pub index: i64,
    pub name: String,
    pub size: i64,
    pub progress: f64,
    pub priority: i64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Transfer {
    pub dl_info_speed: i64,
    pub up_info_speed: i64,
    #[serde(default)]
    pub connection_status: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Properties {
    #[serde(default)]
    pub total_downloaded: i64,
    #[serde(default)]
    pub total_uploaded: i64,
    #[serde(default)]
    pub peers_total: i64,
    #[serde(default)]
    pub seeds_total: i64,
    #[serde(default)]
    pub addition_date: i64,
    #[serde(default)]
    pub comment: String,
}

pub struct NewTorrent {
    pub urls: Vec<String>,
    pub file: Option<(String, Vec<u8>)>,
    pub sequential: bool,
}

impl Qbit {
    pub fn new(base: &str) -> Self {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(30)).build().expect("http client");
        Self { base: base.trim_end_matches('/').to_string(), http }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/v2/{path}", self.base)
    }

    async fn get<T: for<'de> Deserialize<'de>>(&self, path: &str, query: &[(&str, &str)]) -> Result<T> {
        let resp = self.http.get(self.url(path)).query(query).send().await?;
        if !resp.status().is_success() {
            bail!("qBittorrent {path}: HTTP {}", resp.status());
        }
        Ok(resp.json().await?)
    }

    async fn post(&self, path: &str, form: &[(&str, String)]) -> Result<()> {
        let resp = self.http.post(self.url(path)).form(form).send().await?;
        if !resp.status().is_success() {
            bail!("qBittorrent {path}: HTTP {} {}", resp.status(), resp.text().await.unwrap_or_default());
        }
        Ok(())
    }

    pub async fn version(&self) -> Result<String> {
        Ok(self.http.get(self.url("app/version")).send().await?.error_for_status()?.text().await?)
    }

    pub async fn torrents(&self) -> Result<Vec<Torrent>> {
        self.get("torrents/info", &[("sort", "added_on"), ("reverse", "true")]).await
    }

    pub async fn torrent(&self, hash: &str) -> Result<Option<Torrent>> {
        let list: Vec<Torrent> = self.get("torrents/info", &[("hashes", hash)]).await?;
        Ok(list.into_iter().next())
    }

    pub async fn files(&self, hash: &str) -> Result<Vec<TorrentFile>> {
        self.get("torrents/files", &[("hash", hash)]).await
    }

    pub async fn properties(&self, hash: &str) -> Result<Properties> {
        self.get("torrents/properties", &[("hash", hash)]).await
    }

    pub async fn transfer(&self) -> Result<Transfer> {
        self.get("transfer/info", &[]).await
    }

    pub async fn stop(&self, hash: &str) -> Result<()> {
        self.post("torrents/stop", &[("hashes", hash.to_string())]).await
    }

    pub async fn start(&self, hash: &str) -> Result<()> {
        self.post("torrents/start", &[("hashes", hash.to_string())]).await
    }

    pub async fn delete(&self, hash: &str, files: bool) -> Result<()> {
        self.post("torrents/delete", &[("hashes", hash.to_string()), ("deleteFiles", files.to_string())]).await
    }

    /// Turns sequential download (and first/last pieces first, so the container index arrives early) on or off.
    pub async fn set_sequential(&self, t: &Torrent, on: bool) -> Result<()> {
        if t.seq_dl != on {
            self.post("torrents/toggleSequentialDownload", &[("hashes", t.hash.clone())]).await?;
        }
        if t.f_l_piece_prio != on {
            self.post("torrents/toggleFirstLastPiecePrio", &[("hashes", t.hash.clone())]).await?;
        }
        Ok(())
    }

    pub async fn file_priority(&self, hash: &str, index: i64, priority: i64) -> Result<()> {
        self.post("torrents/filePrio", &[("hash", hash.to_string()), ("id", index.to_string()), ("priority", priority.to_string())]).await
    }

    pub async fn add(&self, t: NewTorrent) -> Result<()> {
        let mut form = reqwest::multipart::Form::new();
        if !t.urls.is_empty() {
            form = form.text("urls", t.urls.join("\n"));
        }
        if let Some((name, bytes)) = t.file {
            form = form.part("torrents", reqwest::multipart::Part::bytes(bytes).file_name(name).mime_str("application/x-bittorrent")?);
        }
        if t.sequential {
            form = form.text("sequentialDownload", "true").text("firstLastPiecePrio", "true");
        }
        let resp = self.http.post(self.url("torrents/add")).multipart(form).send().await?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() || body.trim().eq_ignore_ascii_case("fails.") {
            bail!("qBittorrent did not accept the torrent ({status} {body})");
        }
        Ok(())
    }
}
