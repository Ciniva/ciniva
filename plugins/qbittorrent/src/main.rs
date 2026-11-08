mod prowlarr;
mod qbit;
mod release;
mod ui;

use axum::{
    extract::{Path, Query, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Map, Value};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::RwLock;

pub struct Config {
    pub ciniva_url: String,
    pub secret: String,
    pub plugin_url: String,
    pub port: u16,
    pub qbit_url: String,
    pub prowlarr_url: String,
    pub prowlarr_key: String,
    /// qBittorrent path prefix → the same folder as Ciniva sees it.
    pub path_map: Vec<(String, String)>,
}

/// `NAME`, or the contents of the file named by `NAME_FILE` (Docker/Compose secrets).
fn env(name: &str, default: &str) -> String {
    let from_file = || std::env::var(format!("{name}_FILE")).ok().and_then(|path| std::fs::read_to_string(path).ok());
    std::env::var(name).ok().or_else(from_file).map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

impl Config {
    fn from_env() -> Self {
        Self {
            ciniva_url: env("CINIVA_URL", "http://ciniva:8484").trim_end_matches('/').to_string(),
            secret: env("CINIVA_PLUGIN_SECRET", ""),
            plugin_url: env("PLUGIN_URL", "http://ciniva-qbittorrent:8090"),
            port: env("PORT", "8090").parse().unwrap_or(8090),
            qbit_url: env("QBIT_URL", "http://qbittorrent-internal:8080"),
            prowlarr_url: env("PROWLARR_URL", "http://prowlarr-internal:9696"),
            prowlarr_key: env("PROWLARR_API_KEY", ""),
            path_map: env("PATH_MAP", "/data/storage/share/Downloads=/media/Downloads")
                .split(';')
                .filter_map(|p| p.split_once('=').map(|(a, b)| (a.trim().to_string(), b.trim().to_string())))
                .collect(),
        }
    }

    pub fn to_ciniva(&self, path: &str) -> Option<String> {
        self.path_map.iter().find_map(|(from, to)| path.strip_prefix(from.as_str()).map(|rest| format!("{to}{rest}")))
    }

    pub fn downloads_root(&self) -> Option<&str> {
        self.path_map.first().map(|(from, _)| from.as_str())
    }
}

pub struct App {
    pub cfg: Config,
    pub qbit: qbit::Qbit,
    pub settings: RwLock<Map<String, Value>>,
    http: reqwest::Client,
}

pub type Shared = Arc<App>;

impl App {
    pub async fn setting_str(&self, name: &str) -> String {
        self.settings.read().await.get(name).and_then(|v| v.as_str()).unwrap_or_default().to_string()
    }

    pub async fn setting_f64(&self, name: &str) -> Option<f64> {
        let s = self.settings.read().await;
        s.get(name).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|x| x.parse().ok())))
    }

    pub async fn setting_bool(&self, name: &str, default: bool) -> bool {
        self.settings.read().await.get(name).and_then(|v| v.as_bool()).unwrap_or(default)
    }

    pub async fn prowlarr(&self) -> (String, String) {
        let url = Some(self.setting_str("prowlarr_url").await).filter(|s| !s.is_empty()).unwrap_or_else(|| self.cfg.prowlarr_url.clone());
        let key = Some(self.setting_str("prowlarr_api_key").await).filter(|s| !s.is_empty()).unwrap_or_else(|| self.cfg.prowlarr_key.clone());
        (url, key)
    }

    async fn ciniva_post(&self, path: &str, body: Value) -> anyhow::Result<reqwest::Response> {
        Ok(self.http.post(format!("{}{path}", self.cfg.ciniva_url)).bearer_auth(&self.cfg.secret).json(&body).send().await?)
    }

    pub async fn notify(&self, title: &str, body: &str, tone: &str) {
        if !self.setting_bool("notify", true).await {
            return;
        }
        let _ = self.ciniva_post("/api/notifications", json!({ "title": title, "body": body, "tone": tone, "plugin": "qbittorrent" })).await;
    }

    pub async fn rescan(&self, ciniva_path: &str) {
        match self.ciniva_post("/api/scan", json!({ "paths": [ciniva_path] })).await {
            Ok(r) if r.status().is_success() => tracing::info!(path = ciniva_path, "asked Ciniva to rescan"),
            Ok(r) => tracing::warn!(status = %r.status(), "rescan request refused"),
            Err(e) => tracing::warn!("rescan request failed: {e:#}"),
        }
    }
}

