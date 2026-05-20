use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use servir::{DbError, ServirError};
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use tracing::info;

// ---------------------------------Types----------------------------------

/// A user's app-level profile. Keyed by auth user ID.
#[derive(Debug, Clone, Serialize)]
pub struct UserProfile {
    pub id: String,
    pub username: String,
    pub created_at: i64,
}

// ---------------------------------Db----------------------------------

/// Thin wrapper around a SQLite connection pool for app-specific data.
#[derive(Debug, Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// Connects to the app database, creating the file if it does not exist.
    pub async fn connect(database_url: &str) -> Result<Self, ServirError> {
        let opts = SqliteConnectOptions::from_str(database_url)
            .map_err(|e| DbError::connection(database_url, e))?
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts)
            .await
            .map_err(|e| DbError::connection(database_url, e))?;
        Ok(Self { pool })
    }

    /// Wraps an existing pool (useful for tests with in-memory databases).
    #[cfg(test)]
    fn from_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Runs the consumer's migrations and logs the current schema version.
    pub async fn migrate(&self) -> Result<(), ServirError> {
        sqlx::migrate!("src/db/migrations")
            .run(&self.pool)
            .await
            .map_err(DbError::migration)?;

        let version: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success = 1",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);

        info!(version, "app schema up to date");
        Ok(())
    }

    /// Returns the user's app profile, creating it with defaults on first access.
    pub async fn find_or_create_profile(
        &self,
        user_id: &str,
        username: &str,
    ) -> Result<UserProfile, ServirError> {
        if let Some(row) = sqlx::query_as::<_, (String, String, i64)>(
            "SELECT id, username, created_at FROM user_profiles WHERE id = ?",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DbError::unknown("find_user_profile", e))?
        {
            return Ok(UserProfile {
                id: row.0,
                username: row.1,
                created_at: row.2,
            });
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before epoch")
            .as_secs() as i64;

        sqlx::query("INSERT INTO user_profiles (id, username, created_at) VALUES (?, ?, ?)")
            .bind(user_id)
            .bind(username)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(|e| DbError::unknown("insert_user_profile", e))?;

        info!(user_id, username, "created user profile");

        Ok(UserProfile {
            id: user_id.to_string(),
            username: username.to_string(),
            created_at: now,
        })
    }
}

// ---------------------------------Tests----------------------------------

#[cfg(test)]
mod tests {
    use sqlx::Sqlite;
    use sqlx::pool::PoolOptions;
    use sqlx::sqlite::SqliteConnectOptions;

    use super::*;

    async fn test_db() -> Db {
        let pool = PoolOptions::<Sqlite>::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(":memory:")
                    .create_if_missing(true),
            )
            .await
            .unwrap();

        let db = Db::from_pool(pool);
        db.migrate().await.unwrap();
        db
    }

    #[tokio::test]
    async fn connect_and_migrate() {
        let db = test_db().await;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_profiles")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn create_profile_on_first_access() {
        let db = test_db().await;
        let profile = db.find_or_create_profile("user-1", "alice").await.unwrap();

        assert_eq!(profile.id, "user-1");
        assert_eq!(profile.username, "alice");
        assert!(profile.created_at > 0);
    }

    #[tokio::test]
    async fn find_existing_profile() {
        let db = test_db().await;

        let created = db.find_or_create_profile("user-2", "bob").await.unwrap();
        let found = db.find_or_create_profile("user-2", "bob").await.unwrap();

        assert_eq!(created.id, found.id);
        assert_eq!(created.username, found.username);
        assert_eq!(created.created_at, found.created_at);
    }

    #[tokio::test]
    async fn different_users_get_separate_profiles() {
        let db = test_db().await;

        let alice = db.find_or_create_profile("u1", "alice").await.unwrap();
        let bob = db.find_or_create_profile("u2", "bob").await.unwrap();

        assert_ne!(alice.id, bob.id);
        assert_eq!(alice.username, "alice");
        assert_eq!(bob.username, "bob");
    }
}
