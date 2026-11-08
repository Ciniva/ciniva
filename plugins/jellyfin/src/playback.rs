use crate::{
    app::{body_json, field, field_bool, field_i64, field_str, json, not_found, ApiError, ApiResult, Auth, Shared, Q},
    ciniva::{File, MediaInfo},
    ids::{secs_to_ticks, ticks_to_ms, Id, Kind},
    items::{self, container_of, default_audio_index},
    library::{self, files_for, resolve, Resolved},
};
use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{json, Value};

fn new_play_session() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut b);
    hex::encode(b)
}

fn split(v: Option<&Value>) -> Vec<String> {
    v.and_then(|v| v.as_str()).unwrap_or("").split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect()
}

fn container_matches(list: &[String], container: &str) -> bool {
    let aliases: &[&str] = match container {
        "mkv" => &["mkv", "matroska", "webm"],
        "mp4" | "m4v" => &["mp4", "m4v", "mov"],
        "mov" => &["mov", "mp4"],
        "ts" | "m2ts" => &["ts", "mpegts", "m2ts"],
        c => return list.is_empty() || list.iter().any(|l| l == c),
    };
    list.is_empty() || list.iter().any(|l| aliases.contains(&l.as_str()))
}

fn codec_matches(list: &[String], codec: &str) -> bool {
    let alt = match codec {
        "hevc" => "h265",
        "h264" => "avc",
        _ => codec,
    };
    list.is_empty() || list.iter().any(|l| l == codec || l == alt)
}

struct Props<'a> {
    probe: &'a MediaInfo,
    audio: Option<&'a crate::ciniva::AudioInfo>,
}

impl Props<'_> {
    fn get(&self, name: &str) -> Option<String> {
        let v = self.probe.video.as_ref();
        Some(match name {
            "VideoBitDepth" => v?.bit_depth?.to_string(),
            "Width" => v?.width.to_string(),
            "Height" => v?.height.to_string(),
            "VideoProfile" => v?.profile.clone()?.to_lowercase(),
            "VideoFramerate" => v?.fps?.to_string(),
            "VideoRangeType" => {
                let v = v?;
                if v.dolby_vision_profile.is_some() {
                    "DOVI".into()
                } else {
                    match v.hdr.as_deref() {
                        Some("hlg") => "HLG".into(),
                        Some(_) => "HDR10".into(),
                        None => "SDR".into(),
                    }
                }
            }
            "AudioChannels" => self.audio?.channels?.to_string(),
            "AudioProfile" => self.audio?.profile.clone()?.to_lowercase(),
            "IsSecondaryAudio" => "false".into(),
            "VideoBitrate" | "AudioBitrate" => self.probe.bitrate?.to_string(),
            _ => return None,
        })
    }
}

fn condition_holds(c: &Value, props: &Props) -> bool {
    let prop = c["Property"].as_str().unwrap_or("");
    let Some(actual) = props.get(prop) else { return true };
    let want = c["Value"].as_str().unwrap_or("");
    let num = |s: &str| s.parse::<f64>().ok();
    match c["Condition"].as_str().unwrap_or("") {
        "Equals" => actual.eq_ignore_ascii_case(want),
        "NotEquals" => !actual.eq_ignore_ascii_case(want),
        "EqualsAny" => want.split('|').any(|w| w.eq_ignore_ascii_case(&actual)),
        "LessThanEqual" => matches!((num(&actual), num(want)), (Some(a), Some(w)) if a <= w) || num(&actual).is_none(),
        "GreaterThanEqual" => matches!((num(&actual), num(want)), (Some(a), Some(w)) if a >= w) || num(&actual).is_none(),
        _ => true,
    }
}

