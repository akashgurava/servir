pub mod error;
pub mod middleware;
pub mod response;
pub mod server;

#[cfg(feature = "telemetry")]
pub mod telemetry;

pub use error::ServirError;
pub use middleware::standard_middleware;
pub use response::{ApiResponse, ErrorBody};
pub use server::{ServerConfig, serve, start_server, start_server_from_env};

#[cfg(feature = "telemetry")]
pub use telemetry::init_tracing;
