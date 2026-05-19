use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::response::{ApiResponse, ErrorBody};

/// An identifier for an error.
///
/// Provides structured context about which resource an error relates to.
/// For example, for a user not found error:
/// `Identifier { kind: "username", value: "john_doe" }`
#[derive(Debug)]
pub(crate) struct Identifier {
    kind: &'static str,
    value: String,
}

impl Identifier {
    /// Creates a new identifier with the given kind and value.
    fn new(kind: &'static str, value: impl ToString) -> Self {
        Self {
            kind,
            value: value.to_string(),
        }
    }

    /// Returns the kind of the identifier (e.g. `"username"`, `"user_id"`, `"url"`).
    fn kind(&self) -> &'static str {
        self.kind
    }

    /// Returns the value of the identifier (e.g. `"john_doe"`, `"abc-123"`).
    fn value(&self) -> &str {
        &self.value
    }
}

/// Structured error context serialized in API error responses.
///
/// Contains an optional error description and an optional identifier.
/// Serializes as a flat JSON map:
/// ```json
/// {"error": "connection refused", "url": "sqlite://bad.db"}
/// {"error": null, "username": "alice"}
/// {"error": null}
/// ```
#[derive(Debug)]
pub(crate) struct ErrorContext {
    error: Option<String>,
    identifier: Option<Identifier>,
}

impl ErrorContext {
    /// Creates a context with both an error description and an identifier.
    ///
    /// ```ignore
    /// ErrorContext::new(Some("unique constraint violated"), "username", "alice")
    /// // Serializes as: {"error": "unique constraint violated", "username": "alice"}
    /// ```
    pub(crate) fn new(
        error: Option<impl ToString>,
        kind: &'static str,
        value: impl ToString,
    ) -> Self {
        Self {
            error: error.map(|e| e.to_string()),
            identifier: Some(Identifier::new(kind, value)),
        }
    }

    /// Creates a context with only an error description, no identifier.
    ///
    /// ```ignore
    /// ErrorContext::error_only("migration checksum mismatch")
    /// // Serializes as: {"error": "migration checksum mismatch"}
    /// ```
    pub(crate) fn error_only(error: impl ToString) -> Self {
        Self {
            error: Some(error.to_string()),
            identifier: None,
        }
    }

    /// Creates an empty context (no error, no identifier).
    ///
    /// ```ignore
    /// ErrorContext::empty()
    /// // Serializes as: {"error": null}
    /// ```
    pub(crate) fn empty() -> Self {
        Self {
            error: None,
            identifier: None,
        }
    }
}

impl serde::Serialize for ErrorContext {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let len = 1 + usize::from(self.identifier.is_some());
        let mut map = serializer.serialize_map(Some(len))?;
        map.serialize_entry("error", &self.error)?;
        if let Some(id) = &self.identifier {
            map.serialize_entry(id.kind(), id.value())?;
        }
        map.end()
    }
}

/// Internal trait for converting error types into HTTP response components.
///
/// Each error type provides:
/// - A status code (e.g. 401, 404, 500)
/// - A machine-readable error ID (e.g. `"INVALID_CREDENTIALS"`)
/// - Structured context for the response body
pub(crate) trait ErrorInfo {
    /// Returns the HTTP status code for this error.
    fn status_code(&self) -> StatusCode;

    /// Returns a machine-readable error identifier (e.g. `"TOKEN_EXPIRED"`).
    fn error_id(&self) -> &'static str;

    /// Returns the structured context for this error.
    fn context(&self) -> ErrorContext;
}

/// Generates `Display` and `Error` implementations from [`ErrorInfo`].
///
/// The `Display` output format is: `ERROR_ID` or `ERROR_ID. kind: value` when
/// an identifier is present, for structured log output.
macro_rules! impl_err_from_info {
    ($error:ty) => {
        impl std::fmt::Display for $error {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let ctx = self.context();
                f.write_str(self.error_id())?;
                if let Some(id) = ctx.identifier.as_ref() {
                    write!(f, ". {}: {}", id.kind(), id.value())?;
                }
                if let Some(error) = ctx.error.as_ref() {
                    write!(f, " ({error})")?;
                }
                Ok(())
            }
        }

        impl std::error::Error for $error {}
    };
}

