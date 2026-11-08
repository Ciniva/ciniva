//! Declarative views (docs/plugins.md) for downloads, a torrent's details, and search.

use crate::{
    prowlarr::{self, Kind},
    qbit::{NewTorrent, Torrent},
    release, Shared,
};
use base64::Engine;
use serde_json::{json, Value};
use std::collections::HashMap;

fn bytes(n: i64) -> String {
    let n = n.max(0) as f64;
    let (v, u) = if n >= 1e12 {
        (n / 1e12, "TB")
    } else if n >= 1e9 {
        (n / 1e9, "GB")
    } else if n >= 1e6 {
        (n / 1e6, "MB")
    } else {
        (n / 1e3, "KB")
    };
    if v >= 100.0 { format!("{v:.0} {u}") } else { format!("{v:.1} {u}") }
}

fn speed(n: i64) -> String {
    format!("{}/s", bytes(n))
}

fn eta(secs: i64) -> Option<String> {
    if !(1..8_640_000).contains(&secs) {
        return None;
    }
    Some(if secs >= 3600 {
        format!("{}h {}m", secs / 3600, secs / 60 % 60)
    } else if secs >= 60 {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    })
}

fn state_badge(t: &Torrent) -> (&'static str, &'static str) {
    match t.state.as_str() {
        "error" | "missingFiles" => ("Error", "error"),
        "downloading" | "forcedDL" | "metaDL" | "forcedMetaDL" => ("Downloading", "info"),
        "stalledDL" => ("Waiting for peers", "warning"),
        "queuedDL" => ("Queued", "warning"),
        "checkingDL" | "checkingUP" | "checkingResumeData" | "moving" => ("Checking", "warning"),
        "uploading" | "forcedUP" => ("Seeding", "success"),
        "stalledUP" | "queuedUP" => ("Seeding (idle)", "success"),
        s if s.starts_with("stopped") || s.starts_with("paused") => (if t.complete() { "Complete" } else { "Paused" }, if t.complete() { "success" } else { "warning" }),
        _ => ("Unknown", "info"),
    }
}

fn op(method: &str, path: &str, body: Value) -> Value {
    json!({ "method": method, "path": path, "body": body })
}

fn error_view(title: &str, text: &str) -> Value {
    json!({ "title": title, "blocks": [{ "type": "text", "text": text, "tone": "error" }] })
}

fn in_downloads(app: &Shared, t: &Torrent) -> bool {
    app.cfg.downloads_root().is_some_and(|root| t.save_path.trim_end_matches('/').starts_with(root))
}

fn torrent_actions(app: &Shared, t: &Torrent) -> Vec<Value> {
    let mut a = vec![];
    if t.paused() {
        a.push(json!({ "label": "Resume", "call": op("POST", &format!("torrents/{}/start", t.hash), Value::Null) }));
    } else {
        a.push(json!({ "label": "Pause", "call": op("POST", &format!("torrents/{}/stop", t.hash), Value::Null) }));
    }
    if !t.complete() {
        let (label, verb) = if t.seq_dl { ("Normal order", "sequential_off") } else { ("Sequential", "sequential_on") };
        a.push(json!({ "label": label, "call": op("POST", &format!("torrents/{}/{verb}", t.hash), Value::Null) }));
    }
    a.push(json!({ "label": "Remove", "confirm": format!("Remove “{}” from qBittorrent? The files stay.", t.name), "call": op("POST", &format!("torrents/{}/delete", t.hash), Value::Null) }));
    if in_downloads(app, t) {
        a.push(json!({ "label": "Delete files", "tone": "danger", "confirm": format!("Remove “{}” and delete its downloaded files?", t.name),
                       "call": op("POST", &format!("torrents/{}/delete_files", t.hash), Value::Null) }));
    }
    a
}

fn torrent_item(app: &Shared, t: &Torrent) -> Value {
    let (state, tone) = state_badge(t);
    let mut sub = vec![bytes(t.size)];
    if !t.complete() {
        sub.push(format!("{:.0}%", t.progress * 100.0));
        if t.dlspeed > 0 {
            sub.push(format!("↓ {}", speed(t.dlspeed)));
        }
        if let Some(e) = eta(t.eta) {
            sub.push(format!("{e} left"));
        }
    }
    if t.upspeed > 0 {
        sub.push(format!("↑ {}", speed(t.upspeed)));
    }
    sub.push(format!("ratio {:.2}", t.ratio));
    let mut badges = vec![json!({ "text": state, "tone": tone })];
    if t.seq_dl && !t.complete() {
        badges.push(json!({ "text": "Sequential", "tone": "info" }));
    }
    json!({
        "id": t.hash,
        "title": t.name,
        "subtitle": sub.join(" · "),
        "progress": if t.complete() { Value::Null } else { json!(t.progress) },
        "badges": badges,
        "actions": torrent_actions(app, t),
        "link": { "page": "torrent", "params": { "hash": t.hash } },
    })
}

