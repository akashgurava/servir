//! Runnable example demonstrating the servir library's injection pattern.
//!
//! Run with:
//!   cargo run --example echo_server --features telemetry
//!
//! Override instance name:
//!   INSTANCE_NAME=node-2 cargo run --example echo_server --features telemetry
//!
//! Override log filter:
//!   RUST_LOG=debug cargo run --example echo_server --features telemetry

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use servir::{ApiResponse, ServirError, init_tracing, start_server_from_env};
use tracing::{info, instrument};

// State — downstream consumer owns this; servir has no knowledge of it.

#[derive(Clone)]
struct AppState {
    instance_name: String,
    request_count: Arc<AtomicUsize>,
}

#[derive(Deserialize)]
struct EchoPayload {
    message: String,
}

#[derive(Serialize)]
struct EchoResponse {
    echo: String,
    from: String,
    total_requests: usize,
}

#[instrument]
async fn ping() -> ApiResponse<&'static str> {
    ApiResponse::ok("pong")
}

#[instrument(skip_all, fields(instance = %state.instance_name))]
async fn echo(
    State(state): State<AppState>,
    Json(payload): Json<EchoPayload>,
) -> Result<ApiResponse<EchoResponse>, ServirError> {
    if payload.message.is_empty() {
        return Err(ServirError::bad_request("message must not be empty"));
    }
    let total_requests = state.request_count.fetch_add(1, Ordering::Relaxed) + 1;
    info!(total_requests, "echo handled");
    Ok(ApiResponse::ok(EchoResponse {
        echo: payload.message,
        from: state.instance_name.clone(),
        total_requests,
    }))
}

#[instrument(skip_all, fields(instance = %state.instance_name))]
async fn request_count(State(state): State<AppState>) -> ApiResponse<usize> {
    let count = state.request_count.load(Ordering::Relaxed);
    info!(count, "count queried");
    ApiResponse::ok(count)
}

#[tokio::main]
async fn main() {
    init_tracing("echo_server");

    let state = AppState {
        instance_name: std::env::var("INSTANCE_NAME").unwrap_or_else(|_| "default".to_string()),
        request_count: Arc::new(AtomicUsize::new(0)),
    };

    let router = Router::new()
        .route("/ping", get(ping))
        .route("/echo", post(echo))
        .route("/count", get(request_count))
        .with_state(state);

    start_server_from_env(3000, router)
        .await
        .expect("server failed");
}
