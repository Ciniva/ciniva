use crate::{library::{self, Ctx}, probe::MediaInfo};
use axum::{
    body::Body,
    extract::{Path, Query, Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::{types::Json as SqlJson, FromRow};
use std::{collections::HashMap, sync::Arc};
use tower::ServiceExt;
use tower_http::services::ServeFile;

pub type Shared = Arc<Ctx>;

pub struct ApiError(pub StatusCode, pub String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        let e = e.into();
        if matches!(e.downcast_ref::<sqlx::Error>(), Some(sqlx::Error::RowNotFound)) {
            return ApiError(StatusCode::NOT_FOUND, "not found".into());
        }
        tracing::error!("{e:#}");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"))
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

pub fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "not found".into())
}

pub fn router(ctx: Shared) -> Router {
    let api = Router::new()
        .route("/status", get(status))
        .route("/scan", post(start_scan))
        .route("/home", get(home))
        .route("/movies", get(movies))
        .route("/movies/{id}", get(movie))
        .route("/shows", get(shows))
        .route("/shows/{id}", get(show))
        .route("/shows/{id}/seasons/{season}", get(season))
        .route("/search", get(search))
        .route("/unmatched", get(unmatched))
        .route("/tmdb/search", get(tmdb_search))
        .route("/files/{id}", get(file))
        .route("/files/{id}/match", post(match_file))
        .route("/files/{id}/stream", get(stream))
        .route("/files/{id}/progress", put(progress))
        .route("/files/{id}/watched", post(watched))
        .route("/images/{size}/{name}", get(image))
        .route("/hardware", get(hardware))
        .route("/files/{id}/transcode", post(transcode_start))
        .route("/files/{id}/hls.m3u8", get(transcode_playlist_direct))
        .route("/transcode/{sid}/index.m3u8", get(transcode_playlist))
        .route("/transcode/{sid}/{segment}", get(transcode_segment))
        .route("/transcode/{sid}", axum::routing::delete(transcode_stop))
        .route("/files/{id}/trickplay", get(trickplay_manifest))
        .route("/files/{id}/trickplay/{sheet}", get(trickplay_sheet))
        .route("/files/{id}/subtitles/{track}", get(subtitle))
        .route("/files/{id}/credits", put(set_credits))
        .merge(crate::account::private_routes())
        .merge(crate::plugins::routes())
        .layer(middleware::from_fn_with_state(ctx.clone(), auth))
        .merge(crate::account::public_routes());

    let mut app = Router::new().nest("/api", api).with_state(ctx.clone());
    if let Some(web) = ctx.cfg.web_dir.clone() {
        let index = web.join("index.html");
        let files = tower::ServiceBuilder::new()
            .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(header::CACHE_CONTROL, HeaderValue::from_static("no-cache")))
            .service(tower_http::services::ServeDir::new(web).fallback(ServeFile::new(index)));
        app = app.fallback_service(files);
    }
    app.layer(tower_http::cors::CorsLayer::permissive())
        .layer(tower_http::trace::TraceLayer::new_for_http())
}

/// Session token from the cookie, an `Authorization: Bearer` header, or `?token=` (for players that can only
/// fetch URLs).
pub fn request_token(req: &Request) -> Option<String> {
    let headers = req.headers();
    if let Some(t) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")) {
        return Some(t.trim().to_string());
    }
    if let Some(t) = req.uri().query().and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("token="))) {
        return Some(t.to_string());
    }
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|c| c.trim().strip_prefix(&format!("{}=", crate::auth::COOKIE)).map(str::to_string))
}

async fn auth(State(ctx): State<Shared>, mut req: Request, next: Next) -> Response {
    let Some(token) = request_token(&req) else {
        return ApiError(StatusCode::UNAUTHORIZED, "sign in required".into()).into_response();
    };
    let service = [ctx.cfg.plugin_secret.as_deref(), ctx.cfg.token.as_deref()].into_iter().flatten().any(|s| s == token);
    let identity = if service {
        Some(crate::auth::Identity::Service)
    } else {
        crate::auth::session_user(&ctx.db, &token).await.map(crate::auth::Identity::User)
    };
    match identity {
        Some(id) => {
            req.extensions_mut().insert(id);
            next.run(req).await
        }
        None => ApiError(StatusCode::UNAUTHORIZED, "session expired or invalid".into()).into_response(),
    }
}

#[derive(Serialize)]
struct Status {
    version: &'static str,
    tmdb: bool,
    scan: library::ScanStatus,
    counts: Counts,
}

#[derive(Serialize, FromRow)]
struct Counts {
    movies: i64,
    shows: i64,
    episodes: i64,
    unmatched: i64,
    pending: i64,
}

