use crate::{
    ciniva::{Cast, Episode, File, MediaInfo, Movie, MovieCard, Progress, SeasonCard, Show, ShowCard},
    ids::{ms_to_ticks, secs_to_ticks, Id, Kind},
};
use serde_json::{json, Map, Value};

/// Everything item builders need from the request.
pub struct Ctx<'a> {
    pub server_id: &'a str,
    pub token: &'a str,
}

pub fn image_tag(path: &Option<String>) -> Option<String> {
    path.as_deref().map(|p| p.trim_start_matches('/').to_string()).filter(|p| valid_image_name(p))
}

pub fn valid_image_name(name: &str) -> bool {
    name.len() < 100
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        && (name.ends_with(".jpg") || name.ends_with(".png") || name.ends_with(".svg"))
}

pub fn date(s: &Option<String>) -> Option<String> {
    let s = s.as_deref()?.trim();
    if s.is_empty() {
        return None;
    }
    Some(if s.len() == 10 { format!("{s}T00:00:00.0000000Z") } else { format!("{}.0000000Z", s.replacen(' ', "T", 1).trim_end_matches('Z')) })
}

fn year_of(s: &Option<String>) -> Option<i64> {
    s.as_deref().and_then(|d| d.get(..4)).and_then(|y| y.parse().ok())
}

fn put(m: &mut Map<String, Value>, key: &str, v: impl Into<Value>) {
    let v = v.into();
    if !v.is_null() {
        m.insert(key.to_string(), v);
    }
}

fn base(ctx: &Ctx, id: &str, name: &str, kind: &str) -> Map<String, Value> {
    let mut m = Map::new();
    put(&mut m, "Name", name);
    put(&mut m, "ServerId", ctx.server_id);
    put(&mut m, "Id", id);
    put(&mut m, "Type", kind);
    put(&mut m, "SortName", sort_name(name));
    put(&mut m, "LocationType", "FileSystem");
    put(&mut m, "ImageTags", json!({}));
    put(&mut m, "BackdropImageTags", json!([]));
    put(&mut m, "ImageBlurHashes", json!({}));
    put(&mut m, "Etag", id);
    m
}

pub fn sort_name(name: &str) -> String {
    let lower = name.to_lowercase();
    for article in ["the ", "a ", "an "] {
        if let Some(rest) = lower.strip_prefix(article) {
            return rest.to_string();
        }
    }
    lower
}

pub fn user_data(key: &str, position_ms: i64, duration_ms: i64, played: bool) -> Value {
    let pct = if duration_ms > 0 && position_ms > 0 { Some((position_ms as f64 / duration_ms as f64 * 100.0).min(100.0)) } else { None };
    let mut v = json!({
        "PlaybackPositionTicks": ms_to_ticks(position_ms.max(0)),
        "PlayCount": if played { 1 } else { 0 },
        "IsFavorite": false,
        "Played": played,
        "Key": key,
        "ItemId": key,
    });
    if let Some(p) = pct {
        v["PlayedPercentage"] = json!(p);
    }
    v
}

fn progress_user_data(key: &str, p: &Progress) -> Value {
    user_data(key, p.position_ms.unwrap_or(0), p.duration_ms.unwrap_or(0), p.watched.unwrap_or(false))
}

fn folder_user_data(key: &str, total: i64, watched: i64) -> Value {
    let mut v = user_data(key, 0, 0, total > 0 && watched >= total);
    v["UnplayedItemCount"] = json!((total - watched).max(0));
    if total > 0 && watched > 0 {
        v["PlayedPercentage"] = json!(watched as f64 / total as f64 * 100.0);
    }
    v
}

fn set_images(m: &mut Map<String, Value>, primary: Option<String>, backdrop: Option<String>, aspect: f64) {
    let mut tags = Map::new();
    if let Some(p) = &primary {
        tags.insert("Primary".into(), json!(p));
        m.insert("PrimaryImageAspectRatio".into(), json!(aspect));
    }
    if let Some(b) = &backdrop {
        tags.insert("Thumb".into(), json!(b));
        m.insert("BackdropImageTags".into(), json!([b]));
    }
    m.insert("ImageTags".into(), Value::Object(tags));
}