/// Database-level failure.
///
/// All variants map to [`StatusCode::INTERNAL_SERVER_ERROR`].
/// Each variant wraps the underlying database error message for debugging.
#[derive(Debug)]
pub enum DbError {
    Connection {
        url: String,
        error: String,
    },
    Migration {
        error: String,
    },
    UserCreate {
        username: String,
        error: String,
    },
    UserFindByUsername {
        username: String,
        error: String,
    },
    UserFindById {
        user_id: String,
        error: String,
    },
    StoreRefreshToken {
        username: String,
        error: String,
    },
    DeleteRefreshTokensForUser {
        username: String,
        error: String,
    },
    ConsumeRefreshToken {
        error: String,
    },
    Unknown {
        operation: &'static str,
        error: String,
    },
}

impl DbError {
    /// Failed to connect to the database.
    ///
    /// Status Code: [`StatusCode::INTERNAL_SERVER_ERROR`].
    /// Error ID: `DB_CONNECTION_FAILED`.
    /// Context: `url`, `error`.
    pub fn connection(url: impl Into<String>, error: impl std::fmt::Display) -> ServirError {
        ServirError::Db(Self::Connection {
            url: url.into(),
            error: error.to_string(),
        })
    }

    /// Schema migration failed.
    ///
    /// Status Code: [`StatusCode::INTERNAL_SERVER_ERROR`].
    /// Error ID: `DB_MIGRATION_FAILED`.
    /// Context: `error`.
    pub fn migration(error: impl std::fmt::Display) -> ServirError {
        ServirError::Db(Self::Migration {
            error: error.to_string(),
        })
    }

    /// Failed to insert a new user (e.g. unique constraint).
    ///
    /// Status Code: [`StatusCode::INTERNAL_SERVER_ERROR`].
    /// Error ID: `DB_USER_CREATE_FAILED`.
    /// Context: `username`, `error`.
    pub fn user_create(username: impl Into<String>, error: impl std::fmt::Display) -> ServirError {
        ServirError::Db(Self::UserCreate {
            username: username.into(),
            error: error.to_string(),
        })
    }

    /// Failed to look up a user by username.
    ///
    /// Status Code: [`StatusCode::INTERNAL_SERVER_ERROR`].
    /// Error ID: `DB_USER_FIND_BY_USERNAME_FAILED`.
    /// Context: `username`, `error`.
    pub fn user_find_by_username(
        username: impl Into<String>,
        error: impl std::fmt::Display,
    ) -> ServirError {
        ServirError::Db(Self::UserFindByUsername {
            username: username.into(),
            error: error.to_string(),
        })
    }

    /// Failed to look up a user by ID.
    ///
    /// Status Code: [`StatusCode::INTERNAL_SERVER_ERROR`].
    /// Error ID: `DB_USER_FIND_BY_ID_FAILED`.
    /// Context: `user_id`, `error`.
    pub fn user_find_by_id(
        user_id: impl Into<String>,
        error: impl std::fmt::Display,
    ) -> ServirError {
        ServirError::Db(Self::UserFindById {
            user_id: user_id.into(),
            error: error.to_string(),
        })
    }

    /// Failed to persist a refresh token.
    ///
    /// Status Code: [`StatusCode::INTERNAL_SERVER_ERROR`].
    /// Error ID: `DB_STORE_REFRESH_TOKEN_FAILED`.
    /// Context: `username`, `error`.
    pub fn store_refresh_token(
        username: impl Into<String>,
        error: impl std::fmt::Display,
    ) -> ServirError {
        ServirError::Db(Self::StoreRefreshToken {
            username: username.into(),
            error: error.to_string(),
        })
    }

    /// Failed to delete refresh tokens for a user.
    ///
    /// Status Code: [`StatusCode::INTERNAL_SERVER_ERROR`].
    /// Error ID: `DB_DELETE_REFRESH_TOKENS_FOR_USER_FAILED`.
    /// Context: `username`, `error`.
    pub fn delete_refresh_tokens_for_user(
        username: impl Into<String>,
        error: impl std::fmt::Display,
    ) -> ServirError {
        ServirError::Db(Self::DeleteRefreshTokensForUser {
            username: username.into(),
            error: error.to_string(),
        })
    }

