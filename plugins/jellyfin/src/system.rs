use crate::{
    app::{body_json, field_str, json, not_found, ApiError, ApiResult, Auth, Client, Shared, Q},
    ciniva::User,
    ids::{Id, Kind},
    items::valid_image_name,
    library::{self, resolve, Resolved},
};
use axum::{
    body::Bytes,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    http::{header, HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{json, Value};

fn local_address(st: &Shared, headers: &HeaderMap) -> String {
    if let Some(u) = &st.public_url {
        return u.trim_end_matches('/').to_string();
    }
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("localhost:8096");
    let proto = headers.get("x-forwarded-proto").and_then(|h| h.to_str().ok()).unwrap_or("http");
    format!("{proto}://{host}")
}

fn public_info(st: &Shared, headers: &HeaderMap) -> Value {
    json!({
        "LocalAddress": local_address(st, headers),
        "ServerName": st.server_name,
        "Version": st.version,
        "ProductName": "Jellyfin Server",
        "OperatingSystem": "Linux",
        "Id": st.server_id,
        "StartupWizardCompleted": true,
    })
}

pub async fn info_public(State(st): State<Shared>, headers: HeaderMap) -> Response {
    json(public_info(&st, &headers))
}

pub async fn info(State(st): State<Shared>, _auth: Auth, headers: HeaderMap) -> Response {
    let mut v = public_info(&st, &headers);
    let extra = json!({
        "OperatingSystemDisplayName": "Linux",
        "PackageName": "ciniva-jellyfin",
        "HasPendingRestart": false,
        "IsShuttingDown": false,
        "SupportsLibraryMonitor": false,
        "WebSocketPortNumber": 0,
        "CompletedInstallations": [],
        "CanSelfRestart": false,
        "CanLaunchWebBrowser": false,
        "ProgramDataPath": "",
        "WebPath": "",
        "ItemsByNamePath": "",
        "CachePath": "",
        "LogPath": "",
        "InternalMetadataPath": "",
        "TranscodingTempPath": "",
        "CastReceiverApplications": [],
        "HasUpdateAvailable": false,
        "EncoderLocation": "System",
        "SystemArchitecture": std::env::consts::ARCH,
    });
    if let (Some(o), Value::Object(e)) = (v.as_object_mut(), extra) {
        o.extend(e);
    }
    json(v)
}

pub async fn ping() -> &'static str {
    "Jellyfin Server"
}

pub async fn endpoint() -> Response {
    json(json!({ "IsLocal": true, "IsInNetwork": true }))
}

pub async fn branding() -> Response {
    json(json!({ "LoginDisclaimer": "", "CustomCss": "", "SplashscreenEnabled": false }))
}

pub async fn branding_css() -> Response {
    ([(header::CONTENT_TYPE, "text/css")], "").into_response()
}

pub fn user_dto(st: &Shared, u: &User) -> Value {
    json!({
        "Name": u.username,
        "ServerId": st.server_id,
        "ServerName": st.server_name,
        "Id": Id::user(u.id),
        "HasPassword": true,
        "HasConfiguredPassword": true,
        "HasConfiguredEasyPassword": false,
        "EnableAutoLogin": false,
        "Configuration": {
            "AudioLanguagePreference": "",
            "PlayDefaultAudioTrack": true,
            "SubtitleLanguagePreference": "",
            "DisplayMissingEpisodes": false,
            "GroupedFolders": [],
            "SubtitleMode": "Default",
            "DisplayCollectionsView": false,
            "EnableLocalPassword": false,
            "OrderedViews": [],
            "LatestItemsExcludes": [],
            "MyMediaExcludes": [],
            "HidePlayedInLatest": true,
            "RememberAudioSelections": true,
            "RememberSubtitleSelections": true,
            "EnableNextEpisodeAutoPlay": true,
            "CastReceiverId": "",
        },
        "Policy": {
            "IsAdministrator": false,
            "IsHidden": false,
            "EnableCollectionManagement": false,
            "EnableSubtitleManagement": false,
            "EnableLyricManagement": false,
            "IsDisabled": false,
            "BlockedTags": [],
            "AllowedTags": [],
            "EnableUserPreferenceAccess": true,
            "AccessSchedules": [],
            "BlockUnratedItems": [],
            "EnableRemoteControlOfOtherUsers": false,
            "EnableSharedDeviceControl": false,
            "EnableRemoteAccess": true,
            "EnableLiveTvManagement": false,
            "EnableLiveTvAccess": false,
            "EnableMediaPlayback": true,
            "EnableAudioPlaybackTranscoding": true,
            "EnableVideoPlaybackTranscoding": true,
            "EnablePlaybackRemuxing": true,
            "ForceRemoteSourceTranscoding": false,
            "EnableContentDeletion": false,
            "EnableContentDeletionFromFolders": [],
            "EnableContentDownloading": true,
            "EnableSyncTranscoding": false,
            "EnableMediaConversion": false,
            "EnabledDevices": [],
            "EnableAllDevices": true,
            "EnabledChannels": [],
            "EnableAllChannels": true,
            "EnabledFolders": [],
            "EnableAllFolders": true,
            "InvalidLoginAttemptCount": 0,
            "LoginAttemptsBeforeLockout": -1,
            "MaxActiveSessions": 0,
            "EnablePublicSharing": false,
            "BlockedMediaFolders": [],
            "BlockedChannels": [],
            "RemoteClientBitrateLimit": 0,
            "AuthenticationProviderId": "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider",
            "PasswordResetProviderId": "Jellyfin.Server.Implementations.Users.DefaultPasswordResetProvider",
            "SyncPlayAccess": "None",
        },
    })
}

fn now_iso() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.0000000Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn session_info(st: &Shared, u: &User, c: &Client) -> Value {
    let now = now_iso();
    json!({
        "Id": c.device_id.clone().map(|d| crate::items::hash_id(&d)).unwrap_or_else(|| Id::user(u.id)),
        "UserId": Id::user(u.id),
        "UserName": u.username,
        "Client": c.client.clone().unwrap_or_default(),
        "DeviceName": c.device.clone().unwrap_or_default(),
        "DeviceId": c.device_id.clone().unwrap_or_default(),
        "ApplicationVersion": c.version.clone().unwrap_or_default(),
        "ServerId": st.server_id,
        "LastActivityDate": now,
        "LastPlaybackCheckIn": now,
        "IsActive": true,
        "SupportsMediaControl": false,
        "SupportsRemoteControl": false,
        "HasCustomDeviceName": false,
        "PlayableMediaTypes": ["Video"],
        "SupportedCommands": [],
        "AdditionalUsers": [],
        "NowPlayingQueue": [],
    })
}

pub async fn authenticate(State(st): State<Shared>, client: Client, body: Bytes) -> ApiResult {
    let b = body_json(&body);
    let username = field_str(&b, "Username").ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, "Username required".into()))?;
    let password = field_str(&b, "Pw").or_else(|| field_str(&b, "Password")).unwrap_or_default();
    let label = match (&client.client, &client.device) {
        (Some(c), Some(d)) => format!("{c} on {d}"),
        (Some(c), None) => c.clone(),
        _ => "Jellyfin app".to_string(),
    };
    let resp = st
        .ciniva
        .send(None, Method::POST, "/auth/login", &json!({ "username": username, "password": password, "client": label }))
        .await
        .map_err(|e| if e.0 == StatusCode::UNAUTHORIZED { ApiError(StatusCode::UNAUTHORIZED, "Invalid username or password".into()) } else { e.into() })?;
    let v: Value = resp.json().await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e.to_string()))?;
    let token = v["token"].as_str().ok_or_else(|| ApiError(StatusCode::BAD_GATEWAY, "Ciniva returned no token".into()))?.to_string();
    let user: User = serde_json::from_value(v["user"].clone()).map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e.to_string()))?;
    tracing::info!(user = %user.username, client = %label, "signed in");
    Ok(json(json!({
        "User": user_dto(&st, &user),
        "SessionInfo": session_info(&st, &user, &client),
        "AccessToken": token,
        "ServerId": st.server_id,
    })))
}