pub fn view(ctx: &Ctx, kind: Kind, count: Option<usize>) -> Value {
    let (name, collection) = if kind == Kind::MoviesView { ("Movies", "movies") } else { ("Shows", "tvshows") };
    let id = Id::view(kind);
    let mut m = base(ctx, &id, name, "CollectionFolder");
    put(&mut m, "CollectionType", collection);
    put(&mut m, "IsFolder", true);
    put(&mut m, "MediaType", "Unknown");
    put(&mut m, "ParentId", Id::view(Kind::Root));
    put(&mut m, "DisplayPreferencesId", id.clone());
    put(&mut m, "PrimaryImageAspectRatio", 1.7777777777777777);
    put(&mut m, "UserData", folder_user_data(&id, 0, 0));
    if let Some(c) = count {
        put(&mut m, "ChildCount", c);
        put(&mut m, "RecursiveItemCount", c);
    }
    Value::Object(m)
}

pub fn root(ctx: &Ctx) -> Value {
    let id = Id::view(Kind::Root);
    let mut m = base(ctx, &id, "Media Folders", "UserRootFolder");
    put(&mut m, "IsFolder", true);
    put(&mut m, "MediaType", "Unknown");
    put(&mut m, "ChildCount", 2);
    put(&mut m, "UserData", folder_user_data(&id, 0, 0));
    Value::Object(m)
}

pub fn movie_card(ctx: &Ctx, c: &MovieCard) -> Value {
    let id = Id::movie(c.id);
    let mut m = base(ctx, &id, &c.title, "Movie");
    put(&mut m, "IsFolder", false);
    put(&mut m, "MediaType", "Video");
    put(&mut m, "VideoType", "VideoFile");
    put(&mut m, "ParentId", Id::view(Kind::MoviesView));
    put(&mut m, "ProductionYear", c.year);
    put(&mut m, "CommunityRating", c.rating.map(round1));
    put(&mut m, "DateCreated", date(&c.added_at));
    put(&mut m, "PremiereDate", c.year.map(|y| format!("{y}-01-01T00:00:00.0000000Z")));
    put(&mut m, "UserData", progress_user_data(&id, &c.progress));
    if let Some(d) = c.progress.duration_ms.filter(|d| *d > 0) {
        put(&mut m, "RunTimeTicks", ms_to_ticks(d));
    }
    set_images(&mut m, image_tag(&c.poster_path), image_tag(&c.backdrop_path), 2.0 / 3.0);
    Value::Object(m)
}

fn round1(r: f64) -> f64 {
    (r * 10.0).round() / 10.0
}

fn people(cast: &[Cast], kind: Kind, owner: i64) -> Value {
    Value::Array(
        cast.iter()
            .enumerate()
            .map(|(i, c)| {
                let mut p = json!({ "Name": c.name, "Id": Id::new(kind, owner, 0, i as i64).encode(), "Type": "Actor" });
                if let Some(r) = &c.character {
                    p["Role"] = json!(r);
                }
                if let Some(t) = image_tag(&c.profile_path) {
                    p["PrimaryImageTag"] = json!(t);
                }
                p
            })
            .collect(),
    )
}

pub fn person(ctx: &Ctx, id: &str, c: &Cast) -> Value {
    let mut m = base(ctx, id, &c.name, "Person");
    put(&mut m, "IsFolder", false);
    put(&mut m, "MediaType", "Unknown");
    if let Some(t) = image_tag(&c.profile_path) {
        put(&mut m, "ImageTags", json!({ "Primary": t }));
        put(&mut m, "PrimaryImageAspectRatio", 2.0 / 3.0);
    }
    Value::Object(m)
}

fn file_runtime_ticks(f: &File) -> Option<i64> {
    f.duration.or_else(|| f.probe.as_ref().and_then(|p| p.duration)).filter(|d| *d > 0.0).map(secs_to_ticks)
}

fn file_user_data(key: &str, f: &File) -> Value {
    let dur = f.progress.duration_ms.filter(|d| *d > 0).or_else(|| f.duration.map(|d| (d * 1000.0) as i64)).unwrap_or(0);
    user_data(key, f.progress.position_ms.unwrap_or(0), dur, f.progress.watched.unwrap_or(false))
}

fn video_flags(m: &mut Map<String, Value>, f: &File) {
    if let Some(v) = f.probe.as_ref().and_then(|p| p.video.as_ref()) {
        put(m, "Width", v.width);
        put(m, "Height", v.height);
        put(m, "IsHD", v.height >= 720);
        put(m, "Video3DFormat", Value::Null);
    }
    put(m, "HasSubtitles", f.probe.as_ref().is_some_and(|p| !p.subtitles.is_empty()));
    put(m, "Container", container_of(f));
    put(m, "Path", f.path.clone());
}

