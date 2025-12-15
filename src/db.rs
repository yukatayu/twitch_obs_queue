use std::path::Path;

use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    FromRow, SqlitePool,
};

use crate::util;

#[derive(Debug, Clone)]
pub struct OAuthToken {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
}

#[derive(Debug, FromRow)]
struct OAuthTokenRow {
    access_token: String,
    refresh_token: String,
    expires_at: i64,
}

pub async fn init_pool(db_path: &str) -> anyhow::Result<SqlitePool> {
    if let Some(parent) = Path::new(db_path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    // let url = format!("sqlite://{}", db_path);
    // let pool = SqlitePoolOptions::new()
    //     .max_connections(5)
    //     .connect(&url)
    //     .await?;

    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    sqlx::migrate!("./migrations").run(&pool).await?;

    Ok(pool)
}

pub async fn get_oauth_token(pool: &SqlitePool) -> anyhow::Result<Option<OAuthToken>> {
    let row = sqlx::query_as::<_, OAuthTokenRow>(
        r#"SELECT access_token, refresh_token, expires_at
           FROM oauth_tokens
           WHERE id = 1"#,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| OAuthToken {
        access_token: r.access_token,
        refresh_token: r.refresh_token,
        expires_at: r.expires_at,
    }))
}

pub async fn upsert_oauth_token(pool: &SqlitePool, token: &OAuthToken) -> anyhow::Result<()> {
    sqlx::query(
        r#"INSERT INTO oauth_tokens (id, access_token, refresh_token, expires_at)
           VALUES (1, ?1, ?2, ?3)
           ON CONFLICT(id) DO UPDATE SET
             access_token = excluded.access_token,
             refresh_token = excluded.refresh_token,
             expires_at = excluded.expires_at"#,
    )
    .bind(&token.access_token)
    .bind(&token.refresh_token)
    .bind(token.expires_at)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn delete_oauth_token(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM oauth_tokens WHERE id = 1")
        .execute(pool)
        .await?;
    Ok(())
}

#[derive(Debug, FromRow)]
struct KvRow {
    value: String,
}

pub async fn get_kv(pool: &SqlitePool, key: &str) -> anyhow::Result<Option<String>> {
    let row = sqlx::query_as::<_, KvRow>(
        r#"SELECT value
           FROM app_kv
           WHERE key = ?1"#,
    )
    .bind(key)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| r.value))
}

pub async fn set_kv(pool: &SqlitePool, key: &str, value: &str) -> anyhow::Result<()> {
    sqlx::query(
        r#"INSERT INTO app_kv (key, value)
           VALUES (?1, ?2)
           ON CONFLICT(key) DO UPDATE SET
             value = excluded.value"#,
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;

    Ok(())
}

#[derive(Debug, FromRow)]
struct MessageRow {
    message_id: String,
}

pub async fn is_processed_message(pool: &SqlitePool, message_id: &str) -> anyhow::Result<bool> {
    let row = sqlx::query_as::<_, MessageRow>(
        r#"SELECT message_id
           FROM processed_messages
           WHERE message_id = ?1"#,
    )
    .bind(message_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.is_some())
}

pub async fn mark_processed_message(
    pool: &SqlitePool,
    message_id: &str,
    received_at: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"INSERT OR IGNORE INTO processed_messages (message_id, received_at)
           VALUES (?1, ?2)"#,
    )
    .bind(message_id)
    .bind(received_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn cleanup_processed_messages(pool: &SqlitePool, cutoff: i64) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"DELETE FROM processed_messages
           WHERE received_at < ?1"#,
    )
    .bind(cutoff)
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

pub async fn get_broadcaster_id(pool: &SqlitePool) -> anyhow::Result<Option<String>> {
    get_kv(pool, "broadcaster_id").await
}

pub async fn set_broadcaster_id(pool: &SqlitePool, id: &str) -> anyhow::Result<()> {
    set_kv(pool, "broadcaster_id", id).await
}

pub async fn get_broadcaster_login(pool: &SqlitePool) -> anyhow::Result<Option<String>> {
    get_kv(pool, "broadcaster_login").await
}

pub async fn set_broadcaster_login(pool: &SqlitePool, login: &str) -> anyhow::Result<()> {
    set_kv(pool, "broadcaster_login", login).await
}

/// Convenience: returns true if we have a token and it looks non-expired.
pub async fn has_validish_token(pool: &SqlitePool) -> anyhow::Result<bool> {
    let Some(t) = get_oauth_token(pool).await? else {
        return Ok(false);
    };
    Ok(t.expires_at > util::now_epoch() + 30)
}

// --- Twitch user cache ------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CachedUserProfile {
    pub user_id: String,
    pub user_login: String,
    pub display_name: String,
    pub profile_image_url: String,
    pub updated_at: i64,
}

#[derive(Debug, FromRow)]
struct CachedUserProfileRow {
    user_id: String,
    user_login: String,
    display_name: String,
    profile_image_url: String,
    updated_at: i64,
}

pub async fn get_cached_user_profile(
    pool: &SqlitePool,
    user_id: &str,
) -> anyhow::Result<Option<CachedUserProfile>> {
    let row = sqlx::query_as::<_, CachedUserProfileRow>(
        r#"SELECT user_id, user_login, display_name, profile_image_url, updated_at
           FROM user_cache
           WHERE user_id = ?1"#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| CachedUserProfile {
        user_id: r.user_id,
        user_login: r.user_login,
        display_name: r.display_name,
        profile_image_url: r.profile_image_url,
        updated_at: r.updated_at,
    }))
}

pub async fn upsert_cached_user_profile(
    pool: &SqlitePool,
    profile: &CachedUserProfile,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"INSERT INTO user_cache (user_id, user_login, display_name, profile_image_url, updated_at)
           VALUES (?1, ?2, ?3, ?4, ?5)
           ON CONFLICT(user_id) DO UPDATE SET
             user_login = excluded.user_login,
             display_name = excluded.display_name,
             profile_image_url = excluded.profile_image_url,
             updated_at = excluded.updated_at"#,
    )
    .bind(&profile.user_id)
    .bind(&profile.user_login)
    .bind(&profile.display_name)
    .bind(&profile.profile_image_url)
    .bind(profile.updated_at)
    .execute(pool)
    .await?;

    Ok(())
}
