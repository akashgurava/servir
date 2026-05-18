use std::net::SocketAddr;

use axum::{Router, routing::get};
use tokio::net::TcpListener;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer},
};
use tracing::{Level, info, instrument};

use crate::error::ServirError;
use crate::response::ApiResponse;

// ---------------------------------------------------------------------------
// Telemetry
// ---------------------------------------------------------------------------

/// Initialises the global `tracing` subscriber.
///
/// Filter precedence: `RUST_LOG` env var → `{service_name}=debug,servir=debug,tower_http=debug`.
/// Safe to call multiple times — subsequent calls are no-ops.
#[cfg(feature = "telemetry")]
pub fn init_tracing(service_name: &str) {
    use tracing_subscriber::EnvFilter;

    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            format!("{service_name}=debug,servir=debug,tower_http=debug").into()
        }))
        .try_init();
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

/// Applies the standard middleware stack to a router.
///
/// Layer order (outermost → innermost, i.e. first to see the request):
///   1. `SetRequestIdLayer`       — assigns `x-request-id` if absent (UUID v4)
///   2. `PropagateRequestIdLayer` — echoes `x-request-id` back on the response
///   3. `TraceLayer`              — emits an INFO span per request (method, uri, status, latency)
///
/// Accepts `Router<()>` — call `.with_state(state)` before passing in.
fn standard_middleware(router: Router) -> Router {
    router
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_request(())
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
}

// ---------------------------------------------------------------------------
// Extract
// ---------------------------------------------------------------------------

/// Drop-in replacement for [`axum::Json`] that maps parse failures
/// into [`ServirError::BadRequest`], ensuring the error envelope is preserved.
///
/// Use this instead of `axum::Json` in handlers to guarantee that malformed
/// request bodies produce a structured `ApiResponse::Error` rather than
/// axum's default plain-text rejection.
pub struct AppJson<T>(pub T);

