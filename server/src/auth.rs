//! Accounts and sessions. Passwords are Argon2 hashes; session tokens are random and stored only as SHA-256.

use anyhow::{anyhow, Result};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand::RngCore;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, SqlitePool};

pub const COOKIE: &str = "ciniva_session";

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    #[sqlx(rename = "is_admin")]
    pub admin: bool,
}

/// Who a request is from: a signed-in user, or a plugin/service holding the shared service key.
#[derive(Debug, Clone)]
pub enum Identity {
    User(User),
    Service,
}

impl Identity {
    pub fn is_admin(&self) -> bool {
        match self {
            Identity::User(u) => u.admin,
            Identity::Service => true,
        }
    }

    pub fn user(&self) -> Option<&User> {
        match self {
            Identity::User(u) => Some(u),
            Identity::Service => None,
        }
    }
}

pub async fn hash_password(password: String) -> Result<String> {
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default().hash_password(password.as_bytes(), &salt).map(|h| h.to_string()).map_err(|e| anyhow!("{e}"))
    })
    .await?
}

pub async fn verify_password(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash).is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
    })
    .await
    .unwrap_or(false)
}

pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn validate_credentials(username: &str, password: &str) -> Result<(), String> {
    let name = username.trim();
    if name.is_empty() || name.len() > 64 || name.chars().any(|c| c.is_control()) {
        return Err("Choose a username of 1 to 64 characters".into());
    }
    if password.chars().count() < 8 {
        return Err("Use a password of at least 8 characters".into());
    }
    Ok(())
}

pub async fn user_count(db: &SqlitePool) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(db).await?)
}

pub async fn create_user(db: &SqlitePool, username: &str, password: &str, admin: bool) -> Result<User> {
    let hash = hash_password(password.to_string()).await?;
    let id: i64 = sqlx::query_scalar("INSERT INTO users (username, password_hash, is_admin) VALUES (?, ?, ?) RETURNING id")
        .bind(username.trim())
        .bind(hash)
        .bind(admin)
        .fetch_one(db)
        .await?;
    Ok(User { id, username: username.trim().to_string(), admin })
}

pub async fn login(db: &SqlitePool, username: &str, password: &str) -> Result<Option<User>> {
    let row: Option<(i64, String, String, bool)> =
        sqlx::query_as("SELECT id, username, password_hash, is_admin FROM users WHERE username = ?").bind(username.trim()).fetch_optional(db).await?;
    // Hash even for unknown users so response time does not reveal which usernames exist.
    let (row, hash) = match row {
        Some((id, name, hash, admin)) => (Some(User { id, username: name, admin }), hash),
        None => (None, "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$7Ya6LMoZ3UO5wnPC1kvxQbcqHkm8Pp9mByWvBkXMHnM".to_string()),
    };
    let ok = verify_password(password.to_string(), hash).await;
    Ok(row.filter(|_| ok))
}

pub async fn create_session(db: &SqlitePool, user_id: i64, client: Option<&str>) -> Result<String> {
    let token = random_token();
    sqlx::query("INSERT INTO sessions (token_hash, user_id, client) VALUES (?, ?, ?)")
        .bind(token_hash(&token))
        .bind(user_id)
        .bind(client.map(|c| c.chars().take(120).collect::<String>()))
        .execute(db)
        .await?;
    Ok(token)
}

pub async fn session_user(db: &SqlitePool, token: &str) -> Option<User> {
    let hash = token_hash(token);
    let user: Option<User> = sqlx::query_as(
        "SELECT u.id, u.username, u.is_admin FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.token_hash = ?",
    )
    .bind(&hash)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    if user.is_some() {
        let _ = sqlx::query("UPDATE sessions SET last_seen = datetime('now') WHERE token_hash = ? AND last_seen < datetime('now', '-5 minutes')")
            .bind(&hash)
            .execute(db)
            .await;
    }
    user
}

pub async fn end_session(db: &SqlitePool, token: &str) -> Result<()> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = ?").bind(token_hash(token)).execute(db).await?;
    Ok(())
}

pub async fn setting(db: &SqlitePool, key: &str) -> Option<String> {
    sqlx::query_scalar("SELECT value FROM settings WHERE key = ?").bind(key).fetch_optional(db).await.ok().flatten()
}

pub async fn set_setting(db: &SqlitePool, key: &str, value: &str) -> Result<()> {
    sqlx::query("INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value")
        .bind(key)
        .bind(value)
        .execute(db)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn password_roundtrip() {
        let h = hash_password("correct horse".into()).await.unwrap();
        assert!(verify_password("correct horse".into(), h.clone()).await);
        assert!(!verify_password("wrong".into(), h).await);
    }

    #[test]
    fn credential_rules() {
        assert!(validate_credentials("solo", "12345678").is_ok());
        assert!(validate_credentials("", "12345678").is_err());
        assert!(validate_credentials("solo", "short").is_err());
    }
}
