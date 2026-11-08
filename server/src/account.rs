//! First-run setup, sign-in and account management.

use crate::{
    api::{ApiError, Shared},
    auth::{self, Identity},
};
use axum::{
    extract::{Path, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Extension, Json, Router,
};
use serde::Deserialize;
use serde_json::json;

pub fn public_routes() -> Router<Shared> {
    Router::new()
        .route("/setup", get(setup_state))
        .route("/setup/admin", post(setup_admin))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
}

pub fn private_routes() -> Router<Shared> {
    Router::new()
        .route("/me", get(me))
        .route("/me/password", put(change_password))
        .route("/settings/tmdb", put(set_tmdb))
        .route("/users", get(list_users).post(add_user))
        .route("/users/{id}", delete(remove_user))
}

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, msg.into())
}

fn forbidden() -> ApiError {
    ApiError(StatusCode::FORBIDDEN, "administrator only".into())
}

fn session_cookie(token: &str) -> HeaderValue {
    HeaderValue::from_str(&format!("{}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000", auth::COOKIE)).unwrap()
}

fn signed_in(token: String, user: &auth::User) -> Response {
    let mut resp = Json(json!({ "token": token, "user": user })).into_response();
    resp.headers_mut().insert(header::SET_COOKIE, session_cookie(&token));
    resp
}

async fn setup_state(State(ctx): State<Shared>) -> Result<Json<serde_json::Value>, ApiError> {
    let users = auth::user_count(&ctx.db).await?;
    let stored = auth::setting(&ctx.db, "tmdb_api_key").await.is_some();
    Ok(Json(json!({
        "needs_admin": users == 0,
        "tmdb_configured": ctx.tmdb().await.is_some(),
        "tmdb_from_env": !stored && ctx.cfg.tmdb_key.is_some(),
        "version": env!("CARGO_PKG_VERSION"),
    })))
}

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: String,
    #[serde(default)]
    client: Option<String>,
}

async fn setup_admin(State(ctx): State<Shared>, headers: HeaderMap, Json(c): Json<Credentials>) -> Result<Response, ApiError> {
    auth::validate_credentials(&c.username, &c.password).map_err(bad)?;
    let _guard = ctx.setup_lock.lock().await;
    if auth::user_count(&ctx.db).await? > 0 {
        return Err(ApiError(StatusCode::CONFLICT, "Ciniva is already set up; sign in instead".into()));
    }
    let user = auth::create_user(&ctx.db, &c.username, &c.password, true).await?;
    tracing::info!(user = %user.username, "administrator account created");
    let client = c.client.or_else(|| user_agent(&headers));
    let token = auth::create_session(&ctx.db, user.id, client.as_deref()).await?;
    Ok(signed_in(token, &user))
}

fn user_agent(headers: &HeaderMap) -> Option<String> {
    headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).map(str::to_string)
}

async fn login(State(ctx): State<Shared>, headers: HeaderMap, Json(c): Json<Credentials>) -> Result<Response, ApiError> {
    match auth::login(&ctx.db, &c.username, &c.password).await? {
        Some(user) => {
            let client = c.client.or_else(|| user_agent(&headers));
            let token = auth::create_session(&ctx.db, user.id, client.as_deref()).await?;
            Ok(signed_in(token, &user))
        }
        None => {
            // Slows password guessing without any per-client state.
            tokio::time::sleep(std::time::Duration::from_millis(750)).await;
            Err(ApiError(StatusCode::UNAUTHORIZED, "Wrong username or password".into()))
        }
    }
}

async fn logout(State(ctx): State<Shared>, req: Request) -> Result<Response, ApiError> {
    if let Some(token) = crate::api::request_token(&req) {
        auth::end_session(&ctx.db, &token).await?;
    }
    let mut resp = StatusCode::NO_CONTENT.into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!("{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0", auth::COOKIE)).unwrap(),
    );
    Ok(resp)
}

async fn me(Extension(id): Extension<Identity>) -> Json<serde_json::Value> {
    match id {
        Identity::User(u) => Json(json!({ "user": u })),
        Identity::Service => Json(json!({ "service": true })),
    }
}

#[derive(Deserialize)]
struct PasswordChange {
    current: String,
    new: String,
}

async fn change_password(State(ctx): State<Shared>, Extension(id): Extension<Identity>, Json(b): Json<PasswordChange>) -> Result<StatusCode, ApiError> {
    let user = id.user().ok_or_else(forbidden)?;
    if auth::login(&ctx.db, &user.username, &b.current).await?.is_none() {
        return Err(bad("Current password is wrong"));
    }
    auth::validate_credentials(&user.username, &b.new).map_err(bad)?;
    let hash = auth::hash_password(b.new).await?;
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?").bind(hash).bind(user.id).execute(&ctx.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct TmdbKey {
    api_key: String,
}

async fn set_tmdb(State(ctx): State<Shared>, Extension(id): Extension<Identity>, Json(b): Json<TmdbKey>) -> Result<StatusCode, ApiError> {
    if !id.is_admin() {
        return Err(forbidden());
    }
    let key = b.api_key.trim().to_string();
    if key.is_empty() {
        return Err(bad("Paste a TMDB API key or read access token"));
    }
    crate::tmdb::Tmdb::new(key.clone(), ctx.cfg.language.clone())
        .verify()
        .await
        .map_err(|e| bad(format!("TMDB rejected this key: {e:#}")))?;
    auth::set_setting(&ctx.db, "tmdb_api_key", &key).await?;
    ctx.set_tmdb_key(Some(key)).await;
    let scan = ctx.clone();
    tokio::spawn(async move {
        if let Err(e) = crate::library::scan(&scan).await {
            tracing::error!("scan failed: {e:#}");
        }
    });
    Ok(StatusCode::NO_CONTENT)
}

async fn list_users(State(ctx): State<Shared>, Extension(id): Extension<Identity>) -> Result<Json<Vec<auth::User>>, ApiError> {
    if !id.is_admin() {
        return Err(forbidden());
    }
    Ok(Json(sqlx::query_as("SELECT id, username, is_admin FROM users ORDER BY id").fetch_all(&ctx.db).await?))
}

#[derive(Deserialize)]
struct NewUser {
    username: String,
    password: String,
    #[serde(default)]
    admin: bool,
}

async fn add_user(State(ctx): State<Shared>, Extension(id): Extension<Identity>, Json(b): Json<NewUser>) -> Result<Json<auth::User>, ApiError> {
    if !id.is_admin() {
        return Err(forbidden());
    }
    auth::validate_credentials(&b.username, &b.password).map_err(bad)?;
    auth::create_user(&ctx.db, &b.username, &b.password, b.admin)
        .await
        .map(Json)
        .map_err(|_| ApiError(StatusCode::CONFLICT, "That username is taken".into()))
}

async fn remove_user(State(ctx): State<Shared>, Extension(id): Extension<Identity>, Path(uid): Path<i64>) -> Result<StatusCode, ApiError> {
    if !id.is_admin() {
        return Err(forbidden());
    }
    if id.user().is_some_and(|u| u.id == uid) {
        return Err(bad("You can't remove your own account"));
    }
    sqlx::query("DELETE FROM users WHERE id = ?").bind(uid).execute(&ctx.db).await?;
    Ok(StatusCode::NO_CONTENT)
}