async fn downloads(app: &Shared) -> Value {
    let (list, transfer) = match tokio::join!(app.qbit.torrents(), app.qbit.transfer()) {
        (Ok(l), Ok(t)) => (l, t),
        (Err(e), _) | (_, Err(e)) => return error_view("Downloads", &format!("qBittorrent is not reachable: {e:#}")),
    };
    let active: Vec<&Torrent> = list.iter().filter(|t| !t.complete()).collect();
    let seeding = list.iter().filter(|t| t.complete() && !t.paused()).count();
    let mut items: Vec<Value> = active.iter().map(|t| torrent_item(app, t)).collect();
    items.extend(list.iter().filter(|t| t.complete()).take(150).map(|t| torrent_item(app, t)));
    json!({
        "title": "Downloads",
        "subtitle": format!("qBittorrent · {}", if transfer.connection_status == "connected" { "connected" } else { transfer.connection_status.as_str() }),
        "refresh": 3,
        "blocks": [
            { "type": "stats", "items": [
                { "label": "Download", "value": speed(transfer.dl_info_speed) },
                { "label": "Upload", "value": speed(transfer.up_info_speed) },
                { "label": "Downloading", "value": active.len() },
                { "label": "Seeding", "value": seeding },
            ]},
            { "type": "actions", "items": [{ "label": "Search releases", "icon": "search", "navigate": { "page": "search" } }] },
            { "type": "form", "id": "add", "fields": [
                { "name": "source", "label": "Magnet link or torrent URL", "kind": "text", "placeholder": "magnet:?xt=urn:btih:…" },
                { "name": "torrent", "label": "…or a .torrent file", "kind": "file" },
                { "name": "watch", "label": "Download in order", "kind": "toggle", "help": "Sequential, so playback can start sooner" },
            ], "submit": { "label": "Add download", "call": op("POST", "add", json!({})) } },
            { "type": "list", "items": items, "empty": "No torrents yet." },
        ]
    })
}

async fn torrent_page(app: &Shared, hash: &str) -> Value {
    let t = match app.qbit.torrent(hash).await {
        Ok(Some(t)) => t,
        Ok(None) => return error_view("Torrent", "This torrent is no longer in qBittorrent."),
        Err(e) => return error_view("Torrent", &format!("qBittorrent is not reachable: {e:#}")),
    };
    let (props, files) = tokio::join!(app.qbit.properties(hash), app.qbit.files(hash));
    let props = props.unwrap_or_default();
    let files = files.unwrap_or_default();
    let prio_label = |p: i64| match p {
        0 => "Skip",
        6 | 7 => "High",
        _ => "Normal",
    };
    let file_items: Vec<Value> = files
        .iter()
        .map(|f| {
            let mut actions = vec![];
            for (label, p) in [("Skip", 0), ("Normal", 1), ("High", 7)] {
                if prio_label(f.priority) != label {
                    actions.push(json!({ "label": label, "call": op("POST", &format!("files/{}/{}/{p}", t.hash, f.index), Value::Null) }));
                }
            }
            json!({
                "id": f.index.to_string(),
                "title": f.name.rsplit('/').next().unwrap_or(&f.name),
                "subtitle": format!("{} · {:.0}%", bytes(f.size), f.progress * 100.0),
                "progress": if f.progress < 1.0 { json!(f.progress) } else { Value::Null },
                "badges": [{ "text": prio_label(f.priority), "tone": if f.priority == 0 { "warning" } else { "info" } }],
                "actions": actions,
            })
        })
        .collect();
    let (state, _) = state_badge(&t);
    json!({
        "title": t.name,
        "subtitle": state,
        "refresh": 3,
        "blocks": [
            { "type": "stats", "items": [
                { "label": "Size", "value": bytes(t.size) },
                { "label": "Done", "value": format!("{:.1}%", t.progress * 100.0) },
                { "label": "Downloaded", "value": bytes(props.total_downloaded) },
                { "label": "Uploaded", "value": bytes(props.total_uploaded) },
                { "label": "Ratio", "value": format!("{:.2}", t.ratio) },
                { "label": "Seeds / peers", "value": format!("{} / {}", t.num_seeds, t.num_leechs) },
            ]},
            { "type": "text", "text": format!("Saved in {}", t.save_path) },
            { "type": "actions", "items": torrent_actions(app, &t) },
            { "type": "list", "items": file_items, "empty": "No file list yet (waiting for metadata)." },
        ]
    })
}

