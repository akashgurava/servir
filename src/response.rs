use std::borrow::Cow;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::error::ServirError;

/// Structured API response envelope.
///
/// Every response — success or error — shares the same top-level shape,
/// discriminated by `status`:
/// ```json
/// { "status": "ok",    "data":  { ... } }
/// { "status": "error", "error": { "code": "NOT_FOUND", "message": "..." } }
/// ```
///
/// # Building responses
///
/// ```rust,ignore
/// // From a value
/// ApiResponse::ok(my_data)
///
/// // From an error
/// ApiResponse::error(ServirError::not_found("item"))
///
/// // From a Result — collapses Ok/Err in one call
/// ApiResponse::from(some_operation())
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
        let (http_status, code, message) = err.into_error_parts();
        Self::Error {
            error: ErrorBody {
                code,
                message,
                http_status,
            },
        }
    }
}

/// Blanket conversion from [`ServirError`].
///
/// Enables `.into()` in handler return position when the success type is known
/// from context.
impl<T: Serialize> From<ServirError> for ApiResponse<T> {
    #[inline]
    fn from(err: ServirError) -> Self {
        Self::error(err)
    }
}

/// Collapse a `Result<T, ServirError>` into an `ApiResponse<T>` in one call.
///
/// Useful for wrapping a fallible operation without an explicit `match`:
/// ```rust,ignore
/// async fn handler() -> ApiResponse<User> {
///     get_user(id).into()
/// }
/// ```
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
    /// Machine-readable discriminant for client-side branching (e.g. `NOT_FOUND`).
    pub code: &'static str,
    /// Human-readable description. Never contains sensitive internal details.
    pub message: Cow<'static, str>,
    /// Sets the HTTP status line; excluded from the JSON body.
    #[serde(skip)]
    pub(crate) http_status: StatusCode,
}

impl<T: Serialize> IntoResponse for ApiResponse<T> {
    #[inline]
    fn into_response(self) -> Response {
        let http_status = match &self {
            Self::Ok { .. } => StatusCode::OK,
            Self::Error { error } => error.http_status,
        };
        (http_status, Json(self)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_wraps_data() {
        let r: ApiResponse<i32> = ApiResponse::ok(42);
        assert!(matches!(r, ApiResponse::Ok { data: 42 }));
    }

    #[test]
    fn error_from_servir_error() {
        let r: ApiResponse<()> = ApiResponse::error(ServirError::not_found("x"));
        let ApiResponse::Error { error } = r else {
            panic!("expected Error variant")
        };
        assert_eq!(error.code, "NOT_FOUND");
        assert_eq!(&*error.message, "x");
        assert_eq!(error.http_status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn from_servir_error() {
        let r: ApiResponse<()> = ServirError::bad_request("oops").into();
        assert!(matches!(r, ApiResponse::Error { .. }));
    }

    #[test]
    fn from_result_ok() {
        let r: ApiResponse<i32> = Ok::<i32, ServirError>(7).into();
        assert!(matches!(r, ApiResponse::Ok { data: 7 }));
    }

    #[test]
    fn from_result_err() {
        let r: ApiResponse<i32> = Err::<i32, ServirError>(ServirError::not_found("x")).into();
        assert!(matches!(r, ApiResponse::Error { .. }));
    }
}
