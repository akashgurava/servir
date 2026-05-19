use std::time::{SystemTime, UNIX_EPOCH};

use argon2::password_hash::rand_core::OsRng;
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tracing::info;
use uuid::Uuid;

use crate::error::{AuthTokenError, AuthUserError, DbError, ServirError};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before Unix epoch")
        .as_secs()
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// An authenticated user loaded from the database.
#[derive(Debug, Clone)]
pub(super) struct User {
    id: String,
    username: String,
    password_hash: String,
}

impl User {
    pub(super) fn id(&self) -> &str {
        &self.id
    }

    pub(super) fn username(&self) -> &str {
        &self.username
    }

    pub(super) fn password_hash(&self) -> &str {
        &self.password_hash
    }
}

/// Distinguishes access tokens from refresh tokens.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(super) enum TokenKind {
    Access,
    Refresh,
}

/// JWT payload carried by both access and refresh tokens.
///
/// Exposed publicly so consumers can read claims from the [`AuthUser`](super::http::AuthUser) extractor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    sub: String,
    username: String,
    exp: usize,
    iat: usize,
    jti: String,
    kind: TokenKind,
}

impl Claims {
    /// The authenticated user's ID (JWT `sub` claim).
    pub fn sub(&self) -> &str {
        &self.sub
    }

    /// The authenticated user's username.
    pub fn username(&self) -> &str {
        &self.username
    }

    /// The token's unique identifier (JWT `jti` claim).
    pub(super) fn jti(&self) -> &str {
        &self.jti
    }

    /// The token kind (access or refresh).
    pub(super) fn kind(&self) -> &TokenKind {
        &self.kind
    }
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Auth configuration holding the signing secret and token TTLs.
///
/// Construct via [`AuthConfig::new`] (tests) or [`AuthConfig::from_env`] (production).
/// The signing secret is auto-generated and stored in the database on first run.
#[derive(Debug, Clone)]
pub(crate) struct AuthConfig {
    pub(super) secret: String,
    pub(super) access_token_ttl_secs: u64,
    pub(super) refresh_token_ttl_secs: u64,
}

impl AuthConfig {
    /// Constructs a config with an explicit signing secret and sensible defaults.
    ///
    /// Defaults: access TTL 900s (15 min), refresh TTL 604800s (7 days).
    #[cfg(test)]
    pub(crate) fn new(secret: impl Into<String>) -> Self {
        Self {
            secret: secret.into(),
            access_token_ttl_secs: 900,
            refresh_token_ttl_secs: 604_800,
        }
    }