pub fn movie(ctx: &Ctx, mv: &Movie) -> Value {
    let id = Id::movie(mv.id);
    let mut m = base(ctx, &id, &mv.title, "Movie");
    put(&mut m, "OriginalTitle", mv.original_title.clone());
    put(&mut m, "IsFolder", false);
    put(&mut m, "MediaType", "Video");
    put(&mut m, "VideoType", "VideoFile");
    put(&mut m, "ParentId", Id::view(Kind::MoviesView));
    put(&mut m, "Overview", mv.overview.clone());
    put(&mut m, "Taglines", json!(mv.tagline.iter().collect::<Vec<_>>()));
    put(&mut m, "ProductionYear", mv.year.or_else(|| year_of(&mv.release_date)));
    put(&mut m, "PremiereDate", date(&mv.release_date));
    put(&mut m, "CommunityRating", mv.rating.map(round1));
    put(&mut m, "Genres", json!(mv.genres));
    put(&mut m, "GenreItems", json!(mv.genres.iter().map(|g| json!({ "Name": g, "Id": hash_id(g) })).collect::<Vec<_>>()));
    put(&mut m, "People", people(&mv.cast, Kind::MoviePerson, mv.id));
    put(&mut m, "Studios", json!([]));
    put(&mut m, "Tags", json!([]));
    put(&mut m, "RemoteTrailers", json!([]));
    put(&mut m, "ExternalUrls", json!([]));
    if let Some(t) = mv.tmdb_id {
        put(&mut m, "ProviderIds", json!({ "Tmdb": t.to_string() }));
    }
    set_images(&mut m, image_tag(&mv.poster_path), image_tag(&mv.backdrop_path), 2.0 / 3.0);
    match mv.files.first() {
        Some(f) => {
            put(&mut m, "RunTimeTicks", file_runtime_ticks(f).or(mv.runtime.map(|r| r * 60 * 10_000_000)));
            put(&mut m, "DateCreated", date(&f.added_at));
            put(&mut m, "UserData", file_user_data(&id, f));
            put(&mut m, "CanDownload", true);
            put(&mut m, "MediaSources", json!(mv.files.iter().enumerate().map(|(i, f)| media_source(ctx, &id, f, &mv.title, i == 0)).collect::<Vec<_>>()));
            put(&mut m, "MediaStreams", media_source(ctx, &id, f, &mv.title, true)["MediaStreams"].clone());
            put(&mut m, "PartCount", 1);
            put(&mut m, "MediaSourceCount", mv.files.len());
            video_flags(&mut m, f);
        }
        None => {
            put(&mut m, "RunTimeTicks", mv.runtime.map(|r| r * 60 * 10_000_000));
            put(&mut m, "UserData", user_data(&id, 0, 0, false));
        }
    }
    Value::Object(m)
}

pub fn hash_id(name: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(&Sha256::digest(format!("genre:{name}").as_bytes())[..16])
}

pub fn show_card(ctx: &Ctx, c: &ShowCard) -> Value {
    let id = Id::series(c.id);
    let mut m = base(ctx, &id, &c.title, "Series");
    put(&mut m, "IsFolder", true);
    put(&mut m, "MediaType", "Unknown");
    put(&mut m, "ParentId", Id::view(Kind::ShowsView));
    put(&mut m, "ProductionYear", c.year);
    put(&mut m, "PremiereDate", c.year.map(|y| format!("{y}-01-01T00:00:00.0000000Z")));
    put(&mut m, "CommunityRating", c.rating.map(round1));
    put(&mut m, "DateCreated", date(&c.last_added));
    put(&mut m, "DateLastMediaAdded", date(&c.last_added));
    put(&mut m, "RecursiveItemCount", c.episodes);
    put(&mut m, "UserData", folder_user_data(&id, c.episodes, c.watched_episodes));
    set_images(&mut m, image_tag(&c.poster_path), image_tag(&c.backdrop_path), 2.0 / 3.0);
    Value::Object(m)
}

