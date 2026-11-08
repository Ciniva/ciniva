//! Prowlarr search API (searches every configured indexer in parallel).

use anyhow::{bail, Result};
use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub title: String,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub seeders: Option<i64>,
    #[serde(default)]
    pub indexer: String,
    #[serde(default)]
    pub publish_date: Option<String>,
    #[serde(default)]
    pub download_url: Option<String>,
    #[serde(default)]
    pub magnet_url: Option<String>,
    #[serde(default)]
    pub guid: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Tv,
    Movie,
    Any,
}

impl Kind {
    pub fn parse(s: &str) -> Self {
        match s {
            "tv" => Kind::Tv,
            "movie" => Kind::Movie,
            _ => Kind::Any,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Tv => "tv",
            Kind::Movie => "movie",
            Kind::Any => "any",
        }
    }
}

pub async fn indexer_count(base: &str, key: &str) -> Result<usize> {
    let http = reqwest::Client::builder().timeout(Duration::from_secs(10)).build()?;
    let resp = http.get(format!("{}/api/v1/indexer", base.trim_end_matches('/'))).header("X-Api-Key", key).send().await?;
    if !resp.status().is_success() {
        bail!("Prowlarr: HTTP {}", resp.status());
    }
    let list: Vec<serde_json::Value> = resp.json().await?;
    Ok(list.iter().filter(|i| i["enable"].as_bool().unwrap_or(true)).count())
}

pub async fn search(base: &str, key: &str, query: &str, kind: Kind) -> Result<Vec<Release>> {
    let http = reqwest::Client::builder().timeout(Duration::from_secs(90)).build()?;
    let mut q: Vec<(&str, String)> = vec![("query", query.to_string()), ("type", "search".into()), ("limit", "100".into())];
    match kind {
        Kind::Tv => q.push(("categories", "5000".into())),
        Kind::Movie => q.push(("categories", "2000".into())),
        Kind::Any => {}
    }
    let resp = http.get(format!("{}/api/v1/search", base.trim_end_matches('/'))).header("X-Api-Key", key).query(&q).send().await?;
    if !resp.status().is_success() {
        bail!("Prowlarr search: HTTP {} {}", resp.status(), resp.text().await.unwrap_or_default().chars().take(200).collect::<String>());
    }
    Ok(resp.json().await?)
}
