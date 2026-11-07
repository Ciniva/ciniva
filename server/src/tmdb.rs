use crate::parser::normalize;
use anyhow::{bail, Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{Mutex, Semaphore};

const BASE: &str = "https://api.themoviedb.org/3";
/// TMDB also searches alternative and translated titles; a single hit is trusted even when its display title differs.
const SINGLE_HIT_SCORE: f64 = 0.9;

#[derive(Clone)]
pub struct Tmdb {
    http: reqwest::Client,
    key: String,
    language: String,
    cache: Arc<Mutex<HashMap<String, Arc<str>>>>,
    limiter: Arc<Semaphore>,
}

#[derive(Debug, Deserialize)]
struct Page<T> {
    results: Vec<T>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MovieHit {
    pub id: i64,
    pub title: String,
    pub original_title: Option<String>,
    pub release_date: Option<String>,
    pub popularity: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TvHit {
    pub id: i64,
    pub name: String,
    pub original_name: Option<String>,
    pub first_air_date: Option<String>,
    pub popularity: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Genre {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CastMember {
    pub name: String,
    pub character: Option<String>,
    pub profile_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Credits {
    #[serde(default)]
    pub cast: Vec<CastMember>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MovieDetails {
    pub id: i64,
    pub title: String,
    pub original_title: Option<String>,
    pub overview: Option<String>,
    pub tagline: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub runtime: Option<i64>,
    pub vote_average: Option<f64>,
    #[serde(default)]
    pub genres: Vec<Genre>,
    pub release_date: Option<String>,
    #[serde(default)]
    pub credits: Credits,
    pub belongs_to_collection: Option<CollectionRef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CollectionRef {
    pub id: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Collection {
    #[serde(default)]
    pub parts: Vec<MovieHit>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeasonSummary {
    pub season_number: i64,
    pub name: Option<String>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub air_date: Option<String>,
    pub episode_count: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TvDetails {
    pub id: i64,
    pub name: String,
    pub original_name: Option<String>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub vote_average: Option<f64>,
    #[serde(default)]
    pub genres: Vec<Genre>,
    pub first_air_date: Option<String>,
    pub status: Option<String>,
    #[serde(default)]
    pub seasons: Vec<SeasonSummary>,
    #[serde(default)]
    pub credits: Credits,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EpisodeDetails {
    pub season_number: i64,
    pub episode_number: i64,
    pub name: Option<String>,
    pub overview: Option<String>,
    pub still_path: Option<String>,
    pub air_date: Option<String>,
    pub runtime: Option<i64>,
    pub vote_average: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeasonDetails {
    pub season_number: i64,
    pub name: Option<String>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub air_date: Option<String>,
    #[serde(default)]
    pub episodes: Vec<EpisodeDetails>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Scored<T> {
    pub hit: T,
    pub score: f64,
}

pub fn year_of(date: &Option<String>) -> Option<i32> {
    date.as_deref().and_then(|d| d.get(..4)).and_then(|y| y.parse().ok())
}

pub fn score(query: &str, query_year: Option<i32>, titles: &[&str], year: Option<i32>, popularity: Option<f64>) -> f64 {
    let q = normalize(query);
    let q_words: std::collections::HashSet<&str> = q.split_whitespace().collect();
    let mut s = titles
        .iter()
        .map(|t| {
            let t = normalize(t);
            let jw = strsim::jaro_winkler(&q, &t);
            let t_words: std::collections::HashSet<&str> = t.split_whitespace().collect();
            let shared = q_words.intersection(&t_words).count() as f64;
            let contained = q_words.len() >= 2 && shared == q_words.len() as f64 && shared / t_words.len().max(1) as f64 >= 0.6;
            if contained { jw.max(0.9) } else { jw }
        })
        .fold(0.0, f64::max);
    match (query_year, year) {
        (Some(a), Some(b)) if a == b => s += 0.15,
        (Some(a), Some(b)) if (a - b).abs() == 1 => s += 0.05,
        (Some(_), Some(_)) => s -= 0.25,
        _ => {}
    }
    s + (popularity.unwrap_or(0.0).ln_1p() / 100.0).min(0.05)
}

impl Tmdb {
    pub fn new(key: String, language: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent(concat!("Ciniva/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("http client");
        Self { http, key, language, cache: Default::default(), limiter: Arc::new(Semaphore::new(8)) }
    }

    fn language_for(&self, text: &str) -> String {
        let has = |lo: char, hi: char| text.chars().any(|c| (lo..=hi).contains(&c));
        if has('\u{0400}', '\u{04FF}') {
            "ru-RU".into()
        } else if has('\u{10A0}', '\u{10FF}') {
            "ka-GE".into()
        } else if has('\u{3040}', '\u{30FF}') || has('\u{4E00}', '\u{9FFF}') {
            "ja-JP".into()
        } else if has('\u{AC00}', '\u{D7AF}') {
            "ko-KR".into()
        } else {
            self.language.clone()
        }
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T> {
        let mut params: Vec<(&str, String)> = query.to_vec();
        if !params.iter().any(|(k, _)| *k == "language") {
            params.push(("language", self.language.clone()));
        }
        let bearer = self.key.starts_with("eyJ");
        if !bearer {
            params.push(("api_key", self.key.clone()));
        }
        let url = reqwest::Url::parse_with_params(&format!("{BASE}{path}"), &params)?;
        let cache_key = url.as_str().to_string();
        if let Some(body) = self.cache.lock().await.get(&cache_key).cloned() {
            return Ok(serde_json::from_str(&body)?);
        }

        let _permit = self.limiter.acquire().await?;
        let mut attempt = 0;
        let body = loop {
            let mut req = self.http.get(url.clone());
            if bearer {
                req = req.bearer_auth(&self.key);
            }
            let resp = req.send().await.with_context(|| format!("TMDB {path}"))?;
            let status = resp.status();
            if status.as_u16() == 429 && attempt < 5 {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(500 * attempt)).await;
                continue;
            }
            if status.as_u16() == 404 {
                bail!("TMDB {path}: not found");
            }
            if !status.is_success() {
                bail!("TMDB {path}: HTTP {status}");
            }
            break resp.text().await?;
        };
        let parsed = serde_json::from_str(&body).with_context(|| format!("TMDB {path}: bad JSON"))?;
        self.cache.lock().await.insert(cache_key, body.into());
        Ok(parsed)
    }

    pub async fn search_movie(&self, title: &str, year: Option<i32>) -> Result<Vec<Scored<MovieHit>>> {
        let mut q = vec![("query", title.to_string()), ("include_adult", "false".into()), ("language", self.language_for(title))];
        let mut hits: Vec<MovieHit> = self.get::<Page<MovieHit>>("/search/movie", &q).await?.results;
        if let Some(y) = year {
            q.push(("year", y.to_string()));
            let with_year: Vec<MovieHit> = self.get::<Page<MovieHit>>("/search/movie", &q).await?.results;
            if !with_year.is_empty() {
                hits = with_year;
            }
        }
        let mut scored: Vec<_> = hits
            .into_iter()
            .map(|h| {
                let titles = [h.title.as_str(), h.original_title.as_deref().unwrap_or("")];
                let score = score(title, year, &titles, year_of(&h.release_date), h.popularity);
                Scored { hit: h, score }
            })
            .collect();
        if let [only] = scored.as_mut_slice() {
            let year_ok = match (year, year_of(&only.hit.release_date)) {
                (Some(a), Some(b)) => (a - b).abs() <= 1,
                _ => true,
            };
            if year_ok && title.chars().count() >= 4 {
                only.score = only.score.max(SINGLE_HIT_SCORE);
            }
        }
        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(scored)
    }

    pub async fn search_tv(&self, title: &str, year: Option<i32>) -> Result<Vec<Scored<TvHit>>> {
        let mut q = vec![("query", title.to_string()), ("include_adult", "false".into()), ("language", self.language_for(title))];
        let mut hits: Vec<TvHit> = self.get::<Page<TvHit>>("/search/tv", &q).await?.results;
        if let Some(y) = year {
            q.push(("first_air_date_year", y.to_string()));
            let with_year: Vec<TvHit> = self.get::<Page<TvHit>>("/search/tv", &q).await?.results;
            if !with_year.is_empty() {
                hits = with_year;
            }
        }
        let mut scored: Vec<_> = hits
            .into_iter()
            .map(|h| {
                let titles = [h.name.as_str(), h.original_name.as_deref().unwrap_or("")];
                let score = score(title, year, &titles, year_of(&h.first_air_date), h.popularity);
                Scored { hit: h, score }
            })
            .collect();
        if let [only] = scored.as_mut_slice() {
            let year_ok = match (year, year_of(&only.hit.first_air_date)) {
                (Some(a), Some(b)) => (a - b).abs() <= 1,
                _ => true,
            };
            if year_ok && title.chars().count() >= 4 {
                only.score = only.score.max(SINGLE_HIT_SCORE);
            }
        }
        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(scored)
    }

    pub async fn movie(&self, id: i64) -> Result<MovieDetails> {
        self.get(&format!("/movie/{id}"), &[("append_to_response", "credits".into())]).await
    }

    pub async fn collection(&self, id: i64) -> Result<Collection> {
        self.get(&format!("/collection/{id}"), &[]).await
    }

    pub async fn tv(&self, id: i64) -> Result<TvDetails> {
        self.get(&format!("/tv/{id}"), &[("append_to_response", "credits".into())]).await
    }

    pub async fn season(&self, tv_id: i64, season: i64) -> Result<SeasonDetails> {
        self.get(&format!("/tv/{tv_id}/season/{season}"), &[]).await
    }
}