/// Whether a Jellyfin DeviceProfile says the client can play the file as it is.
pub fn can_direct_play(profile: &Value, f: &File, audio_index: Option<i64>) -> bool {
    let Some(probe) = f.probe.as_ref() else { return true };
    let container = container_of(f);
    let vcodec = probe.video.as_ref().map(|v| v.codec.to_ascii_lowercase()).unwrap_or_default();
    let audio = audio_index.and_then(|i| probe.audio.iter().find(|a| a.index == i)).or_else(|| default_audio_index(probe).and_then(|i| probe.audio.iter().find(|a| a.index == i)));
    let acodec = audio.map(|a| a.codec.to_ascii_lowercase()).unwrap_or_default();
    let Some(direct) = field(profile, "DirectPlayProfiles").and_then(|v| v.as_array()) else { return true };
    let ok = direct.iter().any(|p| {
        p["Type"].as_str().unwrap_or("Video") == "Video"
            && container_matches(&split(p.get("Container")), &container)
            && codec_matches(&split(p.get("VideoCodec")), &vcodec)
            && (acodec.is_empty() || codec_matches(&split(p.get("AudioCodec")), &acodec))
    });
    if !ok {
        return false;
    }
    let props = Props { probe, audio };
    field(profile, "CodecProfiles").and_then(|v| v.as_array()).into_iter().flatten().all(|cp| {
        let ty = cp["Type"].as_str().unwrap_or("");
        let codecs = split(cp.get("Codec"));
        let applies = match ty {
            "Video" => codec_matches(&codecs, &vcodec),
            "VideoAudio" => !acodec.is_empty() && codec_matches(&codecs, &acodec),
            _ => false,
        } && container_matches(&split(cp.get("Container")), &container);
        !applies || cp["Conditions"].as_array().into_iter().flatten().all(|c| condition_holds(c, &props))
    })
}

struct Transcode {
    video_codecs: Vec<String>,
    audio_codecs: Vec<String>,
    container: String,
}

fn transcoding_profile(profile: &Value) -> Transcode {
    let list = field(profile, "TranscodingProfiles").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let p = list
        .iter()
        .find(|p| p["Type"] == "Video" && p["Protocol"].as_str().is_some_and(|s| s.eq_ignore_ascii_case("hls")))
        .or_else(|| list.iter().find(|p| p["Type"] == "Video"));
    Transcode {
        video_codecs: p.map(|p| split(p.get("VideoCodec"))).unwrap_or_default(),
        audio_codecs: p.map(|p| split(p.get("AudioCodec"))).unwrap_or_default(),
        container: p.and_then(|p| p["Container"].as_str()).unwrap_or("ts").to_string(),
    }
}