async fn status(State(ctx): State<Shared>) -> ApiResult<Status> {
    let counts: Counts = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM movies) movies, (SELECT COUNT(*) FROM shows) shows,
                (SELECT COUNT(DISTINCT episode_id) FROM file_episodes) episodes,
                (SELECT COUNT(*) FROM files WHERE kind = 'unmatched') unmatched,
                (SELECT COUNT(*) FROM files WHERE kind = 'pending') pending",
    )
    .fetch_one(&ctx.db)
    .await?;
    Ok(Json(Status { version: env!("CARGO_PKG_VERSION"), tmdb: ctx.tmdb().await.is_some(), scan: ctx.status.read().await.clone(), counts }))
}

#[derive(Deserialize, Default)]
struct ScanBody {
    #[serde(default)]
    paths: Vec<String>,
}

async fn start_scan(State(ctx): State<Shared>, body: Option<Json<ScanBody>>) -> StatusCode {
    let paths: Vec<std::path::PathBuf> = body.map(|b| b.0.paths).unwrap_or_default().into_iter().map(Into::into).collect();
    tokio::spawn(async move {
        let result = if paths.is_empty() { library::scan(&ctx).await } else { library::scan_paths(&ctx, paths).await };
        if let Err(e) = result {
            tracing::error!("scan failed: {e:#}");
        }
    });
    StatusCode::ACCEPTED
}

#[derive(Serialize, FromRow)]
struct Progress {
    position_ms: Option<i64>,
    duration_ms: Option<i64>,
    watched: Option<bool>,
}

#[derive(Serialize, FromRow)]
struct MovieCard {
    id: i64,
    title: String,
    year: Option<i64>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    rating: Option<f64>,
    added_at: String,
    #[sqlx(flatten)]
    #[serde(flatten)]
    progress: Progress,
}

const MOVIE_CARD: &str = "SELECT m.id, m.title, m.year, m.poster_path, m.backdrop_path, m.rating, m.added_at,
    p.position_ms, p.duration_ms, p.watched
    FROM movies m
    LEFT JOIN progress p ON p.file_id = (SELECT MIN(f.id) FROM files f WHERE f.movie_id = m.id)";

#[derive(Serialize, FromRow)]
struct ShowCard {
    id: i64,
    title: String,
    year: Option<i64>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    rating: Option<f64>,
    episodes: i64,
    watched_episodes: i64,
    last_added: Option<String>,
}

const SHOW_CARD: &str = "SELECT s.id, s.title, s.year, s.poster_path, s.backdrop_path, s.rating,
    COUNT(DISTINCT e.id) episodes,
    COUNT(DISTINCT CASE WHEN p.watched = 1 THEN e.id END) watched_episodes,
    MAX(f.added_at) last_added
    FROM shows s
    JOIN episodes e ON e.show_id = s.id
    JOIN file_episodes fe ON fe.episode_id = e.id
    JOIN files f ON f.id = fe.file_id
    LEFT JOIN progress p ON p.file_id = f.id";

#[derive(Deserialize)]
struct ListQuery {
    sort: Option<String>,
}

fn order(sort: Option<&str>, kind: &str) -> &'static str {
    match (sort.unwrap_or("title"), kind) {
        ("added", "movie") => " ORDER BY m.added_at DESC",
        ("added", _) => " ORDER BY last_added DESC",
        ("year", "movie") => " ORDER BY m.year DESC, m.title",
        ("year", _) => " ORDER BY s.year DESC, s.title",
        ("rating", "movie") => " ORDER BY m.rating DESC",
        ("rating", _) => " ORDER BY s.rating DESC",
        (_, "movie") => " ORDER BY m.title COLLATE NOCASE",
        _ => " ORDER BY s.title COLLATE NOCASE",
    }
}

async fn movies(State(ctx): State<Shared>, Query(q): Query<ListQuery>) -> ApiResult<Vec<MovieCard>> {
    let sql = format!("{MOVIE_CARD}{}", order(q.sort.as_deref(), "movie"));
    Ok(Json(sqlx::query_as(&sql).fetch_all(&ctx.db).await?))
}

async fn shows(State(ctx): State<Shared>, Query(q): Query<ListQuery>) -> ApiResult<Vec<ShowCard>> {
    let sql = format!("{SHOW_CARD} GROUP BY s.id{}", order(q.sort.as_deref(), "show"));
    Ok(Json(sqlx::query_as(&sql).fetch_all(&ctx.db).await?))
}

