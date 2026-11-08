//! Plugin registry, settings, UI forwarding and the client event stream (protocol: docs/plugins.md).

use crate::{
    api::{ApiError, Shared},
    auth::Identity,
};
use axum::{
    body::{Body, Bytes},
    extract::{Path, RawQuery, State},
    http::{HeaderMap, Method, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{any, get, post},
    Extension, Json, Router,
};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::time::{Duration, Instant};

const OFFLINE_AFTER: Duration = Duration::from_secs(180);
const MASK: &str = "••••••••";

#[derive(Debug, Clone)]
pub struct Live {
    pub url: String,
    pub last_seen: Instant,
}

static HTTP: Lazy<reqwest::Client> =
    Lazy::new(|| reqwest::Client::builder().timeout(Duration::from_secs(60)).build().expect("http client"));

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/plugins", get(list))
        .route("/plugins/register", post(register))
        .route("/plugins/{id}/settings", get(get_settings).put(put_settings))
        .route("/plugins/{id}/ui/{*path}", any(forward).layer(axum::extract::DefaultBodyLimit::max(25 * 1024 * 1024)))
        .route("/notifications", post(notify))
        .route("/events", get(events))
}

fn forbidden() -> ApiError {
    ApiError(StatusCode::FORBIDDEN, "administrator only".into())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Saved values on top of each field's default.
fn effective_settings(manifest: &Value, saved: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    for f in manifest["settings"].as_array().into_iter().flatten() {
        if let Some(name) = f["name"].as_str() {
            let v = saved.get(name).cloned().or_else(|| f.get("value").cloned()).unwrap_or(Value::Null);
            out.insert(name.to_string(), v);
        }
    }
    out
}

async fn register(State(ctx): State<Shared>, Extension(id): Extension<Identity>, Json(manifest): Json<Value>) -> Result<Json<Value>, ApiError> {
    if !matches!(id, Identity::Service) {
        return Err(ApiError(StatusCode::FORBIDDEN, "plugins register with the service key".into()));
    }
    let pid = manifest["id"].as_str().unwrap_or_default().to_string();
    let url = manifest["url"].as_str().unwrap_or_default().trim_end_matches('/').to_string();
    if !valid_id(&pid) || !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "manifest needs an id (letters, digits, - or _) and an http(s) url".into()));
    }
    sqlx::query(
        "INSERT INTO plugins (id, manifest, last_seen) VALUES (?, ?, datetime('now'))
         ON CONFLICT (id) DO UPDATE SET manifest = excluded.manifest, last_seen = excluded.last_seen",
    )
    .bind(&pid)
    .bind(manifest.to_string())
    .execute(&ctx.db)
    .await?;
    let fresh = ctx.plugins.write().await.insert(pid.clone(), Live { url, last_seen: Instant::now() }).is_none();
    if fresh {
        tracing::info!(plugin = %pid, "plugin registered");
    }
    let saved: (String,) = sqlx::query_as("SELECT settings FROM plugins WHERE id = ?").bind(&pid).fetch_one(&ctx.db).await?;
    let saved: Value = serde_json::from_str(&saved.0).unwrap_or_default();
    Ok(Json(json!({ "settings": effective_settings(&manifest, &saved) })))
}

async fn list(State(ctx): State<Shared>) -> Result<Json<Vec<Value>>, ApiError> {
    let rows: Vec<(String, String, bool)> = sqlx::query_as("SELECT id, manifest, enabled FROM plugins ORDER BY id").fetch_all(&ctx.db).await?;
    let live = ctx.plugins.read().await;
    Ok(Json(
        rows.into_iter()
            .filter(|r| r.2)
            .map(|(id, manifest, _)| {
                let m: Value = serde_json::from_str(&manifest).unwrap_or_default();
                let online = live.get(&id).is_some_and(|l| l.last_seen.elapsed() < OFFLINE_AFTER);
                json!({
                    "id": id,
                    "name": m["name"],
                    "version": m["version"],
                    "description": m["description"],
                    "icon": m["icon"],
                    "online": online,
                    "pages": m["pages"].as_array().cloned().unwrap_or_default(),
                    "actions": m["actions"].as_array().cloned().unwrap_or_default(),
                    "has_settings": m["settings"].as_array().is_some_and(|s| !s.is_empty()),
                })
            })
            .collect(),
    ))
}

async fn load(ctx: &Shared, pid: &str) -> Result<(Value, Value), ApiError> {
    let row: Option<(String, String)> = sqlx::query_as("SELECT manifest, settings FROM plugins WHERE id = ?").bind(pid).fetch_optional(&ctx.db).await?;
    let (m, s) = row.ok_or_else(crate::api::not_found)?;
    Ok((serde_json::from_str(&m).unwrap_or_default(), serde_json::from_str(&s).unwrap_or_default()))
}

