mod auth;

use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use std::str::FromStr;
use tracing::info;

use crate::error::{DbError, ServirError};

pub(crate) use auth::AuthRepository;

/// Thin wrapper around a SQLite connection pool.
#[derive(Debug, Clone)]
pub(crate) struct Db {
    pool: SqlitePool,
}

impl Db {
    /// Connects to the SQLite database at `url`, creating the file if missing.
    pub async fn connect(url: &str) -> Result<Self, ServirError> {
        let opts = SqliteConnectOptions::from_str(url)
            .map_err(|e| DbError::connection(url, e))?
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts)
            .await
            .map_err(|e| DbError::connection(url, e))?;
        Ok(Self { pool })
    }

    /// Wraps an existing pool (useful for tests with in-memory databases).
    #[cfg(test)]
    pub(crate) fn from_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Runs the auth schema migrations and logs the current version.
    pub(crate) async fn migrate_auth(&self) -> Result<(), ServirError> {
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

        info!(version, "auth schema up to date");
        Ok(())
    }
}