#[derive(Serialize, FromRow)]
struct FileView {
    id: i64,
    path: String,
    size: i64,
    duration: Option<f64>,
    probe: Option<SqlJson<MediaInfo>>,
    added_at: String,
    credits_start: Option<f64>,
    credits_source: Option<String>,
    #[sqlx(flatten)]
    #[serde(flatten)]
    progress: Progress,
}

const FILE_VIEW: &str = "SELECT f.id, f.path, f.size, f.duration, f.probe, f.added_at, f.credits_start, f.credits_source,
    p.position_ms, p.duration_ms, p.watched
    FROM files f LEFT JOIN progress p ON p.file_id = f.id";

#[derive(Serialize, FromRow)]
struct MovieView {
    id: i64,
    tmdb_id: i64,
    title: String,
    original_title: Option<String>,
    year: Option<i64>,
    overview: Option<String>,
    tagline: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    runtime: Option<i64>,
    rating: Option<f64>,
    genres: SqlJson<Vec<String>>,
    release_date: Option<String>,
    #[sqlx(rename = "cast_json")]
    cast: SqlJson<serde_json::Value>,
    #[sqlx(skip)]
    files: Vec<FileView>,
}

async fn movie(State(ctx): State<Shared>, Path(id): Path<i64>) -> ApiResult<MovieView> {
    let mut m: MovieView = sqlx::query_as("SELECT * FROM movies WHERE id = ?").bind(id).fetch_one(&ctx.db).await?;
    m.files = sqlx::query_as(&format!("{FILE_VIEW} WHERE f.movie_id = ? ORDER BY f.size DESC"))
        .bind(id)
        .fetch_all(&ctx.db)
        .await?;
    Ok(Json(m))
}

#[derive(Serialize, FromRow)]
struct ShowView {
    id: i64,
    tmdb_id: i64,
    title: String,
    original_title: Option<String>,
    year: Option<i64>,
    overview: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    rating: Option<f64>,
    genres: SqlJson<Vec<String>>,
    first_air_date: Option<String>,
    status: Option<String>,
    #[sqlx(rename = "cast_json")]
    cast: SqlJson<serde_json::Value>,
    #[sqlx(skip)]
    seasons: Vec<SeasonCard>,
}

#[derive(Serialize, FromRow)]
struct SeasonCard {
    season_number: i64,
    name: Option<String>,
    poster_path: Option<String>,
    air_date: Option<String>,
    episode_count: Option<i64>,
    available: i64,
    watched: i64,
}

async fn show(State(ctx): State<Shared>, Path(id): Path<i64>) -> ApiResult<ShowView> {
    let mut s: ShowView = sqlx::query_as("SELECT * FROM shows WHERE id = ?").bind(id).fetch_one(&ctx.db).await?;
    s.seasons = sqlx::query_as(
        "SELECT se.season_number, se.name, se.poster_path, se.air_date, se.episode_count,
            COUNT(DISTINCT fe.episode_id) available,
            COUNT(DISTINCT CASE WHEN p.watched = 1 THEN e.id END) watched
         FROM seasons se
         JOIN episodes e ON e.show_id = se.show_id AND e.season_number = se.season_number
         LEFT JOIN file_episodes fe ON fe.episode_id = e.id
         LEFT JOIN progress p ON p.file_id = fe.file_id
         WHERE se.show_id = ?
         GROUP BY se.id HAVING available > 0
         ORDER BY se.season_number = 0, se.season_number",
    )
    .bind(id)
    .fetch_all(&ctx.db)
    .await?;
    Ok(Json(s))
}

#[derive(Serialize, FromRow)]
struct EpisodeView {
    id: i64,
    season_number: i64,
    episode_number: i64,
    title: Option<String>,
    overview: Option<String>,
    still_path: Option<String>,
    air_date: Option<String>,
    runtime: Option<i64>,
    rating: Option<f64>,
    #[sqlx(skip)]
    files: Vec<FileView>,
}

async fn season(State(ctx): State<Shared>, Path((id, season)): Path<(i64, i64)>) -> ApiResult<Vec<EpisodeView>> {
    let mut episodes: Vec<EpisodeView> = sqlx::query_as(
        "SELECT id, season_number, episode_number, title, overview, still_path, air_date, runtime, rating
         FROM episodes WHERE show_id = ? AND season_number = ? ORDER BY episode_number",
    )
    .bind(id)
    .bind(season)
    .fetch_all(&ctx.db)
    .await?;
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT fe.episode_id, fe.file_id FROM file_episodes fe JOIN episodes e ON e.id = fe.episode_id
         WHERE e.show_id = ? AND e.season_number = ?",
    )
    .bind(id)
    .bind(season)
    .fetch_all(&ctx.db)
    .await?;
    let mut by_episode: HashMap<i64, Vec<i64>> = HashMap::new();
    for (eid, fid) in rows {
        by_episode.entry(eid).or_default().push(fid);
    }
    for e in &mut episodes {
        for fid in by_episode.get(&e.id).into_iter().flatten() {
            if let Some(f) = sqlx::query_as::<_, FileView>(&format!("{FILE_VIEW} WHERE f.id = ?")).bind(fid).fetch_optional(&ctx.db).await? {
                e.files.push(f);
            }
        }
    }
    Ok(Json(episodes))
}

