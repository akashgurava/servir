use std::net::SocketAddr;

use axum::Router;
use tokio::net::TcpListener;
use tracing::{info, instrument};

use crate::middleware::standard_middleware;

/// Server configuration.
#[derive(Debug)]
pub struct ServerConfig {
    /// Address (host + port) to bind.
    addr: SocketAddr,
}

impl ServerConfig {
    #[inline]
    pub fn new(addr: SocketAddr) -> Self {
        Self { addr }
    }
}

/// Binds a TCP listener and serves `router` until the server terminates.
///
/// The actual listening address is logged after bind, which may differ from
/// `config.addr` when port 0 is used for OS-assigned ports.
#[instrument(skip(router), fields(bind_addr = %config.addr))]
pub async fn serve(config: ServerConfig, router: Router) -> Result<(), std::io::Error> {
    let listener = TcpListener::bind(config.addr).await?;
    let local_addr = listener.local_addr()?;
    info!(addr = %local_addr, "server listening");
    axum::serve(listener, router).await
}

/// Applies [`crate::standard_middleware`] then calls [`serve`].
///
/// Use [`serve`] directly when you need to compose middleware layers around the standard stack.
#[instrument(skip(router), fields(addr = %addr))]
pub async fn start_server(addr: SocketAddr, router: Router) -> Result<(), std::io::Error> {
    serve(ServerConfig::new(addr), standard_middleware(router)).await
}

/// Reads `PORT` from the environment, falls back to `default_port`, then calls [`start_server`].
///
/// Binds on `0.0.0.0` to accept connections on all interfaces — required in container runtimes
/// where the host network is not the container network.
///
/// Intended as the standard entry point for containerised deployments:
/// ```text
/// docker run -e PORT=8080 my-image
/// ```
#[instrument(skip(router))]
pub async fn start_server_from_env(
    default_port: u16,
    router: Router,
) -> Result<(), std::io::Error> {
    let port = std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(default_port);
    start_server(SocketAddr::from(([0, 0, 0, 0], port)), router).await
}