pub fn show(ctx: &Ctx, s: &Show, last_added: Option<&String>) -> Value {
    let id = Id::series(s.id);
    let mut m = base(ctx, &id, &s.title, "Series");
    put(&mut m, "OriginalTitle", s.original_title.clone());
    put(&mut m, "IsFolder", true);
    put(&mut m, "MediaType", "Unknown");
    put(&mut m, "ParentId", Id::view(Kind::ShowsView));
    put(&mut m, "Overview", s.overview.clone());
    put(&mut m, "ProductionYear", s.year.or_else(|| year_of(&s.first_air_date)));
    put(&mut m, "PremiereDate", date(&s.first_air_date));
    put(&mut m, "CommunityRating", s.rating.map(round1));
    put(&mut m, "Status", s.status.as_deref().map(|st| if st == "Ended" || st == "Canceled" { "Ended" } else { "Continuing" }));
    put(&mut m, "Genres", json!(s.genres));
    put(&mut m, "GenreItems", json!(s.genres.iter().map(|g| json!({ "Name": g, "Id": hash_id(g) })).collect::<Vec<_>>()));
    put(&mut m, "People", people(&s.cast, Kind::ShowPerson, s.id));
    put(&mut m, "Studios", json!([]));
    put(&mut m, "Tags", json!([]));
    put(&mut m, "RemoteTrailers", json!([]));
    put(&mut m, "ExternalUrls", json!([]));
    if let Some(t) = s.tmdb_id {
        put(&mut m, "ProviderIds", json!({ "Tmdb": t.to_string() }));
    }
    let total: i64 = s.seasons.iter().map(|x| x.available).sum();
    let watched: i64 = s.seasons.iter().map(|x| x.watched).sum();
    put(&mut m, "ChildCount", s.seasons.len());
    put(&mut m, "RecursiveItemCount", total);
    put(&mut m, "DateCreated", date(&last_added.cloned()));
    put(&mut m, "UserData", folder_user_data(&id, total, watched));
    set_images(&mut m, image_tag(&s.poster_path), image_tag(&s.backdrop_path), 2.0 / 3.0);
    Value::Object(m)
}

fn series_refs(m: &mut Map<String, Value>, s: &Show) {
    let sid = Id::series(s.id);
    put(m, "SeriesId", sid.clone());
    put(m, "SeriesName", s.title.clone());
    if let Some(p) = image_tag(&s.poster_path) {
        put(m, "SeriesPrimaryImageTag", p);
    }
    if let Some(b) = image_tag(&s.backdrop_path) {
        put(m, "ParentBackdropItemId", sid.clone());
        put(m, "ParentBackdropImageTags", json!([b]));
        put(m, "ParentThumbItemId", sid.clone());
        put(m, "ParentThumbImageTag", b.clone());
        put(m, "SeriesThumbImageTag", b);
    }
    put(m, "Genres", json!(s.genres));
    put(m, "Status", Value::Null);
}

pub fn season_name(c: &SeasonCard) -> String {
    c.name.clone().unwrap_or_else(|| if c.season_number == 0 { "Specials".into() } else { format!("Season {}", c.season_number) })
}

pub fn season(ctx: &Ctx, s: &Show, c: &SeasonCard) -> Value {
    let id = Id::season(s.id, c.season_number);
    let mut m = base(ctx, &id, &season_name(c), "Season");
    put(&mut m, "SortName", format!("{:04}", c.season_number));
    put(&mut m, "IsFolder", true);
    put(&mut m, "MediaType", "Unknown");
    put(&mut m, "IndexNumber", c.season_number);
    put(&mut m, "ParentId", Id::series(s.id));
    put(&mut m, "PremiereDate", date(&c.air_date));
    put(&mut m, "ProductionYear", year_of(&c.air_date));
    put(&mut m, "ChildCount", c.available);
    put(&mut m, "RecursiveItemCount", c.available);
    put(&mut m, "UserData", folder_user_data(&id, c.available, c.watched));
    series_refs(&mut m, s);
    set_images(&mut m, image_tag(&c.poster_path), image_tag(&s.backdrop_path), 2.0 / 3.0);
    Value::Object(m)
}