#[derive(Serialize, FromRow)]
struct ContinueItem {
    file_id: i64,
    position_ms: i64,
    duration_ms: i64,
    updated_at: String,
    movie_id: Option<i64>,
    show_id: Option<i64>,
    title: String,
    subtitle: Option<String>,
    image_path: Option<String>,
    poster_path: Option<String>,
    /// The episode after the last one finished, rather than something partly watched.
    #[sqlx(default)]
    next_up: bool,
}

/// For each show, the first unwatched episode in the library after the most recently finished one.
const UP_NEXT: &str = "WITH last AS (
        SELECT e.show_id, e.season_number sn, e.episode_number en, p.updated_at,
            ROW_NUMBER() OVER (PARTITION BY e.show_id ORDER BY p.updated_at DESC, e.season_number DESC, e.episode_number DESC) rn
        FROM progress p
        JOIN file_episodes fe ON fe.file_id = p.file_id
        JOIN episodes e ON e.id = fe.episode_id
        WHERE p.watched = 1 AND e.season_number > 0
    )
    SELECT MIN(f.id) file_id, 0 position_ms, CAST(COALESCE(MIN(f.duration), 0) * 1000 AS INTEGER) duration_ms, last.updated_at,
        NULL movie_id, s.id show_id, s.title,
        'S' || e.season_number || ' · E' || e.episode_number || COALESCE(' — ' || e.title, '') subtitle,
        COALESCE(e.still_path, s.backdrop_path) image_path, s.poster_path, 1 next_up
    FROM last
    JOIN shows s ON s.id = last.show_id
    JOIN episodes e ON e.id = (
        SELECT e2.id FROM episodes e2
        JOIN file_episodes fe2 ON fe2.episode_id = e2.id
        LEFT JOIN progress p2 ON p2.file_id = fe2.file_id
        WHERE e2.show_id = last.show_id AND e2.season_number > 0
          AND (e2.season_number > last.sn OR (e2.season_number = last.sn AND e2.episode_number > last.en))
        GROUP BY e2.id
        HAVING MAX(COALESCE(p2.watched, 0)) = 0
        ORDER BY e2.season_number, e2.episode_number
        LIMIT 1)
    JOIN file_episodes fe ON fe.episode_id = e.id
    JOIN files f ON f.id = fe.file_id
    WHERE last.rn = 1
    GROUP BY s.id
    ORDER BY last.updated_at DESC
    LIMIT 20";

#[derive(Serialize)]
struct Home {
    continue_watching: Vec<ContinueItem>,
    recent_movies: Vec<MovieCard>,
    recent_shows: Vec<ShowCard>,
}