pub async fn playback_info(State(st): State<Shared>, auth: Auth, Path(id): Path<String>, q: Q, body: Bytes) -> ApiResult {
    let body = body_json(&body);
    let (item_id, files, name) = files_for(&st, &auth.token, &id).await?;
    let wanted_source = field_str(&body, "MediaSourceId").or_else(|| q.get("mediasourceid").map(str::to_string));
    let audio_index = field_i64(&body, "AudioStreamIndex").or_else(|| q.int("audiostreamindex"));
    let max_bitrate = field_i64(&body, "MaxStreamingBitrate").or_else(|| q.int("maxstreamingbitrate"));
    let enable_direct_play = field_bool(&body, "EnableDirectPlay").or_else(|| q.bool("enabledirectplay")).unwrap_or(true);
    let enable_direct_stream = field_bool(&body, "EnableDirectStream").or_else(|| q.bool("enabledirectstream")).unwrap_or(true);
    let enable_transcoding = field_bool(&body, "EnableTranscoding").or_else(|| q.bool("enabletranscoding")).unwrap_or(true);
    let profile = field(&body, "DeviceProfile").cloned();
    let play_session = new_play_session();
    let cx = items::Ctx { server_id: &st.server_id, token: &auth.token };

    let mut sources = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let msid = items::source_id(&item_id, f, i == 0);
        if wanted_source.as_deref().is_some_and(|w| w.replace('-', "").to_ascii_lowercase() != msid) && files.len() > 1 {
            continue;
        }
        let mut src = items::media_source(&cx, &item_id, f, &name, i == 0);
        let over_bitrate = matches!((max_bitrate, f.probe.as_ref().and_then(|p| p.bitrate)), (Some(m), Some(b)) if m > 0 && b > m);
        let direct = enable_direct_play && !over_bitrate && profile.as_ref().is_none_or(|p| can_direct_play(p, f, audio_index));
        if let Some(p) = &profile {
            let has_external = field(p, "SubtitleProfiles").and_then(|v| v.as_array()).is_some_and(|l| {
                l.iter().any(|s| s["Method"] == "External" && ["vtt", "webvtt", "srt", "subrip"].contains(&s["Format"].as_str().unwrap_or("").to_lowercase().as_str()))
            });
            let embed_formats: Vec<String> = field(p, "SubtitleProfiles")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .filter(|s| s["Method"] == "Embed")
                .filter_map(|s| s["Format"].as_str().map(str::to_lowercase))
                .collect();
            if let Some(streams) = src["MediaStreams"].as_array_mut() {
                for s in streams.iter_mut().filter(|s| s["Type"] == "Subtitle") {
                    let codec = s["Codec"].as_str().unwrap_or("").to_lowercase();
                    let embed = direct && (embed_formats.contains(&codec) || (codec == "subrip" && embed_formats.iter().any(|f| f == "srt")));
                    let external = !embed && has_external && s["IsTextSubtitleStream"].as_bool().unwrap_or(false);
                    if !external {
                        s["DeliveryMethod"] = json!(if direct { "Embed" } else { "Encode" });
                        if let Some(o) = s.as_object_mut() {
                            o.remove("DeliveryUrl");
                        }
                    }
                }
            }
        }
        if !direct {
            src["SupportsDirectPlay"] = json!(false);
            src["SupportsDirectStream"] = json!(false);
            if enable_transcoding && f.probe.as_ref().is_some_and(|p| p.video.is_some()) {
                let t = profile.as_ref().map(transcoding_profile).unwrap_or(Transcode { video_codecs: vec![], audio_codecs: vec![], container: "ts".into() });
                let codec = t.video_codecs.iter().find(|c| ["h264", "hevc", "av1"].contains(&c.as_str())).cloned().unwrap_or_else(|| "h264".into());
                let mut url = format!(
                    "/videos/{item_id}/master.m3u8?MediaSourceId={msid}&PlaySessionId={play_session}&api_key={}&VideoCodec={codec}&AudioCodec={}&TranscodingContainer={}",
                    auth.token,
                    if t.audio_codecs.is_empty() { "aac".to_string() } else { t.audio_codecs.join(",") },
                    t.container
                );
                if let Some(b) = max_bitrate.filter(|b| *b > 0) {
                    url.push_str(&format!("&MaxStreamingBitrate={b}"));
                }
                if let Some(a) = audio_index {
                    url.push_str(&format!("&AudioStreamIndex={a}"));
                }
                if enable_direct_stream && !over_bitrate && f.probe.as_ref().and_then(|p| p.video.as_ref()).is_some_and(|v| t.video_codecs.contains(&v.codec.to_ascii_lowercase())) {
                    url.push_str("&AllowVideoStreamCopy=true");
                }
                src["TranscodingUrl"] = json!(url);
                src["TranscodingSubProtocol"] = json!("hls");
                src["TranscodingContainer"] = json!(t.container);
            } else {
                src["SupportsTranscoding"] = json!(false);
            }
        }
        sources.push(src);
    }
    if sources.is_empty() {
        return Err(not_found());
    }
    Ok(json(json!({ "MediaSources": sources, "PlaySessionId": play_session })))
}

fn file_id(st_files: &[File]) -> ApiResult<i64> {
    st_files.first().map(|f| f.id).ok_or_else(not_found)
}