pub fn episode(ctx: &Ctx, s: &Show, e: &Episode) -> Value {
    let id = Id::episode(s.id, e.season_number, e.id);
    let name = e.title.clone().unwrap_or_else(|| format!("Episode {}", e.episode_number));
    let mut m = base(ctx, &id, &name, "Episode");
    put(&mut m, "SortName", format!("{:04}{:04}", e.season_number, e.episode_number));
    put(&mut m, "IsFolder", false);
    put(&mut m, "MediaType", "Video");
    put(&mut m, "VideoType", "VideoFile");
    put(&mut m, "Overview", e.overview.clone());
    put(&mut m, "IndexNumber", e.episode_number);
    put(&mut m, "ParentIndexNumber", e.season_number);
    put(&mut m, "PremiereDate", date(&e.air_date));
    put(&mut m, "ProductionYear", year_of(&e.air_date));
    put(&mut m, "CommunityRating", e.rating.map(round1));
    let season_id = Id::season(s.id, e.season_number);
    put(&mut m, "SeasonId", season_id.clone());
    put(&mut m, "ParentId", season_id);
    put(&mut m, "SeasonName", if e.season_number == 0 { "Specials".to_string() } else { format!("Season {}", e.season_number) });
    series_refs(&mut m, s);
    set_images(&mut m, image_tag(&e.still_path), None, 16.0 / 9.0);
    match e.files.first() {
        Some(f) => {
            put(&mut m, "RunTimeTicks", file_runtime_ticks(f).or(e.runtime.map(|r| r * 60 * 10_000_000)));
            put(&mut m, "DateCreated", date(&f.added_at));
            put(&mut m, "UserData", file_user_data(&id, f));
            put(&mut m, "CanDownload", true);
            put(&mut m, "MediaSources", json!(e.files.iter().enumerate().map(|(i, f)| media_source(ctx, &id, f, &name, i == 0)).collect::<Vec<_>>()));
            put(&mut m, "MediaStreams", media_source(ctx, &id, f, &name, true)["MediaStreams"].clone());
            put(&mut m, "PartCount", 1);
            put(&mut m, "MediaSourceCount", e.files.len());
            put(&mut m, "LocationType", "FileSystem");
            video_flags(&mut m, f);
        }
        None => {
            put(&mut m, "LocationType", "Virtual");
            put(&mut m, "RunTimeTicks", e.runtime.map(|r| r * 60 * 10_000_000));
            put(&mut m, "UserData", user_data(&id, 0, 0, false));
        }
    }
    Value::Object(m)
}

pub fn container_of(f: &File) -> String {
    let ext = std::path::Path::new(&f.path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "m2ts" | "mts" => "ts".into(),
        "" => f.probe.as_ref().and_then(|p| p.container.clone()).and_then(|c| c.split(',').next().map(str::to_string)).unwrap_or_default(),
        e => e.to_string(),
    }
}

fn language_name(code: &str) -> Option<&'static str> {
    Some(match code {
        "eng" | "en" => "English",
        "fre" | "fra" | "fr" => "French",
        "ger" | "deu" | "de" => "German",
        "spa" | "es" => "Spanish",
        "ita" | "it" => "Italian",
        "jpn" | "ja" => "Japanese",
        "por" | "pt" => "Portuguese",
        "rus" | "ru" => "Russian",
        "dut" | "nld" | "nl" => "Dutch",
        "pol" | "pl" => "Polish",
        "swe" | "sv" => "Swedish",
        "dan" | "da" => "Danish",
        "nor" | "nob" | "no" => "Norwegian",
        "fin" | "fi" => "Finnish",
        "chi" | "zho" | "zh" => "Chinese",
        "kor" | "ko" => "Korean",
        "ara" | "ar" => "Arabic",
        "tur" | "tr" => "Turkish",
        "heb" | "he" => "Hebrew",
        "hin" | "hi" => "Hindi",
        _ => return None,
    })
}

fn display_title(lang: &Option<String>, title: &Option<String>, rest: &[String]) -> String {
    let lang = lang.as_deref().filter(|l| *l != "und");
    let mut parts: Vec<String> = Vec::new();
    if let Some(t) = title.as_deref().filter(|t| !t.is_empty()) {
        parts.push(t.to_string());
    } else if let Some(l) = lang {
        parts.push(language_name(l).map(str::to_string).unwrap_or_else(|| l.to_string()));
    }
    let extra: Vec<&String> = rest.iter().filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        parts.extend(extra.iter().map(|s| s.to_string()));
        return parts.join(" - ");
    }
    if !extra.is_empty() {
        return format!("{} - {}", parts.join(" "), extra.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" - "));
    }
    parts.join(" ")
}

pub fn subtitle_codec(codec: &str) -> &str {
    match codec {
        "hdmv_pgs_subtitle" => "PGSSUB",
        "dvd_subtitle" => "DVDSUB",
        "dvb_subtitle" => "DVBSUB",
        c => c,
    }
}

