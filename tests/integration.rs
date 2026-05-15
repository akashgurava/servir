//! Integration tests for the servir library.
//!
//! Uses `tower::ServiceExt::oneshot` to drive a real axum `Router` without
//! binding a TCP socket. All middleware is exercised end-to-end.

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    routing::get,
};
use serde_json::Value;
use servir::{ApiResponse, ServirError, standard_middleware};
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

async fn health(State(state): State<TestState>) -> ApiResponse<String> {
    ApiResponse::ok(state.label.clone())
}

async fn trigger_not_found() -> Result<ApiResponse<()>, ServirError> {
    Err(ServirError::not_found("item"))
}

async fn trigger_bad_request() -> Result<ApiResponse<()>, ServirError> {
    Err(ServirError::bad_request("invalid input"))
}

// ---------------------------------------------------------------------------
// App factory
// ---------------------------------------------------------------------------

fn build_app() -> Router {
    let state = TestState {
        label: "test".to_string(),
    };
    let router = Router::new()
        .route("/health", get(health))
        .route("/not-found", get(trigger_not_found))
        .route("/bad-request", get(trigger_bad_request))
        .with_state(state);
    standard_middleware(router)
}

async fn body_json(router: Router, uri: &str) -> (StatusCode, Value) {
    let response = router
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
    let (status, json) = body_json(build_app(), "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"], "test");
}

#[tokio::test]
async fn error_response_shape_not_found() {
    let (status, json) = body_json(build_app(), "/not-found").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["status"], "error");
    assert_eq!(json["error"]["code"], "NOT_FOUND");
    assert_eq!(json["error"]["message"], "item");
}

#[tokio::test]
async fn error_response_shape_bad_request() {
    let (status, json) = body_json(build_app(), "/bad-request").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["status"], "error");
    assert_eq!(json["error"]["code"], "BAD_REQUEST");
    assert_eq!(json["error"]["message"], "invalid input");
}

#[tokio::test]
async fn unknown_route_returns_404() {
    // Axum's built-in fallback returns an empty body, not an ApiResponse.
    let response = build_app()
        .oneshot(
            Request::builder()
                .uri("/does-not-exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

#[tokio::test]
async fn middleware_generates_request_id_on_response() {
    let response = build_app()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(response.headers().contains_key("x-request-id"));
}

#[tokio::test]
async fn middleware_propagates_caller_supplied_request_id() {
    let supplied_id = "caller-supplied-id-abc123";
    let response = build_app()
        .oneshot(
            Request::builder()
                .uri("/health")
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
