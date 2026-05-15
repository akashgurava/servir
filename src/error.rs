use std::borrow::Cow;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use thiserror::Error;

use crate::response::{ApiResponse, ErrorBody};

/// Transport-agnostic error type.
///
/// Error variants carry structured data. HTTP mapping is confined to the
/// `IntoResponse` impl — the variants themselves carry no HTTP concerns.
#[derive(Debug, Error)]
pub enum ServirError {
    #[error("NOT_FOUND: {resource}")]
    NotFound { resource: String },

    #[error("BAD_REQUEST: {reason}")]
    BadRequest { reason: String },

    #[error("INTERNAL_ERROR")]
    Internal {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl ServirError {
    #[inline]
    pub fn not_found(resource: impl Into<String>) -> Self {
        Self::NotFound {
            resource: resource.into(),
        }
    }

    #[inline]
    pub fn bad_request(reason: impl Into<String>) -> Self {
        Self::BadRequest {
            reason: reason.into(),
        }
    }

    #[inline]
    pub fn internal(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Internal {
            source: Box::new(source),
        }
    }

    /// Decomposes the error into its HTTP status, machine-readable code, and
    /// client-safe message in a single pass. Internal error sources are logged
    /// here and never reach the message field.
    #[inline]
    pub(crate) fn into_error_parts(self) -> (StatusCode, &'static str, Cow<'static, str>) {
        match self {
            Self::NotFound { resource } => {
                (StatusCode::NOT_FOUND, "NOT_FOUND", Cow::Owned(resource))
            }
            Self::BadRequest { reason } => {
                (StatusCode::BAD_REQUEST, "BAD_REQUEST", Cow::Owned(reason))
            }
            Self::Internal { source } => {
                tracing::error!(error = %source, "INTERNAL_ERROR");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "INTERNAL_ERROR",
                    Cow::Borrowed("an unexpected error occurred"),
                )
            }
        }
    }
}

/// [`ServirError::Internal`] sources are logged via `tracing::error!` and never forwarded to the response body.
impl IntoResponse for ServirError {
    #[inline]
    fn into_response(self) -> Response {
        let (http_status, code, message) = self.into_error_parts();
        ApiResponse::<()>::Error {
            error: ErrorBody {
                code,
                message,
                http_status,
            },
        }
        .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_found_display() {
        assert_eq!(
            ServirError::not_found("widget").to_string(),
            "NOT_FOUND: widget"
        );
    }

    #[test]
    fn bad_request_display() {
        assert_eq!(
            ServirError::bad_request("missing field").to_string(),
            "BAD_REQUEST: missing field"
        );
    }

    #[test]
    fn internal_display_does_not_expose_source() {
        assert_eq!(
            ServirError::internal(std::io::Error::other("secret")).to_string(),
            "INTERNAL_ERROR"
        );
    }

    #[test]
    fn into_error_parts_maps_all_fields() {
        let (status, code, msg) = ServirError::not_found("widget").into_error_parts();
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(code, "NOT_FOUND");
        assert_eq!(&*msg, "widget");

        let (status, code, msg) = ServirError::bad_request("bad thing").into_error_parts();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(code, "BAD_REQUEST");
        assert_eq!(&*msg, "bad thing");

        let (status, code, msg) =
            ServirError::internal(std::io::Error::other("secret")).into_error_parts();
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(code, "INTERNAL_ERROR");
        // message must not contain the source
        assert_eq!(&*msg, "an unexpected error occurred");
        assert!(!msg.contains("secret"));
    }
}