pub async fn me(State(st): State<Shared>, auth: Auth) -> ApiResult {
    let u = st.ciniva.me(&auth.token).await?;
    Ok(json(user_dto(&st, &u)))
}

pub async fn user(State(st): State<Shared>, auth: Auth, Path(id): Path<String>) -> ApiResult {
    let u = st.ciniva.me(&auth.token).await?;
    if Id::decode(&id).is_some_and(|i| i.kind == Kind::User && i.id != u.id) {
        return Err(not_found());
    }
    Ok(json(user_dto(&st, &u)))
}

pub async fn sessions(State(st): State<Shared>, auth: Auth) -> ApiResult {
    let u = st.ciniva.me(&auth.token).await?;
    Ok(json(json!([session_info(&st, &u, &auth.client)])))
}

pub async fn logout(State(st): State<Shared>, auth: Auth) -> StatusCode {
    let _ = st.ciniva.send(Some(&auth.token), Method::POST, "/auth/logout", &json!({})).await;
    StatusCode::NO_CONTENT
}

pub async fn empty_list() -> Response {
    json(json!([]))
}

pub async fn empty_result() -> Response {
    library::empty_result()
}

pub async fn no_content() -> StatusCode {
    StatusCode::NO_CONTENT
}

pub async fn quick_connect_enabled() -> Response {
    json(json!(false))
}