async fn get_settings(State(ctx): State<Shared>, Extension(id): Extension<Identity>, Path(pid): Path<String>) -> Result<Json<Value>, ApiError> {
    if !id.is_admin() {
        return Err(forbidden());
    }
    let (manifest, saved) = load(&ctx, &pid).await?;
    let values = effective_settings(&manifest, &saved);
    let fields: Vec<Value> = manifest["settings"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|f| {
            let mut f = f.clone();
            let name = f["name"].as_str().unwrap_or_default().to_string();
            let v = values.get(&name).cloned().unwrap_or(Value::Null);
            let secret = f["kind"] == "password" && v.as_str().is_some_and(|s| !s.is_empty());
            f["value"] = if secret { json!(MASK) } else { v };
            f
        })
        .collect();
    Ok(Json(json!({ "fields": fields })))
}

#[derive(Deserialize)]
struct SettingsBody {
    values: Map<String, Value>,
}

async fn put_settings(
    State(ctx): State<Shared>,
    Extension(id): Extension<Identity>,
    Path(pid): Path<String>,
    Json(b): Json<SettingsBody>,
) -> Result<StatusCode, ApiError> {
    if !id.is_admin() {
        return Err(forbidden());
    }
    let (manifest, saved) = load(&ctx, &pid).await?;
    let known: Vec<String> = manifest["settings"].as_array().into_iter().flatten().filter_map(|f| f["name"].as_str().map(str::to_string)).collect();
    let mut next = saved.as_object().cloned().unwrap_or_default();
    for (k, v) in b.values {
        if !known.contains(&k) || v.as_str() == Some(MASK) {
            continue;
        }
        next.insert(k, v);
    }
    sqlx::query("UPDATE plugins SET settings = ? WHERE id = ?").bind(Value::Object(next).to_string()).bind(&pid).execute(&ctx.db).await?;
    if let Some(url) = ctx.plugins.read().await.get(&pid).map(|l| l.url.clone()) {
        let secret = ctx.cfg.plugin_secret.clone().unwrap_or_default();
        let _ = HTTP.post(format!("{url}/settings-changed")).header("X-Ciniva-Secret", secret).send().await;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn forward(
    State(ctx): State<Shared>,
    Extension(id): Extension<Identity>,
    Path((pid, path)): Path<(String, String)>,
    RawQuery(query): RawQuery,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let url = ctx
        .plugins
        .read()
        .await
        .get(&pid)
        .filter(|l| l.last_seen.elapsed() < OFFLINE_AFTER)
        .map(|l| l.url.clone())
        .ok_or_else(|| ApiError(StatusCode::SERVICE_UNAVAILABLE, format!("plugin {pid} is not running")))?;
    if path.split('/').any(|seg| seg == "..") {
        return Err(ApiError(StatusCode::BAD_REQUEST, "bad path".into()));
    }
    let mut target = format!("{url}/ui/{path}");
    if let Some(q) = query.filter(|q| !q.is_empty()) {
        target.push('?');
        target.push_str(&q);
    }
    let (user, admin) = match &id {
        Identity::User(u) => (u.username.clone(), u.admin),
        Identity::Service => ("service".to_string(), true),
    };
    let mut req = HTTP
        .request(reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET), &target)
        .header("X-Ciniva-Secret", ctx.cfg.plugin_secret.clone().unwrap_or_default())
        .header("X-Ciniva-User", user)
        .header("X-Ciniva-Admin", if admin { "1" } else { "0" });
    if let Some(ct) = headers.get(axum::http::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()) {
        req = req.header("Content-Type", ct);
    }
    if !body.is_empty() {
        req = req.body(body);
    }
    let resp = req.send().await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, format!("plugin {pid}: {e}")))?;
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let ctype = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("application/json").to_string();
    let bytes = resp.bytes().await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, format!("plugin {pid}: {e}")))?;
    Ok((status, [(axum::http::header::CONTENT_TYPE, ctype)], Body::from(bytes)).into_response())
}

#[derive(Deserialize)]
struct Notification {
    title: String,
    body: Option<String>,
    tone: Option<String>,
    plugin: Option<String>,
}

async fn notify(State(ctx): State<Shared>, Extension(id): Extension<Identity>, Json(n): Json<Notification>) -> Result<StatusCode, ApiError> {
    if !id.is_admin() {
        return Err(forbidden());
    }
    ctx.emit("notification", json!({ "title": n.title, "body": n.body, "tone": n.tone.unwrap_or_else(|| "info".into()), "plugin": n.plugin }));
    Ok(StatusCode::NO_CONTENT)
}

async fn events(State(ctx): State<Shared>) -> impl IntoResponse {
    let rx = ctx.events.subscribe();
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok((kind, data)) => return Some((Ok::<_, std::convert::Infallible>(Event::default().event(kind).data(data.to_string())), rx)),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return None,
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(20)))
}