/// The Ciniva file behind a media source id (a file id, or an item id meaning its primary version) or an item id.
async fn file_from(st: &Shared, token: &str, source: Option<&str>, item: Option<&str>) -> ApiResult<i64> {
    for id in [source, item].into_iter().flatten() {
        match Id::decode(id) {
            Some(Id { kind: Kind::File, id, .. }) => return Ok(id),
            Some(Id { kind: Kind::Movie | Kind::Episode, .. }) => {
                let (_, files, _) = files_for(st, token, id).await?;
                return file_id(&files);
            }
            _ => {}
        }
    }
    Err(not_found())
}

async fn resolve_file(st: &Shared, token: &str, item: &str, q: &Q) -> ApiResult<i64> {
    file_from(st, token, q.get("mediasourceid"), Some(item)).await
}

fn proxy_response(resp: reqwest::Response, default_type: Option<&str>) -> Response {
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::OK);
    let mut out = Response::builder().status(status);
    for name in ["content-type", "content-length", "content-range", "accept-ranges", "last-modified", "etag", "cache-control"] {
        if let Some(v) = resp.headers().get(name) {
            out = out.header(name, v.as_bytes());
        }
    }
    if resp.headers().get("content-type").is_none() {
        if let Some(t) = default_type {
            out = out.header("content-type", t);
        }
    }
    out.body(Body::from_stream(resp.bytes_stream())).unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

pub async fn stream(State(st): State<Shared>, auth: Auth, Path(params): Path<Vec<(String, String)>>, q: Q, method: Method, headers: HeaderMap) -> ApiResult {
    let id = params.iter().find(|(k, _)| k == "id").map(|(_, v)| v.clone()).ok_or_else(not_found)?;
    let fid = resolve_file(&st, &auth.token, &id, &q).await?;
    let m = if method == Method::HEAD { Method::HEAD } else { Method::GET };
    let resp = st.ciniva.raw(&auth.token, m, &format!("/files/{fid}/stream"), &headers).await?;
    Ok(proxy_response(resp, None))
}

pub async fn subtitle(State(st): State<Shared>, auth: Auth, Path(params): Path<Vec<(String, String)>>, q: Q) -> ApiResult {
    let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let item = get("id").ok_or_else(not_found)?;
    let index: i64 = get("index").and_then(|i| i.parse().ok()).ok_or_else(not_found)?;
    let format = get("format").unwrap_or_else(|| "vtt".into()).to_ascii_lowercase();
    let format = format.strip_prefix("stream.").unwrap_or(&format).to_string();
    let msid = get("msid").or_else(|| q.get("mediasourceid").map(str::to_string));
    let fid = file_from(&st, &auth.token, msid.as_deref(), Some(&item)).await?;
    let resp = st.ciniva.raw(&auth.token, Method::GET, &format!("/files/{fid}/subtitles/{index}.vtt"), &HeaderMap::new()).await?;
    let vtt = resp.text().await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e.to_string()))?;
    let (body, ctype) = if format == "srt" || format == "subrip" { (vtt_to_srt(&vtt), "application/x-subrip; charset=utf-8") } else { (vtt, "text/vtt; charset=utf-8") };
    Ok(([(header::CONTENT_TYPE, ctype)], body).into_response())
}

pub fn vtt_to_srt(vtt: &str) -> String {
    let mut out = String::new();
    let mut n = 0;
    for block in vtt.replace("\r\n", "\n").split("\n\n") {
        let lines: Vec<&str> = block.lines().collect();
        let Some(pos) = lines.iter().position(|l| l.contains("-->")) else { continue };
        n += 1;
        let timing = lines[pos]
            .split_whitespace()
            .filter(|t| t.starts_with(|c: char| c.is_ascii_digit()) || *t == "-->")
            .map(|t| if t == "-->" { t.to_string() } else { srt_time(t) })
            .collect::<Vec<_>>()
            .join(" ");
        out.push_str(&format!("{n}\n{timing}\n{}\n\n", lines[pos + 1..].join("\n")));
    }
    out
}

fn srt_time(t: &str) -> String {
    let t = t.replace('.', ",");
    if t.matches(':').count() == 1 {
        format!("00:{t}")
    } else {
        t
    }
}