pub async fn display_preferences(Path(id): Path<String>, q: Q) -> Response {
    json(json!({
        "Id": id,
        "SortBy": "SortName",
        "RememberIndexing": false,
        "PrimaryImageHeight": 250,
        "PrimaryImageWidth": 250,
        "CustomPrefs": {},
        "ScrollDirection": "Horizontal",
        "ShowBackdrop": true,
        "RememberSorting": false,
        "SortOrder": "Ascending",
        "ShowSidebar": false,
        "Client": q.get("client").unwrap_or("emby"),
    }))
}

pub async fn theme_media(Path(id): Path<String>) -> Response {
    let empty = json!({ "Items": [], "TotalRecordCount": 0, "StartIndex": 0, "OwnerId": id });
    json(json!({ "ThemeVideosResult": empty, "ThemeSongsResult": empty, "SoundtrackSongsResult": empty }))
}

pub async fn theme_list(Path(id): Path<String>) -> Response {
    json(json!({ "Items": [], "TotalRecordCount": 0, "StartIndex": 0, "OwnerId": id }))
}

const SIZES: &[(i64, &str)] = &[(92, "w92"), (154, "w154"), (185, "w185"), (300, "w300"), (342, "w342"), (500, "w500"), (780, "w780"), (1280, "w1280")];

pub fn image_size(kind: &str, q: &Q) -> &'static str {
    let backdrop = matches!(kind, "backdrop" | "thumb" | "banner" | "art");
    let width = q
        .int("fillwidth")
        .or_else(|| q.int("maxwidth"))
        .or_else(|| q.int("width"))
        .or_else(|| q.int("fillheight").or_else(|| q.int("maxheight")).or_else(|| q.int("height")).map(|h| if backdrop { h * 16 / 9 } else { h * 2 / 3 }));
    match width {
        Some(w) => SIZES.iter().find(|(s, _)| *s >= w).map(|(_, n)| *n).unwrap_or("original"),
        None if backdrop => "w1280",
        None => "w500",
    }
}

async fn image_path(st: &Shared, token: &str, id: &str, kind: &str) -> ApiResult<Option<String>> {
    let tag = |p: &Option<String>| crate::items::image_tag(p);
    let backdrop = matches!(kind, "backdrop" | "thumb" | "art" | "banner");
    Ok(match resolve(st, token, id).await? {
        Resolved::Movie(m) => if backdrop { tag(&m.backdrop_path) } else { tag(&m.poster_path) },
        Resolved::Series(s) => if backdrop { tag(&s.backdrop_path) } else { tag(&s.poster_path) },
        Resolved::Season(s, c) => if backdrop { tag(&s.backdrop_path) } else { tag(&c.poster_path).or_else(|| tag(&s.poster_path)) },
        Resolved::Episode(s, e) => if backdrop { tag(&s.backdrop_path) } else { tag(&e.still_path).or_else(|| tag(&s.backdrop_path)) },
        Resolved::Person(_, c) => tag(&c.profile_path),
        _ => None,
    })
}

