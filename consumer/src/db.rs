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

    info!(user_id, username, "created user profile");

    Ok(UserProfile {
        id: user_id.to_string(),
        username: username.to_string(),
        created_at: now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> SqlitePool {
        let pool = connect("sqlite::memory:").await.unwrap();
        migrate(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn connect_and_migrate() {
        let pool = test_pool().await;
        // Verify the migration created the table.
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_profiles")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn create_profile_on_first_access() {
        let pool = test_pool().await;
        let profile = find_or_create_profile(&pool, "user-1", "alice")
            .await
            .unwrap();

        assert_eq!(profile.id, "user-1");
        assert_eq!(profile.username, "alice");
        assert!(profile.created_at > 0);
    }

    #[tokio::test]
    async fn find_existing_profile() {
        let pool = test_pool().await;

        // First call creates.
        let created = find_or_create_profile(&pool, "user-2", "bob")
            .await
            .unwrap();

        // Second call finds.
        let found = find_or_create_profile(&pool, "user-2", "bob")
            .await
            .unwrap();

        assert_eq!(created.id, found.id);
        assert_eq!(created.username, found.username);
        assert_eq!(created.created_at, found.created_at);
    }

    #[tokio::test]
    async fn different_users_get_separate_profiles() {
        let pool = test_pool().await;

        let alice = find_or_create_profile(&pool, "u1", "alice").await.unwrap();
        let bob = find_or_create_profile(&pool, "u2", "bob").await.unwrap();

        assert_ne!(alice.id, bob.id);
        assert_eq!(alice.username, "alice");
        assert_eq!(bob.username, "bob");
    }
}