async fn search_page(app: &Shared, q: &HashMap<String, String>) -> Value {
    let query = q.get("q").map(|s| s.trim().to_string()).unwrap_or_default();
    let kind = Kind::parse(q.get("kind").map(String::as_str).unwrap_or("any"));
    let form = json!({ "type": "form", "id": "search", "fields": [
        { "name": "q", "label": "Search", "kind": "text", "value": query, "placeholder": "Silo S02E03, Interstellar 2014…", "required": true },
        { "name": "kind", "label": "Type", "kind": "select", "value": kind.as_str(),
          "options": [{ "value": "tv", "label": "TV" }, { "value": "movie", "label": "Movie" }, { "value": "any", "label": "Anything" }] },
    ], "submit": { "label": "Search", "call": op("POST", "search", json!({})) } });
    let (url, key) = app.prowlarr().await;
    if key.is_empty() {
        return json!({ "title": "Search", "blocks": [form, { "type": "text", "tone": "warning",
            "text": "Prowlarr is not connected: add its API key in Settings → qBittorrent." }] });
    }
    match prowlarr::indexer_count(&url, &key).await {
        Ok(0) => {
            return json!({ "title": "Search", "blocks": [form, { "type": "text", "tone": "warning",
                "text": "Prowlarr has no indexers yet. Open Prowlarr (port 9696 on the server), create its login, then add your trackers under Indexers → Add Indexer." }] })
        }
        Err(e) => return json!({ "title": "Search", "blocks": [form, { "type": "text", "tone": "error", "text": format!("Prowlarr is not reachable: {e:#}") }] }),
        Ok(_) => {}
    }
    if query.is_empty() {
        return json!({ "title": "Search", "blocks": [form] });
    }
    let releases = match prowlarr::search(&url, &key, &query, kind).await {
        Ok(r) => r,
        Err(e) => return json!({ "title": "Search", "blocks": [form, { "type": "text", "tone": "error", "text": format!("{e:#}") }] }),
    };
    let resolution = app.setting_str("resolution").await;
    let max_gb = if kind == Kind::Movie { app.setting_f64("max_movie_gb").await } else { app.setting_f64("max_episode_gb").await };
    let looks_like_pack = |title: &str| kind == Kind::Tv && !regex::Regex::new(r"(?i)s\d{1,2}e\d{1,3}").unwrap().is_match(title);
    let prefs = release::Prefs {
        height: resolution.parse().ok(),
        languages: app.setting_str("languages").await.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect(),
        max_bytes: max_gb.filter(|g| *g > 0.0).map(|g| (g * 1e9) as i64),
    };
    let mut ranked: Vec<(f64, &prowlarr::Release, release::Traits)> = releases
        .iter()
        .filter(|r| r.magnet_url.is_some() || r.download_url.is_some())
        .map(|r| {
            let t = release::traits(&r.title);
            let mut p = prefs.clone();
            if looks_like_pack(&r.title) {
                p.max_bytes = None;
            }
            (release::score(&t, r.size, r.seeders.unwrap_or(0), &p), r, t)
        })
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    let items: Vec<Value> = ranked
        .iter()
        .take(60)
        .map(|(_, r, t)| {
            let link = r.magnet_url.clone().or_else(|| r.download_url.clone()).unwrap_or_default();
            let mut sub = vec![bytes(r.size), format!("{} seeders", r.seeders.unwrap_or(0)), r.indexer.clone()];
            if let Some(d) = r.publish_date.as_deref().and_then(|d| d.get(..10)) {
                sub.push(d.to_string());
            }
            let mut badges: Vec<Value> = release::badges(t).into_iter().map(|b| json!({ "text": b })).collect();
            if r.seeders.unwrap_or(0) == 0 {
                badges.push(json!({ "text": "No seeders", "tone": "error" }));
            }
            json!({
                "id": r.guid,
                "title": r.title,
                "subtitle": sub.join(" · "),
                "badges": badges,
                "actions": [
                    { "label": "Download", "tone": "primary", "call": op("POST", "grab", json!({ "link": link, "watch": false, "title": r.title })) },
                    { "label": "Download in order", "call": op("POST", "grab", json!({ "link": link, "watch": true, "title": r.title })) },
                ],
            })
        })
        .collect();
    json!({
        "title": "Search",
        "subtitle": format!("{} results for “{query}”", ranked.len()),
        "blocks": [form, { "type": "list", "items": items, "empty": "Nothing found. Try fewer words, or search for the season (S02) instead of one episode." }]
    })
}

pub async fn page(app: &Shared, page: &str, q: &HashMap<String, String>) -> Value {
    match page {
        "downloads" => downloads(app).await,
        "torrent" => torrent_page(app, q.get("hash").map(String::as_str).unwrap_or_default()).await,
        "search" => search_page(app, q).await,
        _ => error_view("Not found", "This page does not exist."),
    }
}