async fn home(State(ctx): State<Shared>) -> ApiResult<Home> {
    let partial: Vec<ContinueItem> = sqlx::query_as(
        "SELECT p.file_id, p.position_ms, p.duration_ms, p.updated_at,
            m.id movie_id, s.id show_id,
            COALESCE(m.title, s.title) title,
            CASE WHEN e.id IS NOT NULL THEN 'S' || e.season_number || ' · E' || e.episode_number || COALESCE(' — ' || e.title, '') END subtitle,
            COALESCE(e.still_path, m.backdrop_path, s.backdrop_path) image_path,
            COALESCE(m.poster_path, s.poster_path) poster_path
         FROM progress p
         JOIN files f ON f.id = p.file_id
         LEFT JOIN movies m ON m.id = f.movie_id
         LEFT JOIN file_episodes fe ON fe.file_id = f.id
         LEFT JOIN episodes e ON e.id = fe.episode_id
         LEFT JOIN shows s ON s.id = e.show_id
         WHERE p.watched = 0 AND p.position_ms > 30000 AND (m.id IS NOT NULL OR s.id IS NOT NULL)
         GROUP BY p.file_id
         ORDER BY p.updated_at DESC LIMIT 20",
    )
    .fetch_all(&ctx.db)
    .await?;
    let up_next: Vec<ContinueItem> = sqlx::query_as(UP_NEXT).fetch_all(&ctx.db).await?;
    // One entry per show: an episode in progress wins over the next one.
    let busy: std::collections::HashSet<i64> = partial.iter().filter_map(|c| c.show_id).collect();
    let mut continue_watching: Vec<ContinueItem> = partial.into_iter().chain(up_next.into_iter().filter(|c| c.show_id.is_none_or(|s| !busy.contains(&s)))).collect();
    continue_watching.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    continue_watching.truncate(20);
    let recent_movies = sqlx::query_as(&format!("{MOVIE_CARD} ORDER BY m.added_at DESC LIMIT 20")).fetch_all(&ctx.db).await?;
    let recent_shows = sqlx::query_as(&format!("{SHOW_CARD} GROUP BY s.id ORDER BY last_added DESC LIMIT 20")).fetch_all(&ctx.db).await?;
    Ok(Json(Home { continue_watching, recent_movies, recent_shows }))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

#[derive(Serialize)]
struct SearchResults {
    movies: Vec<MovieCard>,
    shows: Vec<ShowCard>,
}

async fn search(State(ctx): State<Shared>, Query(q): Query<SearchQuery>) -> ApiResult<SearchResults> {
    let like = format!("%{}%", q.q.trim());
    let movies = sqlx::query_as(&format!("{MOVIE_CARD} WHERE m.title LIKE ?1 OR m.original_title LIKE ?1 ORDER BY m.rating DESC LIMIT 40"))
        .bind(&like)
        .fetch_all(&ctx.db)
        .await?;
    let shows = sqlx::query_as(&format!("{SHOW_CARD} WHERE s.title LIKE ?1 OR s.original_title LIKE ?1 GROUP BY s.id ORDER BY s.rating DESC LIMIT 40"))
        .bind(&like)
        .fetch_all(&ctx.db)
        .await?;
    Ok(Json(SearchResults { movies, shows }))
}

#[derive(Serialize, FromRow)]
struct UnmatchedFile {
    id: i64,
    path: String,
    size: i64,
    duration: Option<f64>,
    guess: SqlJson<serde_json::Value>,
    match_note: Option<String>,
}

async fn unmatched(State(ctx): State<Shared>) -> ApiResult<Vec<UnmatchedFile>> {
    Ok(Json(
        sqlx::query_as("SELECT id, path, size, duration, guess, match_note FROM files WHERE kind IN ('unmatched', 'pending') ORDER BY path")
            .fetch_all(&ctx.db)
            .await?,
    ))
}

#[derive(Deserialize)]
struct TmdbSearchQuery {
    q: String,
    kind: String,
    year: Option<i32>,
}

async fn tmdb_search(State(ctx): State<Shared>, Query(q): Query<TmdbSearchQuery>) -> ApiResult<serde_json::Value> {
    let tmdb = ctx.tmdb().await.ok_or_else(|| ApiError(StatusCode::SERVICE_UNAVAILABLE, "TMDB is not configured".into()))?;
    let value = if q.kind == "movie" {
        serde_json::to_value(tmdb.search_movie(&q.q, q.year).await?.into_iter().take(10).map(|s| s.hit).map(|h| {
            serde_json::json!({ "id": h.id, "title": h.title, "original_title": h.original_title, "year": crate::tmdb::year_of(&h.release_date), "poster_path": h.poster_path, "overview": h.overview })
        }).collect::<Vec<_>>())?
    } else {
        serde_json::to_value(tmdb.search_tv(&q.q, q.year).await?.into_iter().take(10).map(|s| s.hit).map(|h| {
            serde_json::json!({ "id": h.id, "title": h.name, "original_title": h.original_name, "year": crate::tmdb::year_of(&h.first_air_date), "poster_path": h.poster_path, "overview": h.overview })
        }).collect::<Vec<_>>())?
    };
    Ok(Json(value))
}

async fn file(State(ctx): State<Shared>, Path(id): Path<i64>) -> ApiResult<FileView> {
    Ok(Json(sqlx::query_as(&format!("{FILE_VIEW} WHERE f.id = ?")).bind(id).fetch_one(&ctx.db).await?))
}

#[derive(Deserialize)]
struct MatchBody {
    kind: String,
    tmdb_id: i64,
    season: Option<i64>,
    episode: Option<i64>,
}

async fn match_file(State(ctx): State<Shared>, Path(id): Path<i64>, Json(b): Json<MatchBody>) -> ApiResult<serde_json::Value> {
    if ctx.tmdb().await.is_none() {
        return Err(ApiError(StatusCode::SERVICE_UNAVAILABLE, "TMDB is not configured".into()));
    }
    if b.kind != "movie" && (b.season.is_none() || b.episode.is_none()) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "season and episode are required for shows".into()));
    }
    let ok = library::manual_match(&ctx, id, &b.kind, b.tmdb_id, b.season, b.episode).await?;
    Ok(Json(serde_json::json!({ "matched": ok })))
}