impl<T, S> axum::extract::FromRequest<S> for AppJson<T>
where
    axum::Json<T>:
        axum::extract::FromRequest<S, Rejection = axum::extract::rejection::JsonRejection>,
    S: Send + Sync,
{
    type Rejection = ServirError;

    async fn from_request(
        req: axum::http::Request<axum::body::Body>,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        axum::Json::<T>::from_request(req, state)
            .await
            .map(|axum::Json(v)| AppJson(v))
            .map_err(|e| ServirError::bad_request(e.body_text()))
    }
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

/// Binds a TCP listener and serves `router` until the server terminates.
///
/// The actual listening address is logged after bind, which may differ from
/// `addr` when port 0 is used for OS-assigned ports.
#[instrument(skip(router), fields(bind_addr = %addr))]
async fn serve(addr: SocketAddr, router: Router) -> Result<(), std::io::Error> {
    let listener = TcpListener::bind(addr).await?;
    let local_addr = listener.local_addr()?;
    info!(addr = %local_addr, "server listening");
    axum::serve(listener, router).await
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Entry point for building a servir-managed HTTP server.
///
/// # Example
///
/// ```rust,ignore
/// let routes = Router::new()
///     .route("/user", get(user_handler))
///     .with_state(app_state);
///
/// Servir::builder()
///     .service_name("my-app")
///     .port(3000)
///     .routes(routes)
///     .serve()
///     .await
///     .expect("server failed");
/// ```
///
/// The builder handles: tracing initialization, auth database setup,
/// mounting auth routes, applying `AuthLayer`, `/health` endpoint,
/// 404 fallback, and standard middleware (request IDs + tracing).
pub struct Servir {
    router: Router,
    addr: SocketAddr,
}

impl Servir {
    pub fn builder() -> ServerBuilder {
        ServerBuilder {
            service_name: None,
            addr: None,
            routes: None,
            #[cfg(feature = "auth")]
            auth_database_url: None,
        }
    }
}

impl Servir {
    pub fn new(router: Router, addr: SocketAddr) -> Self {
        Self { router, addr }
    }

    pub fn router(&self) -> &Router {
        &self.router
    }

    pub async fn serve(self) -> Result<(), ServirError> {
        serve(self.addr, self.router).await?;
        Ok(())
    }
}

/// Builder for configuring and launching a servir server.
///
/// Handles tracing, auth setup, health endpoint, consistent 404 responses,
/// and standard middleware automatically.
pub struct ServerBuilder {
    service_name: Option<String>,
    addr: Option<SocketAddr>,
    routes: Option<Router>,
    #[cfg(feature = "auth")]
    auth_database_url: Option<String>,
}

impl ServerBuilder {
    /// Sets the service name used for tracing spans and log filtering.
    pub fn service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = Some(name.into());
        self
    }

    /// Sets the socket address to bind on. Required.
    pub fn addr(mut self, addr: impl Into<SocketAddr>) -> Self {
        self.addr = Some(addr.into());
        self
    }

    /// Sets the consumer's application routes (must already have state applied via `.with_state()`).
    pub fn routes(mut self, router: Router) -> Self {
        self.routes = Some(router);
        self
    }

    /// Overrides the auth database URL. Defaults to `AUTH_DATABASE_URL` env var or `sqlite://auth.db`.
    #[cfg(feature = "auth")]
    pub fn auth_database_url(mut self, url: impl Into<String>) -> Self {
        self.auth_database_url = Some(url.into());
        self
    }

    pub async fn build(self) -> Result<Servir, ServirError> {
        let service_name = self.service_name.unwrap_or_else(|| "servir".to_string());

        #[cfg(feature = "telemetry")]
        init_tracing(&service_name);

        // Suppress unused variable warning when telemetry is off.
        #[cfg(not(feature = "telemetry"))]
        let _ = &service_name;

        let mut api = self
            .routes
            .expect("routes must be set before calling serve()");

        #[cfg(feature = "auth")]
        {
            use std::str::FromStr;
            use sqlx::sqlite::SqliteConnectOptions;

            let database_url = self
                .auth_database_url
                .or_else(|| std::env::var("AUTH_DATABASE_URL").ok())
                .unwrap_or_else(|| "sqlite://auth.db".to_string());

            let opts = SqliteConnectOptions::from_str(&database_url)
                .map_err(|e| crate::error::DbError::connection(&database_url, e))?
                .create_if_missing(true);
            let auth_pool = sqlx::SqlitePool::connect_with(opts)
                .await
                .map_err(|e| crate::error::DbError::connection(&database_url, e))?;

            let mut auth_config = crate::auth::AuthConfig::from_env();
            auth_config.migrate(&auth_pool).await?;
            auth_config.load_or_generate_secret(&auth_pool).await?;
            api = api.merge(crate::auth::mount_auth_routes(
                auth_pool,
                auth_config.clone(),
            ));
            api = api.route("/health", get(health));
            api = api.layer(crate::auth::AuthLayer::new(auth_config));
        }

        #[cfg(not(feature = "auth"))]
        {
            api = api.route("/health", get(health));
        }

        let router = Router::new().nest("/api/v1", api).fallback(not_found);
        let router = standard_middleware(router);

        let addr = self
            .addr
            .expect("addr must be set before calling build()");

        Ok(Servir::new(router, addr))
    }

    /// Initializes tracing, sets up auth, attaches middleware, and starts serving.
    pub async fn serve(self) -> Result<(), ServirError> {
        let servir = self.build().await?;
        servir.serve().await?;

        Ok(())
    }
}

async fn health() -> ApiResponse<HealthResponse> {
    ApiResponse::ok(HealthResponse { status: "healthy" })
}

#[derive(serde::Serialize)]
struct HealthResponse {
    status: &'static str,
}

async fn not_found() -> ApiResponse<()> {
    ApiResponse::error(ServirError::NotFound)
}
