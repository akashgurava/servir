use tracing_subscriber::EnvFilter;

/// Initialises the global `tracing` subscriber.
///
/// Filter precedence: `RUST_LOG` env var → `{service_name}=debug,servir=debug`.
/// Panics if called more than once. Call at process start before any async work.
pub fn init_tracing(service_name: &str) {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("{service_name}=debug,servir=debug").into()),
        )
        .init();
}
