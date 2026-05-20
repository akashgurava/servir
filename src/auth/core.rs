use std::time::{SystemTime, UNIX_EPOCH};

use argon2::password_hash::rand_core::OsRng;
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AuthTokenError, AuthUserError, ServirError};

// ---------------------------------Helpers----------------------------------

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before Unix epoch")
        .as_secs()
}

// ---------------------------------Types----------------------------------

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

// ---------------------------------Config----------------------------------

/// Auth configuration holding the signing secret and token TTLs.
///
/// Construct via [`AuthConfig::new`] (tests) or [`AuthConfig::with_defaults`] (production).
/// The signing secret is loaded from the database via [`AuthRepository::load_or_generate_secret`].
#[derive(Debug, Clone)]
pub(crate) struct AuthConfig {
    secret: String,
    access_token_ttl_secs: u64,
    refresh_token_ttl_secs: u64,
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

    /// Constructs a config with sensible defaults.
    ///
    /// Defaults: access TTL 900s (15 min), refresh TTL 604800s (7 days).
    /// The signing secret must be loaded separately via the database.
    pub(crate) fn with_defaults() -> Self {
        Self {
            secret: String::new(),
            access_token_ttl_secs: 900,
            refresh_token_ttl_secs: 604_800,
        }
    }

    pub(crate) fn set_access_token_ttl(&mut self, secs: u64) {
        self.access_token_ttl_secs = secs;
    }

    pub(crate) fn set_refresh_token_ttl(&mut self, secs: u64) {
        self.refresh_token_ttl_secs = secs;
    }

    pub(crate) fn refresh_token_ttl_secs(&self) -> u64 {
        self.refresh_token_ttl_secs
    }

    pub(crate) fn set_secret(&mut self, secret: String) {
        self.secret = secret;
    }
}

// ---------------------------------Password----------------------------------

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

// ---------------------------------Token----------------------------------

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

// ---------------------------------Unit Tests----------------------------------

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
}
