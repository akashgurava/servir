use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use servir::{DbError, ServirError};
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use tracing::info;

/// Connects to the app database, creating the file if it does not exist.
pub async fn connect(database_url: &str) -> Result<SqlitePool, ServirError> {
    let opts = SqliteConnectOptions::from_str(database_url)
        .map_err(|e| DbError::connection(database_url, e))?
        .create_if_missing(true);
    SqlitePool::connect_with(opts)
        .await
        .map_err(|e| DbError::connection(database_url, e))
}

/// Runs the consumer's migrations and logs the current schema version.
///
/// Aborts with an error if a previously-applied migration file has been
/// modified (checksum mismatch).
pub async fn migrate(pool: &SqlitePool) -> Result<(), ServirError> {
    sqlx::migrate!()
        .run(pool)
        .await
        .map_err(DbError::migration)?;

    let version: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success = 1",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    info!(version, "app schema up to date");
    Ok(())
}

/// A user's app-level profile. Keyed by auth user ID.
#[derive(Debug, Clone, Serialize)]
pub struct UserProfile {
    pub id: String,
    pub username: String,
    pub created_at: i64,
}

/// Returns the user's app profile, creating it with defaults on first access.
pub async fn find_or_create_profile(
    pool: &SqlitePool,
    user_id: &str,
    username: &str,
) -> Result<UserProfile, ServirError> {
    // Try to find existing profile.
    if let Some(row) = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT id, username, created_at FROM user_profiles WHERE id = ?",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| DbError::unknown("find_user_profile", e))?
    {
        return Ok(UserProfile {
            id: row.0,
            username: row.1,
            created_at: row.2,
        });
    }

    // First access — create profile with auth-supplied defaults.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_secs() as i64;

    sqlx::query("INSERT INTO user_profiles (id, username, created_at) VALUES (?, ?, ?)")
        .bind(user_id)
        .bind(username)
        .bind(now)
        .execute(pool)
        .await
        .map_err(|e| DbError::unknown("insert_user_profile", e))?;

    Ok(UserProfile {
        id: user_id.to_string(),
        username: username.to_string(),
        created_at: now,
    })
}