fn manifest(cfg: &Config) -> Value {
    json!({
        "id": "qbittorrent",
        "name": "qBittorrent",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Downloads, Prowlarr search and sequential downloading for qBittorrent.",
        "icon": "download",
        "url": cfg.plugin_url,
        "settings": [
            { "name": "prowlarr_url", "label": "Prowlarr address", "kind": "text", "value": cfg.prowlarr_url, "help": "As reachable from the plugin container" },
            { "name": "prowlarr_api_key", "label": "Prowlarr API key", "kind": "password", "value": cfg.prowlarr_key, "help": "Prowlarr → Settings → General → API Key" },
            { "name": "resolution", "label": "Preferred quality", "kind": "select", "value": "1080",
              "options": [{ "value": "2160", "label": "4K (2160p)" }, { "value": "1080", "label": "1080p" }, { "value": "720", "label": "720p" }, { "value": "any", "label": "No preference" }] },
            { "name": "languages", "label": "Preferred audio languages", "kind": "text", "value": "rus,eng", "help": "Comma-separated: rus, eng, ukr" },
            { "name": "max_episode_gb", "label": "Largest episode (GB)", "kind": "number", "value": 12 },
            { "name": "max_movie_gb", "label": "Largest movie (GB)", "kind": "number", "value": 80 },
            { "name": "notify", "label": "Notify when a download finishes", "kind": "toggle", "value": true }
        ],
        "pages": [
            { "id": "downloads", "title": "Downloads", "icon": "download", "nav": true },
            { "id": "search", "title": "Search", "icon": "search", "nav": false }
        ],
        "actions": [
            { "id": "find_missing", "title": "Find episodes", "icon": "search", "contexts": ["show", "season"] },
            { "id": "find_movie", "title": "Find releases", "icon": "search", "contexts": ["movie"] }
        ]
    })
}

async fn register(app: &Shared) -> bool {
    let resp = app
        .http
        .post(format!("{}/api/plugins/register", app.cfg.ciniva_url))
        .bearer_auth(&app.cfg.secret)
        .json(&manifest(&app.cfg))
        .send()
        .await;
    match resp {
        Ok(r) if r.status().is_success() => {
            if let Ok(v) = r.json::<Value>().await {
                if let Some(s) = v["settings"].as_object() {
                    *app.settings.write().await = s.clone();
                }
            }
            true
        }
        Ok(r) => {
            tracing::warn!(status = %r.status(), "Ciniva refused registration");
            false
        }
        Err(e) => {
            tracing::warn!("cannot reach Ciniva: {e:#}");
            false
        }
    }
}

/// Rescans and notifies when a download completes. Torrents already complete at startup are only remembered.
async fn watch_completions(app: Shared) {
    let mut known: HashMap<String, bool> = HashMap::new();
    let mut primed = false;
    loop {
        if let Ok(list) = app.qbit.torrents().await {
            for t in &list {
                let was = known.insert(t.hash.clone(), t.complete());
                if primed && t.complete() && was == Some(false) {
                    tracing::info!(name = %t.name, "download finished");
                    if let Some(path) = app.cfg.to_ciniva(&t.content_path) {
                        app.rescan(&path).await;
                    }
                    app.notify("Download finished", &t.name, "success").await;
                }
            }
            primed = true;
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

async fn require_ciniva(State(app): State<Shared>, req: Request, next: Next) -> Response {
    let ok = !app.cfg.secret.is_empty() && req.headers().get("x-ciniva-secret").and_then(|v| v.to_str().ok()) == Some(app.cfg.secret.as_str());
    if ok {
        next.run(req).await
    } else {
        (StatusCode::FORBIDDEN, Json(json!({ "error": "only Ciniva may call this plugin" }))).into_response()
    }
}

async fn settings_changed(State(app): State<Shared>) -> StatusCode {
    register(&app).await;
    StatusCode::NO_CONTENT
}

async fn page(State(app): State<Shared>, Path(page): Path<String>, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    Json(ui::page(&app, &page, &q).await)
}

async fn action(State(app): State<Shared>, Path(action): Path<String>, Json(body): Json<Value>) -> Json<Value> {
    Json(ui::action(&app, &action, &body["context"]).await)
}

async fn call(State(app): State<Shared>, Path(path): Path<String>, body: Option<Json<Value>>) -> Json<Value> {
    Json(ui::handle(&app, &path, body.map(|b| b.0).unwrap_or(Value::Null)).await)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "ciniva_qbittorrent=info".into()))
        .init();
    let cfg = Config::from_env();
    if cfg.secret.is_empty() {
        anyhow::bail!("CINIVA_PLUGIN_SECRET is required");
    }
    let app: Shared = Arc::new(App {
        qbit: qbit::Qbit::new(&cfg.qbit_url),
        http: reqwest::Client::builder().timeout(Duration::from_secs(20)).build()?,
        settings: RwLock::new(Map::new()),
        cfg,
    });
    match app.qbit.version().await {
        Ok(v) => tracing::info!(qbittorrent = %v, "connected to qBittorrent"),
        Err(e) => tracing::warn!("qBittorrent not reachable yet: {e:#}"),
    }

    let heartbeat = app.clone();
    tokio::spawn(async move {
        let mut registered = false;
        loop {
            registered = register(&heartbeat).await || registered;
            // Ciniva may still be starting: retry quickly until the first registration succeeds.
            tokio::time::sleep(Duration::from_secs(if registered { 60 } else { 5 })).await;
        }
    });
    tokio::spawn(watch_completions(app.clone()));

    let ui_routes = Router::new()
        .route("/ui/pages/{page}", get(page))
        .route("/ui/actions/{action}", post(action))
        .route("/ui/{*path}", post(call))
        .route("/settings-changed", post(settings_changed))
        .layer(axum::extract::DefaultBodyLimit::max(25 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(app.clone(), require_ciniva));
    let router = Router::new().route("/health", get(|| async { "ok" })).merge(ui_routes).with_state(app.clone());
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", app.cfg.port)).await?;
    tracing::info!("listening on {}", listener.local_addr()?);
    axum::serve(listener, router).await?;
    Ok(())
}