async fn stream(State(ctx): State<Shared>, Path(id): Path<i64>, req: Request) -> Result<Response, ApiError> {
    let (path,): (String,) = sqlx::query_as("SELECT path FROM files WHERE id = ?").bind(id).fetch_optional(&ctx.db).await?.ok_or_else(not_found)?;
    let mime = match std::path::Path::new(&path).extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("mp4" | "m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("mkv") => "video/x-matroska",
        Some("webm") => "video/webm",
        Some("ts" | "m2ts") => "video/mp2t",
        Some("avi") => "video/x-msvideo",
        _ => "application/octet-stream",
    };
    let mut resp = ServeFile::new_with_mime(&path, &mime.parse().unwrap())
        .oneshot(req)
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map(Body::new);
    resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// Finished at 90 %, 2 minutes before the end, or where the credits start, whichever comes first.
fn is_watched(position_ms: i64, duration_ms: i64, credits: Option<f64>) -> bool {
    if duration_ms <= 0 {
        return false;
    }
    let pct = duration_ms as f64 * 0.9;
    let tail = if duration_ms > 240_000 { (duration_ms - 120_000) as f64 } else { pct };
    let credits = credits.map(|c| c * 1000.0).filter(|c| *c >= duration_ms as f64 * 0.5).unwrap_or(f64::MAX);
    position_ms as f64 >= pct.min(tail).min(credits)
}

#[derive(Deserialize)]
struct ProgressBody {
    position_ms: i64,
    duration_ms: i64,
}

async fn progress(State(ctx): State<Shared>, Path(id): Path<i64>, Json(b): Json<ProgressBody>) -> Result<StatusCode, ApiError> {
    let credits: Option<(Option<f64>,)> = sqlx::query_as("SELECT credits_start FROM files WHERE id = ?").bind(id).fetch_optional(&ctx.db).await?;
    let watched = is_watched(b.position_ms, b.duration_ms, credits.and_then(|c| c.0));
    sqlx::query(
        "INSERT INTO progress (file_id, position_ms, duration_ms, watched, updated_at) VALUES (?, ?, ?, ?, datetime('now'))
         ON CONFLICT (file_id) DO UPDATE SET position_ms = excluded.position_ms, duration_ms = excluded.duration_ms,
           watched = MAX(progress.watched, excluded.watched), updated_at = excluded.updated_at",
    )
    .bind(id)
    .bind(if watched { 0 } else { b.position_ms })
    .bind(b.duration_ms)
    .bind(watched)
    .execute(&ctx.db)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct WatchedBody {
    watched: bool,
}

async fn watched(State(ctx): State<Shared>, Path(id): Path<i64>, Json(b): Json<WatchedBody>) -> Result<StatusCode, ApiError> {
    sqlx::query(
        "INSERT INTO progress (file_id, position_ms, duration_ms, watched, updated_at) VALUES (?, 0, 0, ?, datetime('now'))
         ON CONFLICT (file_id) DO UPDATE SET watched = excluded.watched, position_ms = 0, updated_at = excluded.updated_at",
    )
    .bind(id)
    .bind(b.watched)
    .execute(&ctx.db)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn image(State(ctx): State<Shared>, Path((size, name)): Path<(String, String)>) -> Result<Response, ApiError> {
    const SIZES: &[&str] = &["w92", "w154", "w185", "w300", "w342", "w500", "w780", "w1280", "original"];
    let valid_name = name.len() < 100
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        && !name.starts_with('.')
        && (name.ends_with(".jpg") || name.ends_with(".png") || name.ends_with(".svg"));
    if !SIZES.contains(&size.as_str()) || !valid_name {
        return Err(not_found());
    }
    let dir = ctx.cfg.data_dir.join("images").join(&size);
    let file = dir.join(&name);
    if !file.exists() {
        let url = format!("https://image.tmdb.org/t/p/{size}/{name}");
        let resp = reqwest::get(&url).await?;
        if !resp.status().is_success() {
            return Err(not_found());
        }
        let bytes = resp.bytes().await?;
        tokio::fs::create_dir_all(&dir).await?;
        let tmp = dir.join(format!(".{name}.tmp"));
        tokio::fs::write(&tmp, &bytes).await?;
        tokio::fs::rename(&tmp, &file).await?;
    }
    let mime = if name.ends_with(".png") { "image/png" } else if name.ends_with(".svg") { "image/svg+xml" } else { "image/jpeg" };
    let bytes = tokio::fs::read(&file).await?;
    Ok((
        [(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "public, max-age=31536000, immutable")],
        bytes,
    )
        .into_response())
}

async fn hardware(State(ctx): State<Shared>) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::json!({
        "devices": ctx.hw.devices,
        "transcodes": ctx.transcoder.active().await,
    })))
}

