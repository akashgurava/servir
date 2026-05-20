use std::time::{SystemTime, UNIX_EPOCH};

use tracing::info;
use uuid::Uuid;

use super::Db;
use crate::error::{AuthUserError, DbError, ServirError};

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before Unix epoch")
        .as_secs()
}

/// Repository interface for auth-related database operations.
///
/// Implement this trait for alternative backing stores (e.g. Postgres, Turso).
pub(crate) trait AuthRepository: Send + Sync {
    /// Inserts a new user with the given username and pre-hashed password.
    ///
    /// Returns the created [`User`]. Fails with [`AuthUserError::AlreadyExists`] if
    /// the username is taken.
    async fn create_user(&self, username: &str, password_hash: &str) -> Result<User, ServirError>;

    /// Looks up a user by their unique username. Returns `None` if not found.
    async fn find_user_by_username(&self, username: &str) -> Result<Option<User>, ServirError>;

    /// Looks up a user by their unique ID. Returns `None` if not found.
    async fn find_user_by_id(&self, user_id: &str) -> Result<Option<User>, ServirError>;

    /// Persists a refresh token's `jti` so it can be validated on future refresh requests.
    ///
    /// The token expires after `ttl_secs` seconds from now.
    async fn store_refresh_token(
        &self,
        jti: &str,
        user_id: &str,
        username: &str,
        ttl_secs: u64,
    ) -> Result<(), ServirError>;

    /// Atomically consumes a refresh token. Returns `true` if it existed and was not expired.
    async fn consume_refresh_token(&self, jti: &str) -> Result<bool, ServirError>;

    /// Revokes all refresh tokens for the given user (e.g. on logout).
    async fn delete_refresh_tokens_for_user(&self, user_id: &str) -> Result<(), ServirError>;

    /// Loads the signing secret from persistent storage, or generates and stores one.
    async fn load_or_generate_secret(&self) -> Result<String, ServirError>;
}

// ---------------------------------SQLite implementation----------------------------------

#[derive(sqlx::FromRow)]
pub(crate) struct User {
    id: String,
    username: String,
    password_hash: String,
}

impl User {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn username(&self) -> &str {
        &self.username
    }

    pub(crate) fn password_hash(&self) -> &str {
        &self.password_hash
    }
}

impl AuthRepository for Db {
    async fn create_user(&self, username: &str, password_hash: &str) -> Result<User, ServirError> {
        let id = Uuid::new_v4().to_string();

        sqlx::query(
            "INSERT INTO users (id, username, password_hash, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(username)
        .bind(password_hash)
        .bind(now_secs() as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(db) if db.is_unique_violation() => {
                AuthUserError::already_exists(username, &e)
            }
            _ => DbError::user_create(username, &e),
        })?;

        Ok(User {
            id,
            username: username.to_string(),
            password_hash: password_hash.to_string(),
        })
    }

    async fn find_user_by_username(&self, username: &str) -> Result<Option<User>, ServirError> {
        sqlx::query_as::<_, User>(
            "SELECT id, username, password_hash FROM users WHERE username = ?",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DbError::user_find_by_username(username, e))
    }

    async fn find_user_by_id(&self, user_id: &str) -> Result<Option<User>, ServirError> {
        sqlx::query_as::<_, User>("SELECT id, username, password_hash FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| DbError::user_find_by_id(user_id, e))
    }

    async fn store_refresh_token(
        &self,
        jti: &str,
        user_id: &str,
        username: &str,
        ttl_secs: u64,
    ) -> Result<(), ServirError> {
        let expires_at = now_secs() as i64 + ttl_secs as i64;
        sqlx::query(
            "INSERT INTO refresh_tokens (jti, user_id, expires_at, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(jti)
        .bind(user_id)
        .bind(expires_at)
        .bind(now_secs() as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::store_refresh_token(username, e))?;
        Ok(())
    }

    async fn consume_refresh_token(&self, jti: &str) -> Result<bool, ServirError> {
        let result = sqlx::query("DELETE FROM refresh_tokens WHERE jti = ? AND expires_at > ?")
            .bind(jti)
            .bind(now_secs() as i64)
            .execute(&self.pool)
            .await
            .map_err(DbError::consume_refresh_token)?;
        Ok(result.rows_affected() > 0)
    }

    async fn delete_refresh_tokens_for_user(&self, user_id: &str) -> Result<(), ServirError> {
        sqlx::query("DELETE FROM refresh_tokens WHERE user_id = ?")
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(|e| DbError::delete_refresh_tokens_for_user(user_id, e))?;
        Ok(())
    }

    async fn load_or_generate_secret(&self) -> Result<String, ServirError> {
        let existing: Option<String> =
            sqlx::query_scalar("SELECT value FROM auth_config WHERE key = 'signing_secret'")
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| DbError::unknown("load_signing_secret", e))?;

        if let Some(secret) = existing {
            return Ok(secret);
        }

        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("getrandom fill");
        let secret = hex::encode(bytes);

        sqlx::query("INSERT INTO auth_config (key, value) VALUES ('signing_secret', ?)")
            .bind(&secret)
            .execute(&self.pool)
            .await
            .map_err(|e| DbError::unknown("store_signing_secret", e))?;

        info!("generated new signing secret");
        Ok(secret)
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
        db.migrate_auth().await.unwrap();
        db
    }

    #[tokio::test]
    async fn create_and_find_user() {
        let db = test_db().await;
        let user = db.create_user("alice", "hash123").await.unwrap();

        assert_eq!(user.username(), "alice");
        assert!(!user.id().is_empty());

        let found = db.find_user_by_username("alice").await.unwrap();
        assert_eq!(found.unwrap().id(), user.id());
    }

    #[tokio::test]
    async fn duplicate_user_returns_already_exists() {
        let db = test_db().await;
        db.create_user("bob", "hash1").await.unwrap();

        let result = db.create_user("bob", "hash2").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn find_user_by_id_works() {
        let db = test_db().await;
        let user = db.create_user("carol", "hash").await.unwrap();

        let found = db.find_user_by_id(user.id()).await.unwrap();
        assert_eq!(found.unwrap().username(), "carol");

        let missing = db.find_user_by_id("nonexistent").await.unwrap();
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn load_or_generate_secret_persists() {
        let db = test_db().await;

        let first = db.load_or_generate_secret().await.unwrap();
        assert!(!first.is_empty());

        let second = db.load_or_generate_secret().await.unwrap();
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn refresh_token_lifecycle() {
        let db = test_db().await;
        let user = db.create_user("dave", "hash").await.unwrap();

        db.store_refresh_token("jti-1", user.id(), user.username(), 3600)
            .await
            .unwrap();

        // Consume succeeds first time.
        assert!(db.consume_refresh_token("jti-1").await.unwrap());
        // Second consume fails (already consumed).
        assert!(!db.consume_refresh_token("jti-1").await.unwrap());
    }

    #[tokio::test]
    async fn delete_refresh_tokens_for_user_works() {
        let db = test_db().await;
        let user = db.create_user("eve", "hash").await.unwrap();

        db.store_refresh_token("jti-a", user.id(), user.username(), 3600)
            .await
            .unwrap();
        db.store_refresh_token("jti-b", user.id(), user.username(), 3600)
            .await
            .unwrap();

        db.delete_refresh_tokens_for_user(user.id()).await.unwrap();

        assert!(!db.consume_refresh_token("jti-a").await.unwrap());
        assert!(!db.consume_refresh_token("jti-b").await.unwrap());
    }
}
