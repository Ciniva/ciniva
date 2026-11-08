use crate::ciniva::Client;
use axum::http::Method;
use serde_json::json;
use std::time::Duration;

pub async fn run(ciniva: Client, plugin_url: String, connect_address: String) {
    let body = json!({
        "id": "jellyfin",
        "name": "Jellyfin apps",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Lets Jellyfin apps (Android TV, Swiftfin, Findroid, Infuse, webOS, Roku…) browse and play this library",
        "icon": "tv",
        "url": plugin_url,
        "settings": [{
            "name": "address",
            "label": "Server address for Jellyfin apps",
            "kind": "text",
            "value": connect_address,
            "help": "Add this address as a server in a Jellyfin app and sign in with your Ciniva account.",
        }],
        "pages": [],
        "actions": [],
    });
    let mut last_ok: Option<bool> = None;
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    loop {
        tick.tick().await;
        let result = ciniva.send(ciniva.secret(), Method::POST, "/plugins/register", &body).await;
        let ok = result.is_ok();
        if last_ok != Some(ok) {
            match result {
                Ok(_) => tracing::info!("registered with Ciniva"),
                Err(e) => tracing::warn!(status = %e.0, "could not register with Ciniva ({}); retrying every 60 s", e.1),
            }
        }
        last_ok = Some(ok);
    }
}
