use crate::ciniva;
use axum::{
    extract::{FromRequestParts, OptionalFromRequestParts},
    http::{header, request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

pub struct State {
    pub ciniva: ciniva::Client,
    pub server_name: String,
    pub server_id: String,
    pub public_url: Option<String>,
    pub version: String,
    /// PlaySessionId → Ciniva transcode session, so stopping playback can stop the transcode.
    pub transcodes: Mutex<HashMap<String, String>>,
}

pub type Shared = Arc<State>;

pub struct ApiError(pub StatusCode, pub String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if self.0.is_server_error() {
            tracing::warn!(status = %self.0, "{}", self.1);
        }
        (self.0, self.1).into_response()
    }
}

impl From<ciniva::Error> for ApiError {
    fn from(e: ciniva::Error) -> Self {
        ApiError(e.0, e.1)
    }
}

pub type ApiResult<T = Response> = Result<T, ApiError>;

pub fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "not found".into())
}

pub fn json(v: serde_json::Value) -> Response {
    Json(v).into_response()
}

/// `key="value"` pairs of a `MediaBrowser …` / `Emby …` authorization header.
pub fn parse_auth_header(value: &str) -> HashMap<String, String> {
    let rest = value.trim();
    let rest = match rest.split_once(' ') {
        Some((scheme, rest)) if !scheme.contains('=') => rest,
        _ => rest,
    };
    let mut out = HashMap::new();
    for part in split_params(rest) {
        if let Some((k, v)) = part.split_once('=') {
            let v = v.trim().trim_matches('"');
            out.insert(k.trim().to_ascii_lowercase(), percent_decode(v));
        }
    }
    out
}

fn split_params(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ',' if !quoted => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    parts.push(cur);
    parts
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Query parameters with lower-cased keys; repeated keys and comma lists are both supported.
#[derive(Debug, Default, Clone)]
pub struct Q(pub HashMap<String, Vec<String>>);

impl Q {
    pub fn parse(query: Option<&str>) -> Q {
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for pair in query.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            map.entry(percent_decode(k).to_ascii_lowercase()).or_default().push(percent_decode(v));
        }
        Q(map)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.last()).map(String::as_str).filter(|s| !s.is_empty())
    }

    pub fn list(&self, key: &str) -> Vec<String> {
        self.0
            .get(key)
            .into_iter()
            .flatten()
            .flat_map(|v| v.split([',', '|']))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(|v| v.parse().ok())
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        self.get(key).map(|v| v.eq_ignore_ascii_case("true") || v == "1")
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Q {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Q::parse(parts.uri.query()))
    }
}

/// The caller's Jellyfin client description and Ciniva session token.
#[derive(Debug, Clone, Default)]
pub struct Client {
    pub token: Option<String>,
    pub client: Option<String>,
    pub device: Option<String>,
    pub device_id: Option<String>,
    pub version: Option<String>,
}

impl Client {
    pub fn from_parts(headers: &HeaderMap, query: &Q) -> Client {
        let mut c = Client::default();
        for name in ["x-emby-authorization", "authorization"] {
            if let Some(v) = headers.get(name).and_then(|v| v.to_str().ok()) {
                if let Some(t) = v.strip_prefix("Bearer ") {
                    c.token.get_or_insert_with(|| t.trim().to_string());
                    continue;
                }
                let p = parse_auth_header(v);
                c.token = c.token.or_else(|| p.get("token").cloned().filter(|t| !t.is_empty()));
                c.client = c.client.or_else(|| p.get("client").cloned());
                c.device = c.device.or_else(|| p.get("device").cloned());
                c.device_id = c.device_id.or_else(|| p.get("deviceid").cloned());
                c.version = c.version.or_else(|| p.get("version").cloned());
            }
        }
        for name in ["x-emby-token", "x-mediabrowser-token"] {
            if c.token.is_none() {
                c.token = headers.get(name).and_then(|v| v.to_str().ok()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            }
        }
        if c.token.is_none() {
            c.token = query.get("api_key").or_else(|| query.get("apikey")).map(str::to_string);
        }
        c.client = c.client.or_else(|| headers.get("x-emby-client").and_then(|v| v.to_str().ok()).map(str::to_string));
        c.device = c.device.or_else(|| headers.get("x-emby-device-name").and_then(|v| v.to_str().ok()).map(str::to_string));
        c.device_id = c.device_id.or_else(|| headers.get("x-emby-device-id").and_then(|v| v.to_str().ok()).map(str::to_string));
        c
    }
}

impl<S: Send + Sync> OptionalFromRequestParts<S> for Client {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Option<Self>, Self::Rejection> {
        Ok(Some(Client::from_parts(&parts.headers, &Q::parse(parts.uri.query()))))
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Client {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Client::from_parts(&parts.headers, &Q::parse(parts.uri.query())))
    }
}

/// A request that must carry a token.
pub struct Auth {
    pub token: String,
    pub client: Client,
}

impl<S: Send + Sync> FromRequestParts<S> for Auth {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let client = Client::from_parts(&parts.headers, &Q::parse(parts.uri.query()));
        match client.token.clone() {
            Some(token) => Ok(Auth { token, client }),
            None => Err((StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, "MediaBrowser")], "authorization required").into_response()),
        }
    }
}

/// Case-insensitive field lookup in a JSON request body (clients send PascalCase or camelCase).
pub fn field<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    v.as_object()?.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v).filter(|v| !v.is_null())
}

pub fn field_str(v: &serde_json::Value, key: &str) -> Option<String> {
    match field(v, key)? {
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

pub fn field_i64(v: &serde_json::Value, key: &str) -> Option<i64> {
    match field(v, key)? {
        serde_json::Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

pub fn field_bool(v: &serde_json::Value, key: &str) -> Option<bool> {
    match field(v, key)? {
        serde_json::Value::Bool(b) => Some(*b),
        serde_json::Value::String(s) => Some(s.eq_ignore_ascii_case("true")),
        _ => None,
    }
}

pub fn body_json(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice(bytes).unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mediabrowser_header() {
        let p = parse_auth_header(r#"MediaBrowser Client="Jellyfin Web", Device="Chrome", DeviceId="abc%3D%3D", Version="10.10.7", Token="tok123""#);
        assert_eq!(p["client"], "Jellyfin Web");
        assert_eq!(p["deviceid"], "abc==");
        assert_eq!(p["token"], "tok123");
    }

    #[test]
    fn query_lists_and_case() {
        let q = Q::parse(Some("IncludeItemTypes=Movie,Series&includeItemTypes=Episode&Limit=5&api_key=x&Fields="));
        assert_eq!(q.list("includeitemtypes"), vec!["Movie", "Series", "Episode"]);
        assert_eq!(q.int("limit"), Some(5));
        assert_eq!(q.get("fields"), None);
    }

    #[test]
    fn token_sources() {
        let mut h = HeaderMap::new();
        h.insert("x-emby-token", "a".parse().unwrap());
        assert_eq!(Client::from_parts(&h, &Q::default()).token.as_deref(), Some("a"));
        assert_eq!(Client::from_parts(&HeaderMap::new(), &Q::parse(Some("ApiKey=b"))).token.as_deref(), Some("b"));
        let mut h = HeaderMap::new();
        h.insert("authorization", r#"MediaBrowser Client="Swiftfin", Token="c""#.parse().unwrap());
        assert_eq!(Client::from_parts(&h, &Q::parse(Some("api_key=z"))).token.as_deref(), Some("c"));
    }
}