    /// Failed to consume (mark as used) a refresh token.
    ///
    /// Status Code: [`StatusCode::INTERNAL_SERVER_ERROR`].
    /// Error ID: `DB_CONSUME_REFRESH_TOKEN_FAILED`.
    /// Context: `error`.
    pub fn consume_refresh_token(error: impl std::fmt::Display) -> ServirError {
        ServirError::Db(Self::ConsumeRefreshToken {
            error: error.to_string(),
        })
    }

    /// Catch-all for unclassified database operations.
    ///
    /// Status Code: [`StatusCode::INTERNAL_SERVER_ERROR`].
    /// Error ID: `DB_UNKNOWN`.
    /// Context: `operation`, `error`.
    pub fn unknown(operation: &'static str, error: impl std::fmt::Display) -> ServirError {
        ServirError::Db(Self::Unknown {
            operation,
            error: error.to_string(),
        })
    }
}

impl ErrorInfo for DbError {
    fn status_code(&self) -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }

    fn error_id(&self) -> &'static str {
        match self {
            Self::Connection { .. } => "DB_CONNECTION_FAILED",
            Self::Migration { .. } => "DB_MIGRATION_FAILED",
            Self::UserCreate { .. } => "DB_USER_CREATE_FAILED",
            Self::UserFindByUsername { .. } => "DB_USER_FIND_BY_USERNAME_FAILED",
            Self::UserFindById { .. } => "DB_USER_FIND_BY_ID_FAILED",
            Self::StoreRefreshToken { .. } => "DB_STORE_REFRESH_TOKEN_FAILED",
            Self::DeleteRefreshTokensForUser { .. } => "DB_DELETE_REFRESH_TOKENS_FOR_USER_FAILED",
            Self::ConsumeRefreshToken { .. } => "DB_CONSUME_REFRESH_TOKEN_FAILED",
            Self::Unknown { .. } => "DB_UNKNOWN",
        }
    }

    fn context(&self) -> ErrorContext {
        match self {
            Self::Connection { url, error } => ErrorContext::new(Some(error), "url", url),
            Self::Migration { error } => ErrorContext::error_only(error),
            Self::UserCreate { username, error } => {
                ErrorContext::new(Some(error), "username", username)
            }
            Self::UserFindByUsername { username, error } => {
                ErrorContext::new(Some(error), "username", username)
            }
            Self::UserFindById { user_id, error } => {
                ErrorContext::new(Some(error), "user_id", user_id)
            }
            Self::StoreRefreshToken { username, error } => {
                ErrorContext::new(Some(error), "username", username)
            }
            Self::DeleteRefreshTokensForUser { username, error } => {
                ErrorContext::new(Some(error), "username", username)
            }
            Self::ConsumeRefreshToken { error } => ErrorContext::error_only(error),
            Self::Unknown { operation, error } => {
                ErrorContext::new(Some(error), "operation", operation)
            }
        }
    }
}

impl_err_from_info!(DbError);

/// User credential management failure.
///
/// Covers registration, login, and password-related errors.
#[derive(Debug)]
pub enum AuthUserError {
    EmptyUsername,
    WeakPassword,
    AlreadyExists { username: String, error: String },
    InvalidCredentials { username: String },
    PasswordHashFailed { error: String },
}

impl AuthUserError {
    /// Registration attempted with an empty username string.
    ///
    /// Status Code: [`StatusCode::BAD_REQUEST`].
    /// Error ID: `EMPTY_USERNAME`.
    /// Context: None.
    pub fn empty_username() -> ServirError {
        ServirError::AuthUser(Self::EmptyUsername)
    }

    /// Registration attempted with a password shorter than 8 characters.
    ///
    /// Status Code: [`StatusCode::BAD_REQUEST`].
    /// Error ID: `WEAK_PASSWORD`.
    /// Context: None.
    pub fn weak_password() -> ServirError {
        ServirError::AuthUser(Self::WeakPassword)
    }

    /// Username already taken.
    ///
    /// Status Code: [`StatusCode::CONFLICT`].
    /// Error ID: `ALREADY_EXISTS`.
    /// Context: `username`, `error`.
    pub fn already_exists(
        username: impl Into<String>,
        error: impl std::fmt::Display,
    ) -> ServirError {
        ServirError::AuthUser(Self::AlreadyExists {
            username: username.into(),
            error: error.to_string(),
        })
    }