fn hls_request(q: &Q, f: &File) -> Value {
    let probe = f.probe.clone().unwrap_or_default();
    let src_h = probe.video.as_ref().map(|v| v.height).unwrap_or(1080).max(1);
    let max_height = q.int("maxheight").or_else(|| q.int("height")).unwrap_or(src_h).min(src_h).max(240);
    let ceiling = if max_height >= 1600 { 40_000 } else if max_height >= 1000 { 12_000 } else if max_height >= 700 { 5_000 } else { 2_500 };
    let bitrate = q.int("videobitrate").or_else(|| q.int("maxstreamingbitrate")).map(|b| b / 1000).unwrap_or(ceiling).min(ceiling);
    let codecs = q.list("videocodec").iter().map(|s| s.to_ascii_lowercase()).collect::<Vec<_>>();
    let codec = codecs.iter().find(|c| ["h264", "hevc", "av1"].contains(&c.as_str())).cloned().unwrap_or_else(|| "h264".into());
    let audio_codecs: Vec<String> = q.list("audiocodec").iter().map(|s| s.to_ascii_lowercase()).collect();
    let audio = q.int("audiostreamindex");
    let channels = audio.and_then(|i| probe.audio.iter().find(|a| a.index == i)).or(probe.audio.first()).and_then(|a| a.channels).unwrap_or(2);
    let max_channels = q.int("maxaudiochannels").or_else(|| q.int("transcodingmaxaudiochannels")).unwrap_or(6);
    let copy_video = q.bool("allowvideostreamcopy").unwrap_or(false);
    json!({
        "max_height": max_height,
        "bitrate_kbps": bitrate.clamp(300, 120_000),
        "codec": codec,
        "hdr": false,
        "audio_stream": audio,
        "audio_channels": if channels >= 6 && max_channels >= 6 { 6 } else { 2 },
        "audio_copy": if audio_codecs.is_empty() { "aac".to_string() } else { audio_codecs.join(",") },
        "video_copy": if copy_video { probe.video.as_ref().map(|v| v.codec.clone()) } else { None },
    })
}

async fn start_transcode(st: &Shared, auth: &Auth, id: &str, q: &Q) -> ApiResult<(String, Value)> {
    let fid = resolve_file(st, &auth.token, id, q).await?;
    let f = st.ciniva.file(&auth.token, fid).await?;
    let request = hls_request(q, &f);
    let r: Value = st.ciniva.send(Some(&auth.token), Method::POST, &format!("/files/{fid}/transcode"), &request).await?.json().await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e.to_string()))?;
    let sid = r["session"].as_str().ok_or_else(|| ApiError(StatusCode::BAD_GATEWAY, "Ciniva did not start a transcode".into()))?.to_string();
    if let Some(ps) = q.get("playsessionid") {
        if let Some(old) = st.transcodes.lock().await.insert(ps.to_string(), sid.clone()) {
            if old != sid {
                let _ = st.ciniva.send(Some(&auth.token), Method::DELETE, &format!("/transcode/{old}"), &json!({})).await;
            }
        }
    }
    Ok((sid, request))
}

fn m3u8(body: String) -> Response {
    ([(header::CONTENT_TYPE, "application/vnd.apple.mpegurl"), (header::CACHE_CONTROL, "no-store")], body).into_response()
}

pub async fn master(State(st): State<Shared>, auth: Auth, Path(id): Path<String>, q: Q) -> ApiResult {
    let (sid, request) = start_transcode(&st, &auth, &id, &q).await?;
    let bandwidth = request["bitrate_kbps"].as_i64().unwrap_or(8000) * 1000 + 192_000;
    Ok(m3u8(format!("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH={bandwidth}\nhls/{sid}/index.m3u8?api_key={}\n", auth.token)))
}

pub async fn main_playlist(State(st): State<Shared>, auth: Auth, Path(id): Path<String>, q: Q) -> ApiResult {
    let (sid, _) = start_transcode(&st, &auth, &id, &q).await?;
    playlist(&st, &auth, &sid, &format!("hls/{sid}/")).await
}

