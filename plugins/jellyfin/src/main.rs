#![recursion_limit = "256"]

mod app;
mod ciniva;
mod ids;
mod items;
mod library;
mod playback;
mod register;
mod system;

use axum::{
    extract::Request,
    http::{StatusCode, Uri},
    response::Redirect,
    routing::{any, delete, get, post},
    Router,
};
use std::sync::Arc;
use tower::Layer;

/// `NAME`, or the contents of the file named by `NAME_FILE` (Docker/Compose secrets).
fn env(name: &str) -> Option<String> {
    let from_file = || std::env::var(format!("{name}_FILE")).ok().and_then(|path| std::fs::read_to_string(path).ok());
    std::env::var(name).ok().or_else(from_file).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Jellyfin clients disagree on path casing and some still prefix `/emby`; route on one canonical form.
fn normalize(mut req: Request) -> Request {
    let uri = req.uri();
    let mut path = uri.path().to_string();
    for prefix in ["/emby", "/mediabrowser"] {
        if path.len() > prefix.len() && path[..prefix.len()].eq_ignore_ascii_case(prefix) && path.as_bytes()[prefix.len()] == b'/' {
            path = path[prefix.len()..].to_string();
        }
    }
    let lower = path.to_ascii_lowercase();
    let keep_case = lower.starts_with("/web/") && lower != "/web/configurationpages";
    let mut path = if keep_case { path } else { lower };
    if path.len() > 1 && path.ends_with('/') && !keep_case {
        path.pop();
    }
    let pq = match uri.query() {
        Some(q) => format!("{path}?{q}"),
        None => path,
    };
    if let Ok(new) = pq.parse::<Uri>() {
        *req.uri_mut() = new;
    }
    req
}

async fn fallback(req: Request) -> StatusCode {
    tracing::info!(method = %req.method(), path = %req.uri().path(), "unhandled Jellyfin endpoint");
    StatusCode::NOT_FOUND
}

fn routes() -> Router<app::Shared> {
    use library as l;
    use playback as p;
    use system as s;
    Router::new()
        .route("/system/info/public", get(s::info_public))
        .route("/system/info", get(s::info))
        .route("/system/ping", get(s::ping).post(s::ping))
        .route("/system/endpoint", get(s::endpoint))
        .route("/branding/configuration", get(s::branding))
        .route("/branding/css", get(s::branding_css))
        .route("/branding/css.css", get(s::branding_css))
        .route("/users/public", get(s::empty_list))
        .route("/users/authenticatebyname", post(s::authenticate))
        .route("/users/me", get(s::me))
        .route("/users/{id}", get(s::user))
        .route("/users/configuration", post(s::no_content))
        .route("/users/{uid}/configuration", post(s::no_content))
        .route("/users/{uid}/views", get(l::user_views))
        .route("/userviews", get(l::user_views))
        .route("/userviews/groupingoptions", get(s::empty_list))
        .route("/items", get(l::list_items))
        .route("/users/{uid}/items", get(l::list_items))
        .route("/items/{id}", get(l::get_item))
        .route("/users/{uid}/items/{id}", get(l::get_item))
        .route("/items/latest", get(l::latest))
        .route("/users/{uid}/items/latest", get(l::latest))
        .route("/useritems/resume", get(l::resume))
        .route("/users/{uid}/items/resume", get(l::resume))
        .route("/items/root", get(l::root_folder))
        .route("/users/{uid}/items/root", get(l::root_folder))
        .route("/items/counts", get(l::counts))
        .route("/items/filters", get(l::filters))
        .route("/items/filters2", get(l::filters))
        .route("/items/suggestions", get(s::empty_result))
        .route("/items/{id}/ancestors", get(l::ancestors))
        .route("/items/{id}/collections", get(s::empty_list))
        .route("/livetv/programs", get(s::empty_result))
        .route("/items/{id}/similar", get(s::empty_result))
        .route("/shows/{id}/similar", get(s::empty_result))
        .route("/movies/{id}/similar", get(s::empty_result))
        .route("/movies/recommendations", get(s::empty_list))
        .route("/items/{id}/thememedia", get(s::theme_media))
        .route("/items/{id}/themesongs", get(s::theme_list))
        .route("/items/{id}/themevideos", get(s::theme_list))
        .route("/items/{id}/specialfeatures", get(s::empty_list))
        .route("/users/{uid}/items/{id}/specialfeatures", get(s::empty_list))
        .route("/items/{id}/localtrailers", get(s::empty_list))
        .route("/users/{uid}/items/{id}/localtrailers", get(s::empty_list))
        .route("/items/{id}/intros", get(s::empty_result))
        .route("/users/{uid}/items/{id}/intros", get(s::empty_result))
        .route("/shows/nextup", get(l::next_up))
        .route("/shows/upcoming", get(s::empty_result))
        .route("/shows/{id}/seasons", get(l::seasons))
        .route("/shows/{id}/episodes", get(l::episodes))
        .route("/items/{id}/playbackinfo", get(p::playback_info).post(p::playback_info))
        .route("/videos/{id}/stream", get(p::stream))
        .route("/videos/{id}/{file}", get(p::stream))
        .route("/items/{id}/download", get(p::stream))
        .route("/videos/{id}/master.m3u8", get(p::master))
        .route("/videos/{id}/main.m3u8", get(p::main_playlist))
        .route("/videos/{id}/hls/{sid}/index.m3u8", get(p::hls_index))
        .route("/videos/{id}/hls/{sid}/{seg}", get(p::hls_segment))
        .route("/videos/activeencodings", delete(p::stop_encoding))
        .route("/videos/{id}/{msid}/subtitles/{index}/{format}", get(p::subtitle))
        .route("/videos/{id}/{msid}/subtitles/{index}/{ticks}/{format}", get(p::subtitle))
        .route("/sessions/playing", post(p::playing))
        .route("/sessions/playing/progress", post(p::progress))
        .route("/sessions/playing/stopped", post(p::stopped))
        .route("/sessions/playing/ping", post(s::no_content))
        .route("/sessions/capabilities", post(s::no_content))
        .route("/sessions/capabilities/full", post(s::no_content))
        .route("/sessions/viewing", post(s::no_content))
        .route("/sessions", get(s::sessions))
        .route("/sessions/logout", post(s::logout))
        .route("/users/{uid}/playingitems/{id}", post(p::legacy_playing).delete(p::legacy_playing))
        .route("/users/{uid}/playingitems/{id}/progress", post(p::legacy_playing))
        .route("/users/{uid}/playeditems/{id}", post(p::played).delete(p::played))
        .route("/userplayeditems/{id}", post(p::played).delete(p::played))
        .route("/users/{uid}/favoriteitems/{id}", post(p::favorite).delete(p::favorite))
        .route("/userfavoriteitems/{id}", post(p::favorite).delete(p::favorite))
        .route("/useritems/{id}/userdata", get(p::get_user_data).post(p::set_user_data))
        .route("/users/{uid}/items/{id}/userdata", get(p::get_user_data).post(p::set_user_data))
        .route("/useritems/{id}/rating", post(p::get_user_data).delete(p::get_user_data))
        .route("/users/{uid}/items/{id}/rating", post(p::get_user_data).delete(p::get_user_data))
        .route("/mediasegments/{id}", get(p::media_segments))
        .route("/playback/bitratetest", get(p::bitrate_test))
        .route("/displaypreferences/{id}", get(s::display_preferences).post(s::no_content))
        .route("/quickconnect/enabled", get(s::quick_connect_enabled))
        .route("/localization/countries", get(s::empty_list))
        .route("/localization/cultures", get(s::empty_list))
        .route("/localization/parentalratings", get(s::empty_list))
        .route("/localization/options", get(s::empty_list))
        .route("/web/configurationpages", get(s::empty_list))
        .route("/plugins", get(s::empty_list))
        .route("/syncplay/list", get(s::empty_list))
        .route("/genres", get(s::empty_result))
        .route("/studios", get(s::empty_result))
        .route("/persons", get(s::empty_result))
        .route("/artists", get(s::empty_result))
        .route("/livetv/programs/recommended", get(s::empty_result))
        .route("/livetv/channels", get(s::empty_result))
        .route("/clientlog/document", post(s::no_content))
        .route("/socket", any(s::socket))
        .route("/items/{id}/images", get(s::image_infos))
        .route("/items/{id}/images/{type}", get(s::image))
        .route("/items/{id}/images/{type}/{index}", get(s::image))
        .route("/items/{id}/images/{type}/{index}/{tag}/{format}/{maxwidth}/{maxheight}/{pct}/{unplayed}", get(s::image))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=warn".into()))
        .init();
    let ciniva_url = env("CINIVA_URL").unwrap_or_else(|| "http://ciniva:8484".into());
    let secret = env("CINIVA_PLUGIN_SECRET");
    let port: u16 = env("PORT").and_then(|p| p.parse().ok()).unwrap_or(8096);
    let server_name = env("SERVER_NAME").unwrap_or_else(|| "Ciniva".into());
    let plugin_url = env("PLUGIN_URL").unwrap_or_else(|| format!("http://ciniva-jellyfin:{port}"));
    let public_url = env("PUBLIC_URL");

    let state = Arc::new(app::State {
        ciniva: ciniva::Client::new(ciniva_url.clone(), secret.clone()),
        server_id: ids::server_id(&server_name),
        server_name,
        public_url: public_url.clone(),
        version: env("JELLYFIN_VERSION").unwrap_or_else(|| "10.11.0".into()),
        transcodes: Default::default(),
    });
    if secret.is_some() {
        tokio::spawn(register::run(state.ciniva.clone(), plugin_url.clone(), public_url.unwrap_or(plugin_url)));
    } else {
        tracing::warn!("CINIVA_PLUGIN_SECRET is not set: not registering with Ciniva, and images need a signed-in client");
    }

    let mut router = routes().fallback(fallback);
    if let Some(web) = env("WEB_DIR") {
        let index = std::path::Path::new(&web).join("index.html");
        router = router
            .nest_service("/web", tower_http::services::ServeDir::new(&web).fallback(tower_http::services::ServeFile::new(index)))
            .route("/", get(|| async { Redirect::temporary("/web/") }));
    }
    let router = router
        .with_state(state)
        .layer(tower_http::cors::CorsLayer::very_permissive())
        .layer(tower_http::trace::TraceLayer::new_for_http());
    let app = tower::util::MapRequestLayer::new(normalize).layer(router);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await.expect("bind");
    tracing::info!(port, ciniva = %ciniva_url, "Jellyfin bridge listening");
    axum::serve(listener, axum::ServiceExt::<Request>::into_make_service(app)).await.expect("server");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path_of(p: &str) -> String {
        let req = Request::builder().uri(p).body(axum::body::Body::empty()).unwrap();
        normalize(req).uri().to_string()
    }

    #[test]
    fn normalizes_paths() {
        assert_eq!(path_of("/Users/AuthenticateByName"), "/users/authenticatebyname");
        assert_eq!(path_of("/emby/Items/ABC/Images/Primary?tag=Xy.jpg"), "/items/abc/images/primary?tag=Xy.jpg");
        assert_eq!(path_of("/web/Main.JS"), "/web/Main.JS");
        assert_eq!(path_of("/web/ConfigurationPages?x=1"), "/web/configurationpages?x=1");
        assert_eq!(path_of("/System/Info/Public/"), "/system/info/public");
        assert_eq!(path_of("/embyfoo"), "/embyfoo");
    }
}