fn token_query(req_query: Option<&str>) -> String {
    req_query
        .and_then(|q| q.split('&').find(|kv| kv.starts_with("token=")))
        .map(|kv| format!("?{kv}"))
        .unwrap_or_default()
}

async fn start_session(ctx: &Shared, id: i64, request: crate::transcode::Request) -> Result<Arc<crate::transcode::Session>, ApiError> {
    let (path, probe): (String, Option<SqlJson<MediaInfo>>) =
        sqlx::query_as("SELECT path, probe FROM files WHERE id = ?").bind(id).fetch_optional(&ctx.db).await?.ok_or_else(not_found)?;
    let media = probe.ok_or_else(|| ApiError(StatusCode::CONFLICT, "file has not been probed yet".into()))?.0;
    Ok(ctx.transcoder.start(id, path.into(), media, request).await?)
}

async fn transcode_start(
    State(ctx): State<Shared>,
    Path(id): Path<i64>,
    Json(request): Json<crate::transcode::Request>,
) -> ApiResult<serde_json::Value> {
    let s = start_session(&ctx, id, request).await?;
    Ok(Json(serde_json::json!({
        "session": s.id,
        "playlist": format!("/api/transcode/{}/index.m3u8", s.id),
        "plan": s.plan,
    })))
}

fn m3u8(body: String) -> Response {
    ([(header::CONTENT_TYPE, "application/vnd.apple.mpegurl"), (header::CACHE_CONTROL, "no-store")], body).into_response()
}

async fn transcode_playlist_direct(
    State(ctx): State<Shared>,
    Path(id): Path<i64>,
    Query(request): Query<crate::transcode::Request>,
    uri: axum::http::Uri,
) -> Result<Response, ApiError> {
    let s = start_session(&ctx, id, request).await?;
    Ok(m3u8(s.playlist(&format!("/api/transcode/{}/", s.id), &token_query(uri.query()))))
}

async fn transcode_playlist(State(ctx): State<Shared>, Path(sid): Path<String>, uri: axum::http::Uri) -> Result<Response, ApiError> {
    let s = ctx.transcoder.get(&sid).await.ok_or_else(not_found)?;
    Ok(m3u8(s.playlist("", &token_query(uri.query()))))
}

async fn transcode_segment(State(ctx): State<Shared>, Path((sid, segment)): Path<(String, String)>) -> Result<Response, ApiError> {
    let s = ctx.transcoder.get(&sid).await.ok_or_else(not_found)?;
    if segment == "init.mp4" {
        let bytes = s.init_segment().await?;
        return Ok(([(header::CONTENT_TYPE, "video/mp4"), (header::CACHE_CONTROL, "no-store")], bytes.to_vec()).into_response());
    }
    let n: u64 = segment
        .strip_suffix(".ts")
        .or_else(|| segment.strip_suffix(".m4s"))
        .and_then(|n| n.parse().ok())
        .ok_or_else(not_found)?;
    let bytes = s.media_segment(n).await?;
    let ctype = s.content_type();
    Ok(([(header::CONTENT_TYPE, ctype), (header::CACHE_CONTROL, "no-store")], bytes).into_response())
}

async fn transcode_stop(State(ctx): State<Shared>, Path(sid): Path<String>) -> StatusCode {
    ctx.transcoder.stop(&sid).await;
    StatusCode::NO_CONTENT
}

async fn trickplay_manifest(State(ctx): State<Shared>, Path(id): Path<i64>) -> Result<Response, ApiError> {
    let path = crate::transcode::trickplay::dir(&ctx.cfg.data_dir, id).join("manifest.json");
    let bytes = tokio::fs::read(&path).await.map_err(|_| not_found())?;
    Ok(([(header::CONTENT_TYPE, "application/json")], bytes).into_response())
}

async fn trickplay_sheet(State(ctx): State<Shared>, Path((id, sheet)): Path<(i64, String)>) -> Result<Response, ApiError> {
    let n: u32 = sheet.strip_suffix(".jpg").and_then(|n| n.parse().ok()).ok_or_else(not_found)?;
    let path = crate::transcode::trickplay::dir(&ctx.cfg.data_dir, id).join(format!("{n}.jpg"));
    let bytes = tokio::fs::read(&path).await.map_err(|_| not_found())?;
    Ok(([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "public, max-age=86400")], bytes).into_response())
}