pub async fn action(_app: &Shared, action: &str, ctx: &Value) -> Value {
    let title = ctx["title"].as_str().unwrap_or_default();
    match action {
        "find_missing" => {
            let mut q = title.to_string();
            if let Some(season) = ctx["season"].as_i64() {
                let missing: Vec<i64> = ctx["missing"].as_array().into_iter().flatten().filter_map(|m| m["episode"].as_i64()).collect();
                q.push_str(&format!(" S{season:02}"));
                if missing.len() == 1 {
                    q.push_str(&format!("E{:02}", missing[0]));
                }
            }
            json!({ "navigate": { "page": "search", "params": { "q": q, "kind": "tv" } } })
        }
        "find_movie" => {
            let q = match ctx["year"].as_i64() {
                Some(y) => format!("{title} {y}"),
                None => title.to_string(),
            };
            json!({ "navigate": { "page": "search", "params": { "q": q, "kind": "movie" } } })
        }
        _ => json!({ "message": "Unknown action", "tone": "error" }),
    }
}

fn result_err(e: anyhow::Error) -> Value {
    json!({ "message": format!("{e:#}"), "tone": "error" })
}

pub async fn handle(app: &Shared, path: &str, body: Value) -> Value {
    let parts: Vec<&str> = path.split('/').collect();
    let outcome: anyhow::Result<Value> = async {
        match parts.as_slice() {
            ["search"] => {
                let q = body["q"].as_str().unwrap_or_default();
                let kind = body["kind"].as_str().unwrap_or("any");
                Ok(json!({ "navigate": { "page": "search", "params": { "q": q, "kind": kind } } }))
            }
            ["add"] => {
                let source = body["source"].as_str().unwrap_or_default().trim().to_string();
                let file = match body["torrent"].as_object() {
                    Some(f) => {
                        let data = base64::engine::general_purpose::STANDARD.decode(f.get("data_base64").and_then(|v| v.as_str()).unwrap_or_default())?;
                        Some((f.get("name").and_then(|v| v.as_str()).unwrap_or("upload.torrent").to_string(), data))
                    }
                    None => None,
                };
                if source.is_empty() && file.is_none() {
                    anyhow::bail!("Paste a magnet link or URL, or choose a .torrent file");
                }
                if !source.is_empty() && !(source.starts_with("magnet:") || source.starts_with("http://") || source.starts_with("https://")) {
                    anyhow::bail!("That is not a magnet link or http(s) URL");
                }
                let urls = if source.is_empty() { vec![] } else { vec![source] };
                app.qbit.add(NewTorrent { urls, file, sequential: body["watch"].as_bool().unwrap_or(false) }).await?;
                Ok(json!({ "message": "Added to qBittorrent", "tone": "success", "refresh": true }))
            }
            ["grab"] => {
                let link = body["link"].as_str().unwrap_or_default().to_string();
                if link.is_empty() {
                    anyhow::bail!("This release has no download link");
                }
                app.qbit.add(NewTorrent { urls: vec![link], file: None, sequential: body["watch"].as_bool().unwrap_or(false) }).await?;
                Ok(json!({ "message": format!("Downloading {}", body["title"].as_str().unwrap_or("release")), "tone": "success",
                           "navigate": { "page": "downloads" } }))
            }
            ["torrents", hash, op] => {
                let t = app.qbit.torrent(hash).await?.ok_or_else(|| anyhow::anyhow!("Torrent not found"))?;
                match *op {
                    "stop" => app.qbit.stop(hash).await?,
                    "start" => app.qbit.start(hash).await?,
                    "sequential_on" => app.qbit.set_sequential(&t, true).await?,
                    "sequential_off" => app.qbit.set_sequential(&t, false).await?,
                    "delete" => {
                        app.qbit.delete(hash, false).await?;
                        return Ok(json!({ "message": format!("Removed {}", t.name), "navigate": { "page": "downloads" } }));
                    }
                    "delete_files" => {
                        if !in_downloads(app, &t) {
                            anyhow::bail!("Only downloads in the Downloads folder can be deleted from here");
                        }
                        app.qbit.delete(hash, true).await?;
                        if let Some(path) = app.cfg.to_ciniva(&t.content_path) {
                            let app = app.clone();
                            tokio::spawn(async move {
                                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                                app.rescan(&path).await;
                            });
                        }
                        return Ok(json!({ "message": format!("Deleted {}", t.name), "navigate": { "page": "downloads" } }));
                    }
                    _ => anyhow::bail!("Unknown operation"),
                }
                Ok(json!({ "refresh": true }))
            }
            ["files", hash, index, prio] => {
                app.qbit.file_priority(hash, index.parse()?, prio.parse()?).await?;
                Ok(json!({ "refresh": true }))
            }
            _ => anyhow::bail!("Unknown call"),
        }
    }
    .await;
    outcome.unwrap_or_else(result_err)
}
