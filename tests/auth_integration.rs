//! Auth module integration tests.
//!
//! Each test gets its own in-memory SQLite database for full isolation.
//! Requests are driven via `tower::ServiceExt::oneshot` — no TCP socket needed.

use std::net::SocketAddr;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    routing::get,
};
use serde_json::{Value, json};
use servir::{ApiResponse, AuthUser, Servir};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Test app
// ---------------------------------------------------------------------------

/// A minimal protected handler that echoes the authenticated user's name.
async fn protected(AuthUser(claims): AuthUser) -> ApiResponse<String> {
    ApiResponse::ok(claims.username().to_string())
}

/// Builds a fresh router backed by an isolated in-memory SQLite database.
async fn build_test_app() -> Servir {
    let router = Router::new().route("/protected", get(protected));

    Servir::builder()
        .service_name("test")
        .addr(SocketAddr::from(([127, 0, 0, 1], 0)))
        .auth_database_url("sqlite::memory:")
        .routes(router)
        .build()
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn call(app: &Servir, req: Request<Body>) -> (StatusCode, Value) {
    let response = app.router().clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 65_536)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    (status, json)
}

fn post(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn get_authed(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

/// Register a test user and return `(access_token, refresh_token)`.
async fn register(app: &Servir) -> (String, String) {
    let (status, json) = call(
        app,
        post(
            "/api/v1/auth/register",
            json!({"username": "alice", "password": "hunter22"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "register failed: {json}");
    let access = json["data"]["access_token"].as_str().unwrap().to_string();
    let refresh = json["data"]["refresh_token"].as_str().unwrap().to_string();
    (access, refresh)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn register_returns_tokens() {
    let app = build_test_app().await;
    let (status, json) = call(
        &app,
        post(
            "/api/v1/auth/register",
            json!({"username": "bob", "password": "password1"}),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["status"], "ok");
    assert!(json["data"]["access_token"].is_string());
    assert!(json["data"]["refresh_token"].is_string());
    assert_eq!(json["data"]["token_type"], "Bearer");
}

#[tokio::test]
/// Expected response:
/// {
///   "status": "ok",
///   "data": {
///     "access_token": "string",
///     "refresh_token": "string",
///     "token_type": "Bearer"
///   }
/// }
async fn login_correct_credentials_returns_tokens() {
    let app = build_test_app().await;
    register(&app).await;

    let (status, json) = call(
        &app,
        post(
            "/api/v1/auth/login",
            json!({"username": "alice", "password": "hunter22"}),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["status"], "ok");
    assert!(json["data"]["access_token"].is_string());
    assert!(json["data"]["refresh_token"].is_string());
}

#[tokio::test]
/// Expected response:
/// {
///   "status": "error",
///   "error": {
///     "error_id": "INVALID_CREDENTIALS",
///     "context": {
///       "error": null
///       "username": "alice"
///     },
///   }
/// }
async fn login_wrong_password_returns_401() {
    let app = build_test_app().await;
    register(&app).await;

    let (status, json) = call(
        &app,
        post(
            "/api/v1/auth/login",
            json!({"username": "alice", "password": "wrongpass"}),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["status"], "error");
    assert_eq!(json["error"]["error_id"], "INVALID_CREDENTIALS");
    assert_eq!(json["error"]["context"]["username"], "alice");
}

#[tokio::test]
async fn protected_route_with_valid_token_returns_200() {
    let app = build_test_app().await;
    let (access_token, _) = register(&app).await;

    let (status, json) = call(&app, get_authed("/api/v1/protected", &access_token)).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"], "alice");
}

#[tokio::test]
async fn protected_route_without_token_returns_401() {
    let app = build_test_app().await;

    let (status, json) = call(
        &app,
        Request::builder()
            .uri("/api/v1/protected")
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["error_id"], "MISSING_AUTHORIZATION_HEADER");
}

#[tokio::test]
async fn auth_me_returns_username_and_user_id() {
    let app = build_test_app().await;
    let (access_token, _) = register(&app).await;

    let (status, json) = call(&app, get_authed("/api/v1/auth/me", &access_token)).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"]["username"], "alice");
    assert!(json["data"]["user_id"].is_string());
}

#[tokio::test]
async fn auth_me_without_token_returns_401() {
    let app = build_test_app().await;

    let (status, json) = call(
        &app,
        Request::builder()
            .uri("/api/v1/auth/me")
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["error_id"], "MISSING_AUTHORIZATION_HEADER");
}

#[tokio::test]
async fn refresh_valid_token_returns_new_tokens() {
    let app = build_test_app().await;
    let (_, refresh_token) = register(&app).await;

    let (status, json) = call(
        &app,
        post("/api/v1/auth/refresh", json!({"refresh_token": refresh_token})),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["status"], "ok");
    assert!(json["data"]["access_token"].is_string());
    assert!(json["data"]["refresh_token"].is_string());
    // New refresh token must differ from the consumed one (rotation).
    assert_ne!(json["data"]["refresh_token"], refresh_token);
}

#[tokio::test]
async fn refresh_used_token_returns_401() {
    let app = build_test_app().await;
    let (_, refresh_token) = register(&app).await;

    // First use — valid.
    let (status, _) = call(
        &app,
        post("/api/v1/auth/refresh", json!({"refresh_token": refresh_token})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Second use of the same token — must fail.
    let (status, json) = call(
        &app,
        post("/api/v1/auth/refresh", json!({"refresh_token": refresh_token})),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["error_id"], "INVALID_REFRESH_TOKEN");
}

#[tokio::test]
async fn logout_then_refresh_returns_401() {
    let app = build_test_app().await;
    let (_, refresh_token) = register(&app).await;

    // Logout using the refresh token.
    let (status, json) = call(
        &app,
        post("/api/v1/auth/logout", json!({"refresh_token": refresh_token})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "logout failed: {json}");

    // Subsequent refresh must fail.
    let (status, json) = call(
        &app,
        post("/api/v1/auth/refresh", json!({"refresh_token": refresh_token})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "refresh after logout: {json}"
    );
    assert_eq!(json["error"]["error_id"], "INVALID_REFRESH_TOKEN");
}

#[tokio::test]
async fn refresh_with_invalid_token_returns_401() {
    let app = build_test_app().await;

    let (status, json) = call(
        &app,
        post("/api/v1/auth/refresh", json!({"refresh_token": "not.a.valid.jwt"})),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["error_id"], "TOKEN_INVALID");
}

#[tokio::test]
async fn malformed_json_returns_bad_request_envelope() {
    let app = build_test_app().await;

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/register")
        .header("content-type", "application/json")
        .body(Body::from("{broken"))
        .unwrap();

    let (status, json) = call(&app, req).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["status"], "error");
    assert_eq!(json["error"]["error_id"], "BAD_REQUEST");
    assert!(json["error"]["context"]["error"].is_string());
}

#[tokio::test]
async fn missing_content_type_returns_bad_request_envelope() {
    let app = build_test_app().await;

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/register")
        .body(Body::from(r#"{"username":"a","password":"b"}"#))
        .unwrap();

    let (status, json) = call(&app, req).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["status"], "error");
    assert_eq!(json["error"]["error_id"], "BAD_REQUEST");
    assert!(json["error"]["context"]["error"].is_string());
}
