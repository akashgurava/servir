mod error;
mod response;
mod server;

#[cfg(feature = "auth")]
mod auth;

pub use error::{AuthTokenError, AuthUserError, DbError, ServirError};
pub use response::ApiResponse;
pub use server::{AppJson, ServerBuilder, Servir};

#[cfg(feature = "telemetry")]
pub use server::init_tracing;

#[cfg(feature = "auth")]
pub use auth::{AuthUser, Claims};
