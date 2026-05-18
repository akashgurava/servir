use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::error::{ErrorContext, ErrorInfo, ServirError};

/// Structured API response envelope.
///
/// Every response — success or error — shares the same top-level shape,
/// discriminated by `status`:
/// ```json
/// { "status": "ok",    "data": { ... } }
/// { "status": "error", "error": { "error_id": "INVALID_CREDENTIALS", "context": { "error": null, "username": "alice" } } }
/// ```
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum ApiResponse<T: Serialize> {
    Ok { data: T },
    Error { error: ErrorBody },
}

impl<T: Serialize> ApiResponse<T> {
    #[inline]
    pub fn ok(data: T) -> Self {
        Self::Ok { data }
    }

    #[inline]
    pub fn error(err: ServirError) -> Self {
        Self::Error {
            error: ErrorBody::new(err.status_code(), err.error_id(), err.context()),
        }
    }

    /// Transforms the inner data of a successful response.
    ///
    /// If this is an `Error` variant, the error is preserved unchanged.
    #[inline]
    pub fn map<U: Serialize, F: FnOnce(T) -> U>(self, f: F) -> ApiResponse<U> {
        match self {
            Self::Ok { data } => ApiResponse::Ok { data: f(data) },
            Self::Error { error } => ApiResponse::Error { error },
        }
    }
}

/// Blanket conversion from [`ServirError`].
impl<T: Serialize> From<ServirError> for ApiResponse<T> {
    #[inline]
    fn from(err: ServirError) -> Self {
        Self::error(err)
    }
}

/// Collapse a `Result<T, ServirError>` into an `ApiResponse<T>` in one call.
impl<T: Serialize> From<Result<T, ServirError>> for ApiResponse<T> {
    #[inline]
    fn from(result: Result<T, ServirError>) -> Self {
        match result {
            Ok(data) => Self::ok(data),
            Err(err) => Self::error(err),
        }
    }
}

/// Error payload carried inside [`ApiResponse::Error`].
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    /// Sets the HTTP status line; excluded from the JSON body.
    #[serde(skip)]
    status_code: StatusCode,
    /// Machine-readable error discriminant, e.g. `DB_QUERY_FAILED`.
    error_id: &'static str,
    /// Structured context: always contains `"error"` (string or null)
    /// plus any identifier relevant to the error.
    context: ErrorContext,
}

impl ErrorBody {
    #[inline]
    pub(crate) fn new(
        status_code: StatusCode,
        error_id: &'static str,
        context: ErrorContext,
    ) -> Self {
        Self {
            status_code,
            error_id,
            context,
        }
    }
}

impl<T: Serialize> IntoResponse for ApiResponse<T> {
    #[inline]
    fn into_response(self) -> Response {
        let http_status = match &self {
            Self::Ok { .. } => StatusCode::OK,
            Self::Error { error } => error.status_code,
        };
        (http_status, Json(self)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{AuthTokenError, AuthUserError, DbError};

    #[test]
    fn ok_wraps_data() {
        let r: ApiResponse<i32> = ApiResponse::ok(42);
        assert!(matches!(r, ApiResponse::Ok { data: 42 }));
    }

    #[test]
    fn error_from_servir_error() {
        let r: ApiResponse<()> =
            ApiResponse::error(ServirError::AuthToken(AuthTokenError::Expired));
        let ApiResponse::Error { error } = r else {
            panic!("expected Error variant")
        };
        assert_eq!(error.error_id, "TOKEN_EXPIRED");
        assert_eq!(error.status_code, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn from_servir_error() {
        let r: ApiResponse<()> = ServirError::AuthUser(AuthUserError::EmptyUsername).into();
        assert!(matches!(r, ApiResponse::Error { .. }));
    }

    #[test]
    fn from_result_ok() {
        let r: ApiResponse<i32> = Ok::<i32, ServirError>(7).into();
        assert!(matches!(r, ApiResponse::Ok { data: 7 }));
    }

    #[test]
    fn from_result_err() {
        let r: ApiResponse<i32> = Err::<i32, ServirError>(DbError::migration("")).into();
        assert!(matches!(r, ApiResponse::Error { .. }));
    }
}