    /// Login failed — wrong password or user does not exist.
    ///
    /// Status Code: [`StatusCode::UNAUTHORIZED`].
    /// Error ID: `INVALID_CREDENTIALS`.
    /// Context: `username`.
    pub fn invalid_credentials(username: impl Into<String>) -> ServirError {
        ServirError::AuthUser(Self::InvalidCredentials {
            username: username.into(),
        })
    }

    /// Argon2 password hashing failed internally.
    ///
    /// Status Code: [`StatusCode::INTERNAL_SERVER_ERROR`].
    /// Error ID: `PASSWORD_HASH_FAILED`.
    /// Context: `error`.
    pub fn password_hash_failed(error: impl std::fmt::Display) -> ServirError {
        ServirError::AuthUser(Self::PasswordHashFailed {
            error: error.to_string(),
        })
    }
}

impl ErrorInfo for AuthUserError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::EmptyUsername | Self::WeakPassword => StatusCode::BAD_REQUEST,
            Self::AlreadyExists { .. } => StatusCode::CONFLICT,
            Self::InvalidCredentials { .. } => StatusCode::UNAUTHORIZED,
            Self::PasswordHashFailed { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_id(&self) -> &'static str {
        match self {
            Self::EmptyUsername => "EMPTY_USERNAME",
            Self::WeakPassword => "WEAK_PASSWORD",
            Self::AlreadyExists { .. } => "ALREADY_EXISTS",
            Self::InvalidCredentials { .. } => "INVALID_CREDENTIALS",
            Self::PasswordHashFailed { .. } => "PASSWORD_HASH_FAILED",
        }
    }

    fn context(&self) -> ErrorContext {
        match self {
            Self::AlreadyExists { username, error } => {
                ErrorContext::new(Some(error), "username", username)
            }
            Self::InvalidCredentials { username } => {
                ErrorContext::new(None::<&str>, "username", username)
            }
            Self::PasswordHashFailed { error } => ErrorContext::error_only(error),
            _ => ErrorContext::empty(),
        }
    }
}

impl_err_from_info!(AuthUserError);

/// Token lifecycle failure.
///
/// Covers JWT verification, refresh token rotation, and token encoding errors.
#[derive(Debug)]
pub enum AuthTokenError {
    Expired,
    Invalid { error: String },
    MissingHeader,
    WrongKind,
    InvalidRefreshToken,
    UserNotFound { user_id: String },
    MissingLayer,
    EncodeFailed { error: String },
}

impl AuthTokenError {
    /// Token signature is valid but the `exp` claim is in the past.
    ///
    /// Status Code: [`StatusCode::UNAUTHORIZED`].
    /// Error ID: `TOKEN_EXPIRED`.
    /// Context: None.
    pub fn expired() -> ServirError {
        ServirError::AuthToken(Self::Expired)
    }

    /// Token could not be decoded or signature verification failed.
    ///
    /// Status Code: [`StatusCode::UNAUTHORIZED`].
    /// Error ID: `TOKEN_INVALID`.
    /// Context: `error`.
    pub fn invalid(error: impl std::fmt::Display) -> ServirError {
        ServirError::AuthToken(Self::Invalid {
            error: error.to_string(),
        })
    }

    /// No `Authorization: Bearer <token>` header present.
    ///
    /// Status Code: [`StatusCode::UNAUTHORIZED`].
    /// Error ID: `MISSING_AUTHORIZATION_HEADER`.
    /// Context: None.
    pub fn missing_header() -> ServirError {
        ServirError::AuthToken(Self::MissingHeader)
    }

    /// An access token was used where a refresh token is required (or vice versa).
    ///
    /// Status Code: [`StatusCode::UNAUTHORIZED`].
    /// Error ID: `WRONG_TOKEN_KIND`.
    /// Context: None.
    pub fn wrong_kind() -> ServirError {
        ServirError::AuthToken(Self::WrongKind)
    }

    /// Refresh token's `jti` is not in the database (already consumed or revoked).
    ///
    /// Status Code: [`StatusCode::UNAUTHORIZED`].
    /// Error ID: `INVALID_REFRESH_TOKEN`.
    /// Context: None.
    pub fn invalid_refresh_token() -> ServirError {
        ServirError::AuthToken(Self::InvalidRefreshToken)
    }

