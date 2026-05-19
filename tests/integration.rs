//! Integration tests for the servir library.
//!
//! Uses `tower::ServiceExt::oneshot` to drive a real axum `Router` without
//! binding a TCP socket. All middleware is exercised end-to-end.

use std::net::SocketAddr;

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    routing::get,
};
use serde_json::Value;
use servir::{ApiResponse, DbError, Servir, ServirError};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Minimal concrete state
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct TestState {
    label: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn echo(State(state): State<TestState>) -> ApiResponse<String> {
    ApiResponse::ok(state.label.clone())
}

async fn trigger_db_error() -> Result<ApiResponse<()>, ServirError> {
    Err(DbError::consume_refresh_token("forced"))
}

// ---------------------------------------------------------------------------
// App factory
// ---------------------------------------------------------------------------

async fn build_app() -> Servir {
    let state = TestState {
        label: "test".to_string(),
    };
    Servir::builder()
        .service_name("test")
        .addr(SocketAddr::from(([127, 0, 0, 1], 0)))
        .auth_database_url("sqlite::memory:")
        .routes(
            Router::new()
                .route("/echo", get(echo))
                .route("/db-error", get(trigger_db_error))
                .with_state(state),
        )
        .build()
        .await
        .unwrap()
}

async fn body_json(servir: &Servir, uri: &str) -> (StatusCode, Value) {
    let response = servir
        .router()
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    (status, json)
}

// ---------------------------------------------------------------------------
// Response shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn success_response_shape() {
    let app = build_app().await;
    let (status, json) = body_json(&app, "/api/v1/echo").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"], "test");
}

#[tokio::test]
async fn health_endpoint() {
    let app = build_app().await;
    let (status, json) = body_json(&app, "/api/v1/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json,
        serde_json::json!({"status": "ok", "data": {"status": "healthy"}})
    );
}

#[tokio::test]
async fn error_response_shape_internal_error() {
    let app = build_app().await;
    let (status, json) = body_json(&app, "/api/v1/db-error").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(json["status"], "error");
    assert_eq!(json["error"]["error_id"], "DB_CONSUME_REFRESH_TOKEN_FAILED");
    assert_eq!(json["error"]["context"]["error"], "forced");
}

#[tokio::test]
async fn unknown_route_returns_404() {
    let app = build_app().await;
    let (status, json) = body_json(&app, "/does-not-exist").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["error_id"], "NOT_FOUND");
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

#[tokio::test]
async fn middleware_generates_request_id_on_response() {
    let app = build_app().await;
    let response = app
        .router()
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(response.headers().contains_key("x-request-id"));
}

#[tokio::test]
async fn middleware_propagates_caller_supplied_request_id() {
    let app = build_app().await;
    let supplied_id = "caller-supplied-id-abc123";
    let response = app
        .router()
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/health")
                .header("x-request-id", supplied_id)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let returned_id = response
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok());
    assert_eq!(returned_id, Some(supplied_id));
}