const TEXT_SUBS: &[&str] = &["subrip", "srt", "ass", "ssa", "mov_text", "webvtt", "text"];
static SUBTITLE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn subtitle(State(ctx): State<Shared>, Path((id, track)): Path<(i64, String)>) -> Result<Response, ApiError> {
    let index: i64 = track.strip_suffix(".vtt").and_then(|n| n.parse().ok()).ok_or_else(not_found)?;
    let (path, probe): (String, Option<SqlJson<MediaInfo>>) =
        sqlx::query_as("SELECT path, probe FROM files WHERE id = ?").bind(id).fetch_optional(&ctx.db).await?.ok_or_else(not_found)?;
    let subs = probe.map(|p| p.0.subtitles).unwrap_or_default();
    let sub = subs.iter().find(|s| s.index == index).ok_or_else(not_found)?;
    if !TEXT_SUBS.contains(&sub.codec.as_str()) {
        return Err(ApiError(StatusCode::UNPROCESSABLE_ENTITY, format!("{} subtitles are images and cannot be converted to text", sub.codec)));
    }
    let dir = ctx.cfg.data_dir.join("subtitles");
    let vtt = |i: i64| dir.join(format!("{id}-{i}.vtt"));
    if !vtt(index).exists() {
        let _guard = SUBTITLE_LOCK.lock().await;
        if !vtt(index).exists() {
            // Extracting means reading the whole file, so pull every text track out in a single pass.
            tokio::fs::create_dir_all(&dir).await?;
            let tracks: Vec<i64> = subs.iter().filter(|s| TEXT_SUBS.contains(&s.codec.as_str())).map(|s| s.index).collect();
            let tmp = |i: i64| dir.join(format!(".{id}-{i}.vtt.tmp"));
            let mut cmd = tokio::process::Command::new(&ctx.cfg.ffmpeg);
            cmd.args(["-v", "error", "-nostdin", "-y", "-i"]).arg(&path);
            for &i in &tracks {
                cmd.args(["-map", &format!("0:{i}"), "-c:s", "webvtt", "-f", "webvtt"]).arg(tmp(i));
            }
            let out = cmd.output().await?;
            for &i in &tracks {
                if out.status.success() {
                    let _ = tokio::fs::rename(tmp(i), vtt(i)).await;
                } else {
                    let _ = tokio::fs::remove_file(tmp(i)).await;
                }
            }
            if !out.status.success() {
                return Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR, String::from_utf8_lossy(&out.stderr).trim().to_string()));
            }
        }
    }
    let bytes = tokio::fs::read(vtt(index)).await?;
    Ok(([(header::CONTENT_TYPE, "text/vtt; charset=utf-8"), (header::CACHE_CONTROL, "no-store")], bytes).into_response())
}

#[derive(Deserialize)]
struct CreditsBody {
    /// "manual" pins `start` (null means the file has no credits); "auto" clears it and detects again.
    mode: String,
    start: Option<f64>,
}

async fn set_credits(State(ctx): State<Shared>, Path(id): Path<i64>, Json(b): Json<CreditsBody>) -> Result<StatusCode, ApiError> {
    let q = if b.mode == "auto" {
        sqlx::query("UPDATE files SET credits_start = NULL, credits_source = NULL, credits_version = 0 WHERE id = ?").bind(id)
    } else {
        sqlx::query("UPDATE files SET credits_start = ?, credits_source = 'manual', credits_version = ? WHERE id = ?")
            .bind(b.start.filter(|s| *s > 0.0))
            .bind(crate::credits::MANUAL)
            .bind(id)
    };
    if q.execute(&ctx.db).await?.rows_affected() == 0 {
        return Err(not_found());
    }
    if b.mode == "auto" {
        let ctx = ctx.clone();
        tokio::spawn(async move { library::credits_pass(&ctx).await });
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::is_watched;

    #[test]
    fn watched_threshold() {
        let min = 60_000;
        assert!(!is_watched(12 * min + 59_000, 15 * min, None));
        assert!(is_watched(13 * min, 15 * min, None));
        assert!(!is_watched(152 * min, 169 * min, None));
        assert!(is_watched(153 * min, 169 * min, None));
        assert!(!is_watched(100_000, 180_000, None));
        assert!(is_watched(162_000, 180_000, None));
        assert!(is_watched(12 * min, 15 * min, Some(12.0 * 60.0)));
        assert!(!is_watched(5 * min, 15 * min, Some(4.0 * 60.0)));
    }
}