pub const TEXT_SUBS: &[&str] = &["subrip", "srt", "ass", "ssa", "mov_text", "webvtt", "text"];

fn channel_layout(ch: i64) -> &'static str {
    match ch {
        1 => "mono",
        2 => "stereo",
        6 => "5.1",
        8 => "7.1",
        _ => "",
    }
}

fn video_range(v: &crate::ciniva::VideoInfo) -> (&'static str, &'static str) {
    if v.dolby_vision_profile.is_some() {
        return ("HDR", if v.hdr.as_deref() == Some("hdr10") { "DOVIWithHDR10" } else { "DOVI" });
    }
    match v.hdr.as_deref() {
        Some("hlg") => ("HDR", "HLG"),
        Some("hdr10+") | Some("hdr10plus") => ("HDR", "HDR10Plus"),
        Some(_) => ("HDR", "HDR10"),
        None => ("SDR", "SDR"),
    }
}

/// `subtitle_urls` is false when the client should play embedded subtitles itself.
pub fn media_streams(item_id: &str, msid: &str, f: &File, token: &str, external_subs: bool) -> Vec<Value> {
    let Some(p) = f.probe.as_ref() else { return Vec::new() };
    let mut out = Vec::new();
    let used: Vec<i64> = p.audio.iter().map(|a| a.index).chain(p.subtitles.iter().map(|s| s.index)).collect();
    if let Some(v) = &p.video {
        let index = (0..).find(|i| !used.contains(i)).unwrap_or(0);
        let (range, range_type) = video_range(v);
        let res = if v.height >= 2000 { "4K".to_string() } else if v.height > 0 { format!("{}p", v.height) } else { String::new() };
        out.push(json!({
            "Type": "Video",
            "Index": index,
            "Codec": v.codec,
            "Profile": v.profile,
            "Width": v.width,
            "Height": v.height,
            "BitDepth": v.bit_depth,
            "AverageFrameRate": v.fps,
            "RealFrameRate": v.fps,
            "BitRate": p.bitrate,
            "AspectRatio": aspect(v.width, v.height),
            "VideoRange": range,
            "VideoRangeType": range_type,
            "DvProfile": v.dolby_vision_profile,
            "DisplayTitle": display_title(&None, &None, &[res, v.codec.to_uppercase(), range_type.to_string()]),
            "IsAVC": v.codec == "h264",
            "IsInterlaced": false,
            "IsDefault": true,
            "IsForced": false,
            "IsHearingImpaired": false,
            "IsOriginal": false,
            "IsExternal": false,
            "IsTextSubtitleStream": false,
            "SupportsExternalStream": false,
            "IsAnamorphic": false,
            "AudioSpatialFormat": "None",
            "PixelFormat": if v.bit_depth.unwrap_or(8) > 8 { "yuv420p10le" } else { "yuv420p" },
            "Level": 0,
        }));
    }
    let default_audio = p.audio.iter().find(|a| a.default).or(p.audio.first()).map(|a| a.index);
    for a in &p.audio {
        let ch = a.channels.unwrap_or(2);
        let layout = channel_layout(ch);
        out.push(json!({
            "Type": "Audio",
            "Index": a.index,
            "Codec": a.codec,
            "Profile": a.profile,
            "Language": a.language,
            "Title": a.title,
            "Channels": ch,
            "ChannelLayout": layout,
            "DisplayTitle": display_title(&a.language, &a.title, &[a.codec.to_uppercase(), layout.to_string()]),
            "IsDefault": Some(a.index) == default_audio,
            "IsForced": false,
            "IsHearingImpaired": false,
            "IsOriginal": false,
            "IsInterlaced": false,
            "IsExternal": false,
            "IsTextSubtitleStream": false,
            "SupportsExternalStream": false,
            "VideoRange": "Unknown",
            "VideoRangeType": "Unknown",
            "AudioSpatialFormat": if a.atmos { "DolbyAtmos" } else { "None" },
            "Level": 0,
        }));
    }
    for s in &p.subtitles {
        let text = TEXT_SUBS.contains(&s.codec.as_str());
        let mut v = json!({
            "Type": "Subtitle",
            "Index": s.index,
            "Codec": subtitle_codec(&s.codec),
            "Language": s.language,
            "Title": s.title,
            "DisplayTitle": display_title(&s.language, &s.title, &[if s.forced { "Forced".to_string() } else { String::new() }, subtitle_codec(&s.codec).to_uppercase()]),
            "IsDefault": s.default,
            "IsForced": s.forced,
            "IsHearingImpaired": false,
            "IsOriginal": false,
            "IsInterlaced": false,
            "IsExternal": false,
            "IsTextSubtitleStream": text,
            "SupportsExternalStream": text,
            "VideoRange": "Unknown",
            "VideoRangeType": "Unknown",
            "AudioSpatialFormat": "None",
            "Level": 0,
        });
        if text && external_subs {
            v["DeliveryMethod"] = json!("External");
            v["DeliveryUrl"] = json!(format!("/Videos/{item_id}/{msid}/Subtitles/{}/0/Stream.vtt?api_key={token}", s.index));
            v["IsExternalUrl"] = json!(false);
        } else {
            v["DeliveryMethod"] = json!("Embed");
        }
        out.push(v);
    }
    out
}