    /// The `sub` claim references a user ID that no longer exists.
    ///
    /// Status Code: [`StatusCode::UNAUTHORIZED`].
    /// Error ID: `USER_NOT_FOUND`.
    /// Context: `user_id`.
    pub fn user_not_found(user_id: impl Into<String>) -> ServirError {
        ServirError::AuthToken(Self::UserNotFound {
            user_id: user_id.into(),
        })
    }

    /// `AuthLayer` was not applied to the router — programming error.
    ///
    /// Status Code: [`StatusCode::INTERNAL_SERVER_ERROR`].
    /// Error ID: `AUTH_LAYER_MISSING`.
    /// Context: None.
    pub fn missing_layer() -> ServirError {
        ServirError::AuthToken(Self::MissingLayer)
    }

    /// JWT encoding failed internally.
    ///
    /// Status Code: [`StatusCode::INTERNAL_SERVER_ERROR`].
    /// Error ID: `TOKEN_ENCODE_FAILED`.
    /// Context: `error`.
    pub fn encode_failed(error: impl std::fmt::Display) -> ServirError {
        ServirError::AuthToken(Self::EncodeFailed {
            error: error.to_string(),
        })
    }
}

impl ErrorInfo for AuthTokenError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::MissingLayer | Self::EncodeFailed { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::UNAUTHORIZED,
        }
    }

    fn error_id(&self) -> &'static str {
        match self {
            Self::Expired => "TOKEN_EXPIRED",
            Self::Invalid { .. } => "TOKEN_INVALID",
            Self::MissingHeader => "MISSING_AUTHORIZATION_HEADER",
            Self::WrongKind => "WRONG_TOKEN_KIND",
            Self::InvalidRefreshToken => "INVALID_REFRESH_TOKEN",
            Self::UserNotFound { .. } => "USER_NOT_FOUND",
            Self::MissingLayer => "AUTH_LAYER_MISSING",
            Self::EncodeFailed { .. } => "TOKEN_ENCODE_FAILED",
        }
    }

    fn context(&self) -> ErrorContext {
        match self {
            Self::Invalid { error } => ErrorContext::error_only(error),
            Self::UserNotFound { user_id } => ErrorContext::new(None::<&str>, "user_id", user_id),
            Self::EncodeFailed { error } => ErrorContext::error_only(error),
            _ => ErrorContext::empty(),
        }
    }
}

impl_err_from_info!(AuthTokenError);

// --- From impls for ergonomic `?` propagation ---

impl From<DbError> for ServirError {
    fn from(e: DbError) -> Self {
        Self::Db(e)
    }
}

impl From<AuthUserError> for ServirError {
    fn from(e: AuthUserError) -> Self {
        Self::AuthUser(e)
    }
}

impl From<AuthTokenError> for ServirError {
    fn from(e: AuthTokenError) -> Self {
        Self::AuthToken(e)
    }
}

/// Top-level error type for all servir operations.
///
/// Implements [`IntoResponse`] to produce a consistent JSON error envelope:
/// ```json
/// {
///   "status": "error",
///   "error": {
///     "error_id": "INVALID_CREDENTIALS",
///     "context": {"error": null, "username": "alice"}
///   }
/// }
/// ```
///
/// Use the `?` operator in handlers returning `Result<ApiResponse<T>, ServirError>`.
#[derive(Debug)]
pub enum ServirError {
    Db(DbError),
    AuthUser(AuthUserError),
    AuthToken(AuthTokenError),
    BadRequest { message: String },
    NotFound,
    Io(std::io::Error),
}

impl ServirError {
    /// Request body could not be parsed.
    ///
    /// Status Code: [`StatusCode::BAD_REQUEST`].
    /// Error ID: `BAD_REQUEST`.
    /// Context: `error`.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest {
            message: message.into(),
        }
    }
}

