use axum::http::{HeaderMap, Method, StatusCode};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    secret: Option<String>,
}

#[derive(Debug)]
pub struct Error(pub StatusCode, pub String);

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error(StatusCode::BAD_GATEWAY, format!("Ciniva unreachable: {e}"))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

impl Client {
    pub fn new(base: String, secret: Option<String>) -> Client {
        let http = reqwest::Client::builder().connect_timeout(Duration::from_secs(5)).build().expect("http client");
        Client { http, base: base.trim_end_matches('/').to_string(), secret }
    }

    pub fn secret(&self) -> Option<&str> {
        self.secret.as_deref()
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}/api{path}", self.base)
    }

    async fn check(resp: reqwest::Response) -> Result<reqwest::Response> {
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let body = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
            .unwrap_or(body);
        let code = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        Err(Error(code, msg))
    }

    pub async fn get<T: DeserializeOwned>(&self, token: &str, path: &str) -> Result<T> {
        let resp = self.http.get(self.url(path)).bearer_auth(token).timeout(Duration::from_secs(60)).send().await?;
        Self::check(resp).await?.json().await.map_err(|e| Error(StatusCode::BAD_GATEWAY, format!("unexpected Ciniva response: {e}")))
    }

    pub async fn send<B: Serialize>(&self, token: Option<&str>, method: Method, path: &str, body: &B) -> Result<reqwest::Response> {
        let mut req = self.http.request(method, self.url(path)).json(body).timeout(Duration::from_secs(60));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        Self::check(req.send().await?).await
    }

    /// Streams a Ciniva response back, passing `Range` through.
    pub async fn raw(&self, token: &str, method: Method, path: &str, headers: &HeaderMap) -> Result<reqwest::Response> {
        let mut req = self.http.request(method, self.url(path)).bearer_auth(token);
        for name in ["range", "if-range", "if-none-match", "if-modified-since"] {
            if let Some(v) = headers.get(name) {
                req = req.header(name, v.clone());
            }
        }
        let resp = req.send().await?;
        if resp.status().is_success() || resp.status() == reqwest::StatusCode::NOT_MODIFIED || resp.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
            return Ok(resp);
        }
        Self::check(resp).await
    }

    pub async fn me(&self, token: &str) -> Result<User> {
        let me: Me = self.get(token, "/me").await?;
        me.user.ok_or_else(|| Error(StatusCode::UNAUTHORIZED, "not a user session".into()))
    }

    pub async fn movies(&self, token: &str, sort: &str) -> Result<Vec<MovieCard>> {
        self.get(token, &format!("/movies?sort={sort}")).await
    }

    pub async fn movie(&self, token: &str, id: i64) -> Result<Movie> {
        self.get(token, &format!("/movies/{id}")).await
    }

    pub async fn shows(&self, token: &str, sort: &str) -> Result<Vec<ShowCard>> {
        self.get(token, &format!("/shows?sort={sort}")).await
    }

    pub async fn show(&self, token: &str, id: i64) -> Result<Show> {
        self.get(token, &format!("/shows/{id}")).await
    }

    pub async fn season(&self, token: &str, show: i64, season: i64) -> Result<Vec<Episode>> {
        self.get(token, &format!("/shows/{show}/seasons/{season}")).await
    }

    pub async fn file(&self, token: &str, id: i64) -> Result<File> {
        self.get(token, &format!("/files/{id}")).await
    }

    pub async fn home(&self, token: &str) -> Result<Home> {
        self.get(token, "/home").await
    }
}

#[derive(Deserialize)]
struct Me {
    user: Option<User>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: i64,
    pub username: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Progress {
    pub position_ms: Option<i64>,
    pub duration_ms: Option<i64>,
    pub watched: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct MovieCard {
    pub id: i64,
    pub title: String,
    pub year: Option<i64>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub rating: Option<f64>,
    pub added_at: Option<String>,
    #[serde(flatten)]
    pub progress: Progress,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ShowCard {
    pub id: i64,
    pub title: String,
    pub year: Option<i64>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub rating: Option<f64>,
    pub episodes: i64,
    pub watched_episodes: i64,
    pub last_added: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Cast {
    pub name: String,
    pub character: Option<String>,
    pub profile_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Movie {
    pub id: i64,
    pub tmdb_id: Option<i64>,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i64>,
    pub overview: Option<String>,
    pub tagline: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub runtime: Option<i64>,
    pub rating: Option<f64>,
    pub genres: Vec<String>,
    pub release_date: Option<String>,
    pub cast: Vec<Cast>,
    pub files: Vec<File>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Show {
    pub id: i64,
    pub tmdb_id: Option<i64>,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i64>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub rating: Option<f64>,
    pub genres: Vec<String>,
    pub first_air_date: Option<String>,
    pub status: Option<String>,
    pub cast: Vec<Cast>,
    pub seasons: Vec<SeasonCard>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct SeasonCard {
    pub season_number: i64,
    pub name: Option<String>,
    pub poster_path: Option<String>,
    pub air_date: Option<String>,
    pub episode_count: Option<i64>,
    pub available: i64,
    pub watched: i64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Episode {
    pub id: i64,
    pub season_number: i64,
    pub episode_number: i64,
    pub title: Option<String>,
    pub overview: Option<String>,
    pub still_path: Option<String>,
    pub air_date: Option<String>,
    pub runtime: Option<i64>,
    pub rating: Option<f64>,
    pub files: Vec<File>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct File {
    pub id: i64,
    pub path: String,
    pub size: i64,
    pub duration: Option<f64>,
    pub probe: Option<MediaInfo>,
    pub added_at: Option<String>,
    pub credits_start: Option<f64>,
    #[serde(flatten)]
    pub progress: Progress,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct MediaInfo {
    pub container: Option<String>,
    pub duration: Option<f64>,
    pub bitrate: Option<i64>,
    pub video: Option<VideoInfo>,
    pub audio: Vec<AudioInfo>,
    pub subtitles: Vec<SubtitleInfo>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
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

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
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

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct SubtitleInfo {
    pub index: i64,
    pub codec: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub forced: bool,
    pub default: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ContinueItem {
    pub file_id: i64,
    pub position_ms: i64,
    pub duration_ms: i64,
    pub updated_at: Option<String>,
    pub movie_id: Option<i64>,
    pub show_id: Option<i64>,
    pub title: String,
    pub subtitle: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Home {
    pub continue_watching: Vec<ContinueItem>,
}