    /// Reads configuration from environment variables.
    ///
    /// Optional: `AUTH_ACCESS_TOKEN_TTL_SECS` (default: 900),
    ///           `AUTH_REFRESH_TOKEN_TTL_SECS` (default: 604800).
    ///
    /// The signing secret is loaded from the database after [`Self::migrate`].
    pub(crate) fn from_env() -> Self {
        let access_token_ttl_secs = std::env::var("AUTH_ACCESS_TOKEN_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(900);

        let refresh_token_ttl_secs = std::env::var("AUTH_REFRESH_TOKEN_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(604_800);

        Self {
            secret: String::new(), // loaded after migrate via load_or_generate_secret
            access_token_ttl_secs,
            refresh_token_ttl_secs,
        }
    }

    /// Runs embedded migrations and logs the current schema version.
    ///
    /// Returns an error if a previously-applied migration has been modified (checksum mismatch).
    pub async fn migrate(&self, pool: &SqlitePool) -> Result<(), ServirError> {
        sqlx::migrate!("src/auth/migrations")
            .run(pool)
            .await
            .map_err(DbError::migration)?;

        let version: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success = 1",
        )
        .fetch_one(pool)
        .await
        .unwrap_or(0);

        info!(version, "auth schema up to date");
        Ok(())
    }

    /// Loads the signing secret from the database, generating one if it doesn't exist.
    ///
    /// Must be called after [`Self::migrate`]. Generates a 256-bit random key on first run
    /// and persists it in the `auth_config` table.
    pub async fn load_or_generate_secret(&mut self, pool: &SqlitePool) -> Result<(), ServirError> {
        let existing: Option<String> =
            sqlx::query_scalar("SELECT value FROM auth_config WHERE key = 'signing_secret'")
                .fetch_optional(pool)
                .await
                .map_err(|e| DbError::unknown("load_signing_secret", e))?;

        if let Some(secret) = existing {
            self.secret = secret;
        } else {
            let mut bytes = [0u8; 32];
            getrandom::fill(&mut bytes).expect("getrandom fill");
            let secret = hex::encode(bytes);

            sqlx::query("INSERT INTO auth_config (key, value) VALUES ('signing_secret', ?)")
                .bind(&secret)
                .execute(pool)
                .await
                .map_err(|e| DbError::unknown("store_signing_secret", e))?;

            info!("generated new signing secret");
            self.secret = secret;
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Password
// ---------------------------------------------------------------------------

/// Hashes `plaintext` with Argon2id and a random salt.
///
/// Returns the PHC-formatted hash string.
pub(super) fn hash_password(plaintext: &str) -> Result<String, ServirError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(plaintext.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(AuthUserError::password_hash_failed)
}

/// Validates a plaintext password against an Argon2id hash.
///
/// Returns `true` if the password matches, `false` otherwise.
/// Returns [`ServirError`] if the hash string is malformed.
pub(super) fn verify_password(plaintext: &str, hash: &str) -> Result<bool, ServirError> {
    let parsed = PasswordHash::new(hash).map_err(AuthUserError::password_hash_failed)?;
    Ok(Argon2::default()
        .verify_password(plaintext.as_bytes(), &parsed)
        .is_ok())
}

// ---------------------------------------------------------------------------
// Token
// ---------------------------------------------------------------------------

/// Issues a short-lived access token for the given user.
///
/// The token contains the user's ID (`sub`), username, and `kind: access`.
pub(super) fn issue_access_token(
    user_id: &str,
    username: &str,
    config: &AuthConfig,
) -> Result<String, ServirError> {
    let now = now_secs() as usize;
    let claims = Claims {
        sub: user_id.to_string(),
        username: username.to_string(),
        exp: now + config.access_token_ttl_secs as usize,
        iat: now,
        jti: Uuid::new_v4().to_string(),
        kind: TokenKind::Access,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(config.secret.as_bytes()),
    )
    .map_err(AuthTokenError::encode_failed)
}

/// Issues a long-lived refresh token. Returns `(token_string, jti)`.
///
/// The caller is responsible for persisting `jti` in the database.
pub(super) fn issue_refresh_token(
    user_id: &str,
    username: &str,
    config: &AuthConfig,
) -> Result<(String, String), ServirError> {
    let now = now_secs() as usize;
    let jti = Uuid::new_v4().to_string();
    let claims = Claims {
        sub: user_id.to_string(),
        username: username.to_string(),
        exp: now + config.refresh_token_ttl_secs as usize,
        iat: now,
        jti: jti.clone(),
        kind: TokenKind::Refresh,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(config.secret.as_bytes()),
    )
    .map_err(AuthTokenError::encode_failed)?;
    Ok((token, jti))
}

/// Verifies the token signature and expiry. Does **not** check `kind`.
///
/// Maps `ExpiredSignature` to [`AuthTokenError::expired`], all other
/// verification failures to [`AuthTokenError::invalid`].
pub(super) fn verify_token(token: &str, config: &AuthConfig) -> Result<Claims, ServirError> {
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(config.secret.as_bytes()),
        &Validation::default(),
    )
    .map(|d| d.claims)
    .map_err(|e| match e.kind() {
        jsonwebtoken::errors::ErrorKind::ExpiredSignature => AuthTokenError::expired(),
        _ => AuthTokenError::invalid(e),
    })
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct UserRow {
    id: String,
    username: String,
    password_hash: String,
}

impl UserRow {
    fn into_user(self) -> User {
        User {
            id: self.id,
            username: self.username,
            password_hash: self.password_hash,
        }
    }
}

/// Inserts a new user. Returns [`AuthUserError::already_exists`] on duplicate username.
pub(super) async fn create_user(
    pool: &SqlitePool,
    username: &str,
    password_hash: &str,
) -> Result<User, ServirError> {
    let id = Uuid::new_v4().to_string();

    sqlx::query("INSERT INTO users (id, username, password_hash, created_at) VALUES (?, ?, ?, ?)")
        .bind(&id)
        .bind(username)
        .bind(password_hash)
        .bind(now_secs() as i64)
        .execute(pool)
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

/// Finds a user by username. Returns `None` if not found.
pub(super) async fn find_user_by_username(
    pool: &SqlitePool,
    username: &str,
) -> Result<Option<User>, ServirError> {
    sqlx::query_as::<_, UserRow>("SELECT id, username, password_hash FROM users WHERE username = ?")
        .bind(username)
        .fetch_optional(pool)
        .await
        .map(|r| r.map(UserRow::into_user))
        .map_err(|e| DbError::user_find_by_username(username, e))
}

/// Finds a user by ID. Returns `None` if not found.
pub(super) async fn find_user_by_id(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Option<User>, ServirError> {
    sqlx::query_as::<_, UserRow>("SELECT id, username, password_hash FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map(|r| r.map(UserRow::into_user))
        .map_err(|e| DbError::user_find_by_id(user_id, e))
}

/// Persists a refresh token's `jti` so it can be consumed exactly once.
pub(super) async fn store_refresh_token(
    pool: &SqlitePool,
    jti: &str,
    user: &User,
    ttl_secs: u64,
) -> Result<(), ServirError> {
    let expires_at = now_secs() as i64 + ttl_secs as i64;
    sqlx::query(
        "INSERT INTO refresh_tokens (jti, user_id, expires_at, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(jti)
    .bind(&user.id)
    .bind(expires_at)
    .bind(now_secs() as i64)
    .execute(pool)
    .await
    .map_err(|e| DbError::store_refresh_token(&user.username, e))?;
    Ok(())
}

/// Atomically consumes a refresh token. Returns `true` if it existed and was not expired.
pub(super) async fn consume_refresh_token(
    pool: &SqlitePool,
    jti: &str,
) -> Result<bool, ServirError> {
    let result = sqlx::query("DELETE FROM refresh_tokens WHERE jti = ? AND expires_at > ?")
        .bind(jti)
        .bind(now_secs() as i64)
        .execute(pool)
        .await
        .map_err(DbError::consume_refresh_token)?;
    Ok(result.rows_affected() > 0)
}

/// Deletes all refresh tokens for a user — used on logout.
pub(super) async fn delete_refresh_tokens_for_user(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<(), ServirError> {
    sqlx::query("DELETE FROM refresh_tokens WHERE user_id = ?")
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(|e| DbError::delete_refresh_tokens_for_user(user_id, e))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Password ---

    #[test]
    fn hash_and_verify_roundtrip() {
        let hash = hash_password("secure-password-123").unwrap();
        assert!(verify_password("secure-password-123", &hash).unwrap());
    }

    #[test]
    fn verify_wrong_password() {
        let hash = hash_password("correct-password").unwrap();
        assert!(!verify_password("wrong-password", &hash).unwrap());
    }

    #[test]
    fn verify_corrupted_hash() {
        let result = verify_password("anything", "not-a-valid-hash");
        assert!(result.is_err());
    }

    // --- Token ---

    fn test_config() -> AuthConfig {
        AuthConfig::new("test-secret-key-must-be-at-least-32-characters!!")
    }

    #[test]
    fn issue_and_verify_access_token() {
        let config = test_config();
        let token = issue_access_token("user-123", "alice", &config).unwrap();
        let claims = verify_token(&token, &config).unwrap();

        assert_eq!(claims.sub(), "user-123");
        assert_eq!(claims.username(), "alice");
        assert_eq!(claims.kind, TokenKind::Access);
    }

    #[test]
    fn issue_and_verify_refresh_token() {
        let config = test_config();
        let (token, jti) = issue_refresh_token("user-123", "alice", &config).unwrap();
        let claims = verify_token(&token, &config).unwrap();

        assert_eq!(claims.sub(), "user-123");
        assert_eq!(claims.kind, TokenKind::Refresh);
        assert_eq!(claims.jti, jti);
    }

    #[test]
    fn verify_with_wrong_secret() {
        let config = test_config();
        let token = issue_access_token("user-123", "alice", &config).unwrap();

        let other = AuthConfig::new("completely-different-secret-that-is-long-enough");
        let result = verify_token(&token, &other);
        assert!(result.is_err());
    }

    #[test]
    fn expired_token_rejected() {
        let config = test_config();

        // Manually encode a token with exp in the past.
        let claims = Claims {
            sub: "user-123".to_string(),
            username: "alice".to_string(),
            exp: 1_000_000, // far in the past
            iat: 1_000_000,
            jti: "test-jti".to_string(),
            kind: TokenKind::Access,
        };
        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(config.secret.as_bytes()),
        )
        .unwrap();

        let result = verify_token(&token, &config);
        assert!(result.is_err());
    }

    // --- Store ---

    async fn test_pool() -> SqlitePool {
        use sqlx::Sqlite;
        use sqlx::pool::PoolOptions;
        use sqlx::sqlite::SqliteConnectOptions;

        let pool = PoolOptions::<Sqlite>::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(":memory:")
                    .create_if_missing(true),
            )
            .await
            .unwrap();

        sqlx::migrate!("src/auth/migrations")
            .run(&pool)
            .await
            .unwrap();

        pool
    }

    #[tokio::test]
    async fn create_and_find_user() {
        let pool = test_pool().await;
        let user = create_user(&pool, "alice", "hash123").await.unwrap();

        assert_eq!(user.username, "alice");
        assert!(!user.id.is_empty());

        let found = find_user_by_username(&pool, "alice").await.unwrap();
        assert_eq!(found.unwrap().id, user.id);
    }

    #[tokio::test]
    async fn duplicate_user_returns_already_exists() {
        let pool = test_pool().await;
        create_user(&pool, "bob", "hash1").await.unwrap();

        let result = create_user(&pool, "bob", "hash2").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn load_or_generate_secret_persists() {
        let pool = test_pool().await;
        let mut config = AuthConfig::new("");

        // First call generates and stores.
        config.load_or_generate_secret(&pool).await.unwrap();
        let first_secret = config.secret.clone();
        assert!(!first_secret.is_empty());

        // Second call loads existing.
        let mut config2 = AuthConfig::new("");
        config2.load_or_generate_secret(&pool).await.unwrap();
        assert_eq!(config2.secret, first_secret);
    }

    #[tokio::test]
    async fn find_user_by_id_works() {
        let pool = test_pool().await;
        let user = create_user(&pool, "carol", "hash").await.unwrap();

        let found = find_user_by_id(&pool, &user.id).await.unwrap();
        assert_eq!(found.unwrap().username, "carol");

        let missing = find_user_by_id(&pool, "nonexistent").await.unwrap();
        assert!(missing.is_none());
    }
}