impl ErrorInfo for ServirError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Db(e) => e.status_code(),
            Self::AuthUser(e) => e.status_code(),
            Self::AuthToken(e) => e.status_code(),
            Self::BadRequest { .. } => StatusCode::BAD_REQUEST,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_id(&self) -> &'static str {
        match self {
            Self::Db(e) => e.error_id(),
            Self::AuthUser(e) => e.error_id(),
            Self::AuthToken(e) => e.error_id(),
            Self::BadRequest { .. } => "BAD_REQUEST",
            Self::NotFound => "NOT_FOUND",
            Self::Io(_) => "IO_ERROR",
        }
    }

    fn context(&self) -> ErrorContext {
        match self {
            Self::Db(e) => e.context(),
            Self::AuthUser(e) => e.context(),
            Self::AuthToken(e) => e.context(),
            Self::BadRequest { message } => ErrorContext::error_only(message),
            Self::NotFound => ErrorContext::empty(),
            Self::Io(e) => ErrorContext::error_only(e),
        }
    }
}

impl From<std::io::Error> for ServirError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl_err_from_info!(ServirError);

impl IntoResponse for ServirError {
    fn into_response(self) -> Response {
        ApiResponse::<()>::Error {
            error: ErrorBody::new(self.status_code(), self.error_id(), self.context()),
        }
        .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_with_identifier_and_error() {
        let err = DbError::Connection {
            url: "sqlite://test.db".to_string(),
            error: "not found".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "DB_CONNECTION_FAILED. url: sqlite://test.db (not found)"
        );
    }

    #[test]
    fn display_with_identifier_no_error() {
        let err = AuthUserError::InvalidCredentials {
            username: "alice".to_string(),
        };
        assert_eq!(err.to_string(), "INVALID_CREDENTIALS. username: alice");
    }

    #[test]
    fn display_without_identifier() {
        let err = AuthTokenError::Expired;
        assert_eq!(err.to_string(), "TOKEN_EXPIRED");
    }

    #[test]
    fn display_error_only() {
        let err = AuthTokenError::Invalid {
            error: "bad signature".to_string(),
        };
        assert_eq!(err.to_string(), "TOKEN_INVALID (bad signature)");
    }

    #[test]
    fn db_error_constructors() {
        let e = DbError::connection("sqlite://x.db", "refused");
        assert_eq!(e.error_id(), "DB_CONNECTION_FAILED");
        assert_eq!(e.status_code(), StatusCode::INTERNAL_SERVER_ERROR);

        let e = DbError::migration("checksum mismatch");
        assert_eq!(e.error_id(), "DB_MIGRATION_FAILED");

        let e = DbError::user_create("alice", "unique violation");
        assert_eq!(e.error_id(), "DB_USER_CREATE_FAILED");

        let e = DbError::user_find_by_username("bob", "timeout");
        assert_eq!(e.error_id(), "DB_USER_FIND_BY_USERNAME_FAILED");

        let e = DbError::user_find_by_id("user-1", "timeout");
        assert_eq!(e.error_id(), "DB_USER_FIND_BY_ID_FAILED");

        let e = DbError::store_refresh_token("alice", "disk full");
        assert_eq!(e.error_id(), "DB_STORE_REFRESH_TOKEN_FAILED");

        let e = DbError::delete_refresh_tokens_for_user("alice", "locked");
        assert_eq!(e.error_id(), "DB_DELETE_REFRESH_TOKENS_FOR_USER_FAILED");

        let e = DbError::consume_refresh_token("row not found");
        assert_eq!(e.error_id(), "DB_CONSUME_REFRESH_TOKEN_FAILED");

        let e = DbError::unknown("some_op", "unexpected");
        assert_eq!(e.error_id(), "DB_UNKNOWN");
    }

    #[test]
    fn servir_error_variants() {
        let e = ServirError::bad_request("invalid json");
        assert_eq!(e.error_id(), "BAD_REQUEST");
        assert_eq!(e.status_code(), StatusCode::BAD_REQUEST);

        let e = ServirError::NotFound;
        assert_eq!(e.error_id(), "NOT_FOUND");
        assert_eq!(e.status_code(), StatusCode::NOT_FOUND);

        let e = ServirError::from(std::io::Error::other("boom"));
        assert_eq!(e.error_id(), "IO_ERROR");
        assert_eq!(e.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn from_impls() {
        let _: ServirError = DbError::Connection {
            url: "x".to_string(),
            error: "e".to_string(),
        }
        .into();

        let _: ServirError = AuthUserError::EmptyUsername.into();
        let _: ServirError = AuthTokenError::Expired.into();
    }
}