async fn playlist(st: &Shared, auth: &Auth, sid: &str, prefix: &str) -> ApiResult {
    let resp = st.ciniva.raw(&auth.token, Method::GET, &format!("/transcode/{sid}/index.m3u8"), &HeaderMap::new()).await?;
    let text = resp.text().await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e.to_string()))?;
    Ok(m3u8(rewrite_playlist(&text, prefix, &auth.token)))
}

pub fn rewrite_playlist(text: &str, prefix: &str, token: &str) -> String {
    let fix = |uri: &str| {
        let base = uri.split('?').next().unwrap_or(uri);
        format!("{prefix}{base}?api_key={token}")
    };
    text.lines()
        .map(|l| {
            if l.is_empty() {
                String::new()
            } else if !l.starts_with('#') {
                fix(l)
            } else if let Some(start) = l.find("URI=\"") {
                let rest = &l[start + 5..];
                let end = rest.find('"').unwrap_or(rest.len());
                format!("{}URI=\"{}\"{}", &l[..start], fix(&rest[..end]), &rest[end..].get(1..).unwrap_or(""))
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

pub async fn hls_index(State(st): State<Shared>, auth: Auth, Path((_id, sid)): Path<(String, String)>) -> ApiResult {
    playlist(&st, &auth, &sid, "").await
}

pub async fn hls_segment(State(st): State<Shared>, auth: Auth, Path((_id, sid, seg)): Path<(String, String, String)>) -> ApiResult {
    let resp = st.ciniva.raw(&auth.token, Method::GET, &format!("/transcode/{sid}/{seg}"), &HeaderMap::new()).await?;
    Ok(proxy_response(resp, None))
}

pub async fn stop_encoding(State(st): State<Shared>, auth: Auth, q: Q) -> StatusCode {
    if let Some(ps) = q.get("playsessionid") {
        stop_session(&st, &auth.token, ps).await;
    }
    StatusCode::NO_CONTENT
}

async fn stop_session(st: &Shared, token: &str, play_session: &str) {
    let sid = st.transcodes.lock().await.remove(play_session);
    if let Some(sid) = sid {
        let _ = st.ciniva.send(Some(token), Method::DELETE, &format!("/transcode/{sid}"), &json!({})).await;
    }
}

async fn report_position(st: &Shared, token: &str, item: Option<String>, source: Option<String>, ticks: Option<i64>) -> ApiResult<()> {
    let Some(ticks) = ticks else { return Ok(()) };
    let fid = file_from(st, token, source.as_deref(), item.as_deref()).await?;
    let f = st.ciniva.file(token, fid).await?;
    let duration_ms = f.duration.or_else(|| f.probe.as_ref().and_then(|p| p.duration)).map(|d| (d * 1000.0) as i64).unwrap_or(0);
    if duration_ms <= 0 {
        return Ok(());
    }
    st.ciniva
        .send(Some(token), Method::PUT, &format!("/files/{fid}/progress"), &json!({ "position_ms": ticks_to_ms(ticks).clamp(0, duration_ms), "duration_ms": duration_ms }))
        .await?;
    Ok(())
}

pub async fn playing(State(st): State<Shared>, auth: Auth, body: Bytes) -> ApiResult<StatusCode> {
    let b = body_json(&body);
    report_position(&st, &auth.token, field_str(&b, "ItemId"), field_str(&b, "MediaSourceId"), field_i64(&b, "PositionTicks").filter(|t| *t > 0)).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn progress(State(st): State<Shared>, auth: Auth, body: Bytes) -> ApiResult<StatusCode> {
    let b = body_json(&body);
    report_position(&st, &auth.token, field_str(&b, "ItemId"), field_str(&b, "MediaSourceId"), field_i64(&b, "PositionTicks")).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn stopped(State(st): State<Shared>, auth: Auth, body: Bytes) -> ApiResult<StatusCode> {
    let b = body_json(&body);
    if let Some(ps) = field_str(&b, "PlaySessionId") {
        stop_session(&st, &auth.token, &ps).await;
    }
    if field_bool(&b, "Failed") != Some(true) {
        report_position(&st, &auth.token, field_str(&b, "ItemId"), field_str(&b, "MediaSourceId"), field_i64(&b, "PositionTicks")).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Legacy `/Users/{userId}/PlayingItems/{itemId}[/Progress]`.
pub async fn legacy_playing(State(st): State<Shared>, auth: Auth, Path((_u, item)): Path<(String, String)>, q: Q) -> ApiResult<StatusCode> {
    report_position(&st, &auth.token, Some(item), q.get("mediasourceid").map(str::to_string), q.int("positionticks")).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn mark(st: &Shared, token: &str, item: &str, watched: bool) -> ApiResult<()> {
    let files: Vec<i64> = match resolve(st, token, item).await? {
        Resolved::Movie(m) => m.files.iter().map(|f| f.id).collect(),
        Resolved::Episode(_, e) => e.files.iter().map(|f| f.id).collect(),
        Resolved::Season(s, c) => library::season_episodes(st, token, &s, &[c.season_number]).await?.iter().flat_map(|e| e.files.iter().map(|f| f.id)).collect(),
        Resolved::Series(s) => {
            let seasons: Vec<i64> = s.seasons.iter().map(|c| c.season_number).collect();
            library::season_episodes(st, token, &s, &seasons).await?.iter().flat_map(|e| e.files.iter().map(|f| f.id)).collect()
        }
        _ => return Err(not_found()),
    };
    for f in files {
        st.ciniva.send(Some(token), Method::POST, &format!("/files/{f}/watched"), &json!({ "watched": watched })).await?;
    }
    Ok(())
}

fn last_id(params: &[(String, String)]) -> ApiResult<String> {
    params.iter().find(|(k, _)| k == "id").map(|(_, v)| v.clone()).ok_or_else(not_found)
}

async fn current_user_data(st: &Shared, auth: &Auth, item: &str) -> ApiResult<Value> {
    let r = resolve(st, &auth.token, item).await?;
    let v = library::item_value(st, auth, &r).await?;
    Ok(v.get("UserData").cloned().unwrap_or_else(|| items::user_data(item, 0, 0, false)))
}

pub async fn played(State(st): State<Shared>, auth: Auth, Path(params): Path<Vec<(String, String)>>, method: Method) -> ApiResult {
    let id = last_id(&params)?;
    mark(&st, &auth.token, &id, method != Method::DELETE).await?;
    Ok(json(current_user_data(&st, &auth, &id).await?))
}

pub async fn favorite(State(st): State<Shared>, auth: Auth, Path(params): Path<Vec<(String, String)>>, method: Method) -> ApiResult {
    let id = last_id(&params)?;
    let mut data = current_user_data(&st, &auth, &id).await.unwrap_or_else(|_| items::user_data(&id, 0, 0, false));
    data["IsFavorite"] = json!(method != Method::DELETE);
    Ok(json(data))
}

pub async fn get_user_data(State(st): State<Shared>, auth: Auth, Path(params): Path<Vec<(String, String)>>) -> ApiResult {
    let id = last_id(&params)?;
    Ok(json(current_user_data(&st, &auth, &id).await?))
}

pub async fn set_user_data(State(st): State<Shared>, auth: Auth, Path(params): Path<Vec<(String, String)>>, body: Bytes) -> ApiResult {
    let id = last_id(&params)?;
    let b = body_json(&body);
    if let Some(p) = field_bool(&b, "Played") {
        mark(&st, &auth.token, &id, p).await?;
    }
    if let Some(t) = field_i64(&b, "PlaybackPositionTicks").filter(|t| *t > 0) {
        report_position(&st, &auth.token, Some(id.clone()), None, Some(t)).await?;
    }
    let mut data = current_user_data(&st, &auth, &id).await?;
    if let Some(f) = field_bool(&b, "IsFavorite") {
        data["IsFavorite"] = json!(f);
    }
    Ok(json(data))
}

pub async fn media_segments(State(st): State<Shared>, auth: Auth, Path(id): Path<String>) -> ApiResult {
    let (_, files, _) = match files_for(&st, &auth.token, &id).await {
        Ok(v) => v,
        Err(_) => return Ok(library::empty_result()),
    };
    let mut segs = Vec::new();
    if let Some(f) = files.first() {
        if let (Some(start), Some(dur)) = (f.credits_start, f.duration.or_else(|| f.probe.as_ref().and_then(|p| p.duration))) {
            if start > 0.0 && start < dur {
                segs.push(json!({
                    "Id": Id::new(Kind::File, 1, 0, f.id).encode(),
                    "ItemId": id.replace('-', "").to_ascii_lowercase(),
                    "Type": "Outro",
                    "StartTicks": secs_to_ticks(start),
                    "EndTicks": secs_to_ticks(dur),
                }));
            }
        }
    }
    let n = segs.len();
    Ok(library::query_result(segs, n, 0))
}

pub async fn bitrate_test(q: Q) -> Response {
    let size = q.int("size").unwrap_or(102_400).clamp(1, 10_000_000) as usize;
    ([(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"))], vec![0u8; size]).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ciniva::{AudioInfo, VideoInfo};

    fn file(path: &str, vcodec: &str, depth: i64, acodec: &str) -> File {
        File {
            id: 1,
            path: path.into(),
            probe: Some(MediaInfo {
                video: Some(VideoInfo { codec: vcodec.into(), width: 1920, height: 1080, bit_depth: Some(depth), profile: Some("Main 10".into()), ..Default::default() }),
                audio: vec![AudioInfo { index: 1, codec: acodec.into(), channels: Some(2), default: true, ..Default::default() }],
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn device_profile_matching() {
        let web = json!({
            "DirectPlayProfiles": [
                { "Container": "mp4,m4v", "Type": "Video", "VideoCodec": "h264,hevc", "AudioCodec": "aac,mp3" },
                { "Container": "mkv", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac" }
            ],
            "CodecProfiles": [
                { "Type": "Video", "Codec": "hevc", "Conditions": [{ "Condition": "LessThanEqual", "Property": "VideoBitDepth", "Value": "8" }] }
            ]
        });
        assert!(can_direct_play(&web, &file("/a.mkv", "h264", 8, "aac"), None));
        assert!(!can_direct_play(&web, &file("/a.mkv", "h264", 8, "dts"), None));
        assert!(!can_direct_play(&web, &file("/a.mkv", "hevc", 8, "aac"), None));
        assert!(can_direct_play(&web, &file("/a.mp4", "hevc", 8, "aac"), None));
        assert!(!can_direct_play(&web, &file("/a.mp4", "hevc", 10, "aac"), None));
        assert!(can_direct_play(&json!({}), &file("/a.avi", "mpeg4", 8, "mp3"), None));
    }

    #[test]
    fn playlist_rewrite() {
        let src = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:6.000,\n0.m4s\n#EXT-X-ENDLIST\n";
        let out = rewrite_playlist(src, "hls/abc/", "T");
        assert!(out.contains("#EXT-X-MAP:URI=\"hls/abc/init.mp4?api_key=T\"\n"));
        assert!(out.contains("\nhls/abc/0.m4s?api_key=T\n"));
    }

    #[test]
    fn srt_conversion() {
        let vtt = "WEBVTT\n\n00:01.000 --> 00:02.500\nHello\n\n01:00:00.000 --> 01:00:01.000 align:start\nBye\nnow\n";
        assert_eq!(vtt_to_srt(vtt), "1\n00:00:01,000 --> 00:00:02,500\nHello\n\n2\n01:00:00,000 --> 01:00:01,000\nBye\nnow\n\n");
    }
}