pub async fn image(State(st): State<Shared>, client: Client, Path(params): Path<Vec<(String, String)>>, q: Q, method: Method) -> ApiResult {
    let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let id = get("id").ok_or_else(not_found)?;
    let kind = get("type").unwrap_or_else(|| "primary".into()).to_ascii_lowercase();
    let token = st.ciniva.secret().map(str::to_string).or(client.token).ok_or_else(|| ApiError(StatusCode::UNAUTHORIZED, "authorization required".into()))?;
    let tag = q.get("tag").map(str::to_string).or_else(|| get("tag")).filter(|t| valid_image_name(t));
    let name = match tag {
        Some(t) => t,
        None => image_path(&st, &token, &id, &kind).await?.ok_or_else(not_found)?,
    };
    let size = image_size(&kind, &q);
    let m = if method == Method::HEAD { Method::HEAD } else { Method::GET };
    let resp = match st.ciniva.raw(&token, m.clone(), &format!("/images/{size}/{name}"), &HeaderMap::new()).await {
        Ok(r) => r,
        Err(_) if size != "original" => st.ciniva.raw(&token, m, &format!("/images/original/{name}"), &HeaderMap::new()).await?,
        Err(e) => return Err(e.into()),
    };
    let ctype = resp.headers().get("content-type").cloned();
    let bytes = resp.bytes().await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e.to_string()))?;
    let mut out = (StatusCode::OK, [(header::CACHE_CONTROL, "public, max-age=31536000")], bytes).into_response();
    if let Some(t) = ctype {
        out.headers_mut().insert(header::CONTENT_TYPE, t);
    }
    Ok(out)
}

pub async fn image_infos(State(st): State<Shared>, auth: Auth, Path(id): Path<String>) -> ApiResult {
    let r = resolve(&st, &auth.token, &id).await?;
    let v = library::item_value(&st, &auth, &r).await?;
    let mut out = Vec::new();
    if let Some(p) = v["ImageTags"]["Primary"].as_str() {
        out.push(json!({ "ImageType": "Primary", "ImageTag": p, "Path": p, "Size": 0 }));
    }
    for (i, b) in v["BackdropImageTags"].as_array().into_iter().flatten().enumerate() {
        out.push(json!({ "ImageType": "Backdrop", "ImageIndex": i, "ImageTag": b, "Path": b, "Size": 0 }));
    }
    Ok(json(Value::Array(out)))
}

pub async fn socket(ws: WebSocketUpgrade, auth: Auth) -> Response {
    let _ = auth.token;
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    let hello = json!({ "MessageType": "ForceKeepAlive", "Data": 60, "MessageId": crate::items::hash_id("keepalive") }).to_string();
    if socket.send(Message::Text(hello.into())).await.is_err() {
        return;
    }
    while let Some(Ok(msg)) = socket.recv().await {
        if let Message::Text(t) = msg {
            if t.contains("KeepAlive") {
                let reply = json!({ "MessageType": "KeepAlive", "MessageId": crate::items::hash_id(&t) }).to_string();
                if socket.send(Message::Text(reply.into())).await.is_err() {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_image_sizes() {
        assert_eq!(image_size("primary", &Q::parse(Some("fillWidth=300"))), "w300");
        assert_eq!(image_size("primary", &Q::parse(Some("maxWidth=301"))), "w342");
        assert_eq!(image_size("backdrop", &Q::parse(Some("maxWidth=3840"))), "original");
        assert_eq!(image_size("primary", &Q::parse(Some("fillHeight=450"))), "w300");
        assert_eq!(image_size("backdrop", &Q::default()), "w1280");
        assert_eq!(image_size("primary", &Q::default()), "w500");
    }

    #[test]
    fn iso_now_shape() {
        let n = now_iso();
        assert_eq!(n.len(), 28);
        assert!(n.starts_with("20") && n.ends_with('Z'));
    }
}