fn aspect(w: i64, h: i64) -> Option<String> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let r = w as f64 / h as f64;
    Some(
        [(16.0 / 9.0, "16:9"), (4.0 / 3.0, "4:3"), (2.39, "2.39:1"), (2.35, "2.35:1"), (1.85, "1.85:1"), (2.0, "2:1"), (2.2, "2.20:1")]
            .iter()
            .find(|(v, _)| (r - v).abs() < 0.02)
            .map(|(_, s)| s.to_string())
            .unwrap_or_else(|| format!("{:.2}:1", r)),
    )
}

pub fn default_audio_index(p: &MediaInfo) -> Option<i64> {
    p.audio.iter().find(|a| a.default).or(p.audio.first()).map(|a| a.index)
}

/// Like Jellyfin, an item's primary version shares the item's id; other versions get file ids.
pub fn source_id(item_id: &str, f: &File, primary: bool) -> String {
    if primary {
        item_id.to_string()
    } else {
        Id::file(f.id)
    }
}

pub fn media_source(ctx: &Ctx, item_id: &str, f: &File, name: &str, primary: bool) -> Value {
    let container = container_of(f);
    let msid = source_id(item_id, f, primary);
    let probe = f.probe.clone().unwrap_or_default();
    json!({
        "Protocol": "File",
        "Id": msid,
        "Path": f.path,
        "Type": "Default",
        "Container": container,
        "Size": f.size,
        "Name": name,
        "IsRemote": false,
        "ETag": msid,
        "RunTimeTicks": file_runtime_ticks(f),
        "ReadAtNativeFramerate": false,
        "IgnoreDts": false,
        "IgnoreIndex": false,
        "GenPtsInput": false,
        "SupportsTranscoding": probe.video.is_some(),
        "SupportsDirectStream": true,
        "SupportsDirectPlay": true,
        "IsInfiniteStream": false,
        "UseMostCompatibleTranscodingProfile": false,
        "RequiresOpening": false,
        "RequiresClosing": false,
        "RequiresLooping": false,
        "SupportsProbing": true,
        "VideoType": "VideoFile",
        "MediaStreams": media_streams(item_id, &msid, f, ctx.token, true),
        "MediaAttachments": [],
        "Formats": [],
        "Bitrate": probe.bitrate,
        "RequiredHttpHeaders": {},
        "TranscodingSubProtocol": "http",
        "DefaultAudioStreamIndex": default_audio_index(&probe),
        "DefaultSubtitleStreamIndex": probe.subtitles.iter().find(|s| s.default || s.forced).map(|s| s.index).unwrap_or(-1),
        "HasSegments": f.credits_start.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(date(&Some("2010-09-30".into())).unwrap(), "2010-09-30T00:00:00.0000000Z");
        assert_eq!(date(&Some("2026-11-08 02:12:56".into())).unwrap(), "2026-11-08T02:12:56.0000000Z");
        assert_eq!(date(&None), None);
    }

    #[test]
    fn user_data_ticks() {
        let u = user_data("k", 60_000, 600_000, false);
        assert_eq!(u["PlaybackPositionTicks"], 600_000_000);
        assert_eq!(u["PlayedPercentage"], 10.0);
        assert_eq!(user_data("k", 0, 0, true)["PlayCount"], 1);
    }

    #[test]
    fn sort_names_skip_articles() {
        assert_eq!(sort_name("The Matrix"), "matrix");
        assert_eq!(sort_name("Sintel"), "sintel");
    }
}
