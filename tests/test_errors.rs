//! Comprehensive integration test for all servir routes and error responses.
//!
//! Uses the builder with an in-memory SQLite database and `tower::ServiceExt::oneshot` —
//! no TCP socket, no running server process.
//!
//! Tests that require crafted JWTs (expired token, wrong-kind token) are covered
//! by the in-crate unit tests in `src/auth/http.rs`.

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
// Test app setup
// ---------------------------------------------------------------------------

async fn protected_authed(AuthUser(claims): AuthUser) -> ApiResponse<String> {
    ApiResponse::ok(claims.username().to_string())
}

async fn build_app() -> Servir {
    let router = Router::new().route("/protected", get(protected_authed));

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

fn get_req(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
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

fn post_json(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

// ---------------------------------------------------------------------------
// Single comprehensive test
// ---------------------------------------------------------------------------

/// Tests all routes and error responses with full JSON structure verification.
///
/// Each section documents the exact expected response shape.
#[tokio::test]
async fn test_all_routes_and_errors() {
    let app = build_app().await;

    // =========================================================================
    // 1. GET /api/v1/health -> 200 OK
    // =========================================================================
    let (status, json) = call(&app, get_req("/api/v1/health")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json, json!({"status": "ok", "data": {"status": "healthy"}}));

    // =========================================================================
    // 2. GET /nonexistent -> 404 NOT_FOUND
    // =========================================================================
    let (status, json) = call(&app, get_req("/nonexistent")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        json,
        json!({"status": "error", "error": {"error_id": "NOT_FOUND", "context": {"error": null}}})
    );

    // =========================================================================
    // 3. POST /api/v1/auth/register - empty username -> 400 EMPTY_USERNAME
    // =========================================================================
    let (status, json) = call(
        &app,
        post_json(
            "/api/v1/auth/register",
            json!({"username": "", "password": "12345678"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        json,
        json!({"status": "error", "error": {"error_id": "EMPTY_USERNAME", "context": {"error": null}}})
    );

    // =========================================================================
    // 4. POST /api/v1/auth/register - weak password -> 400 WEAK_PASSWORD
    // =========================================================================
    let (status, json) = call(
        &app,
        post_json(
            "/api/v1/auth/register",
            json!({"username": "bob", "password": "short"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        json,
        json!({"status": "error", "error": {"error_id": "WEAK_PASSWORD", "context": {"error": null}}})
    );

    // =========================================================================
    // 5. POST /api/v1/auth/register - success
    // =========================================================================
    let (status, json) = call(
        &app,
        post_json(
            "/api/v1/auth/register",
            json!({"username": "alice", "password": "hunter22!"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["status"], "ok");
    assert!(json["data"]["access_token"].is_string());
    assert!(json["data"]["refresh_token"].is_string());
    assert_eq!(json["data"]["token_type"], "Bearer");
    let access_token = json["data"]["access_token"].as_str().unwrap().to_string();
    let refresh_token = json["data"]["refresh_token"].as_str().unwrap().to_string();

    // =========================================================================
    // 6. POST /api/v1/auth/register - duplicate username -> 409 ALREADY_EXISTS
    // =========================================================================
    let (status, json) = call(
        &app,
        post_json(
            "/api/v1/auth/register",
            json!({"username": "alice", "password": "hunter22!"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["status"], "error");
    assert_eq!(json["error"]["error_id"], "ALREADY_EXISTS");
    assert_eq!(json["error"]["context"]["username"], "alice");
    assert!(json["error"]["context"]["error"].is_string());

    // =========================================================================
    // 7. POST /api/v1/auth/login - wrong password -> 401 INVALID_CREDENTIALS
    // =========================================================================
    let (status, json) = call(
        &app,
        post_json(
            "/api/v1/auth/login",
            json!({"username": "alice", "password": "wrongpass"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["status"], "error");
    assert_eq!(json["error"]["error_id"], "INVALID_CREDENTIALS");
    assert_eq!(json["error"]["context"]["username"], "alice");
    assert_eq!(json["error"]["context"]["error"], json!(null));

    // =========================================================================
    // 8. POST /api/v1/auth/login - nonexistent user -> 401 INVALID_CREDENTIALS
    // =========================================================================
    let (status, json) = call(
        &app,
        post_json(
            "/api/v1/auth/login",
            json!({"username": "nobody", "password": "hunter22!"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["status"], "error");
    assert_eq!(json["error"]["error_id"], "INVALID_CREDENTIALS");
    assert_eq!(json["error"]["context"]["username"], "nobody");

    // =========================================================================
    // 9. POST /api/v1/auth/login - success
    // =========================================================================
    let (status, json) = call(
        &app,
        post_json(
            "/api/v1/auth/login",
            json!({"username": "alice", "password": "hunter22!"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["status"], "ok");
    assert!(json["data"]["access_token"].is_string());
    assert!(json["data"]["refresh_token"].is_string());
    assert_eq!(json["data"]["token_type"], "Bearer");

    // =========================================================================
    // 10. GET /api/v1/protected - no token -> 401 MISSING_AUTHORIZATION_HEADER
    // =========================================================================
    let (status, json) = call(&app, get_req("/api/v1/protected")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        json,
        json!({"status": "error", "error": {"error_id": "MISSING_AUTHORIZATION_HEADER", "context": {"error": null}}})
    );

    // =========================================================================
    // 11. GET /api/v1/protected - invalid token -> 401 TOKEN_INVALID
    // =========================================================================
    let (status, json) = call(&app, get_authed("/api/v1/protected", "not.a.valid.jwt")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["status"], "error");
    assert_eq!(json["error"]["error_id"], "TOKEN_INVALID");
    assert!(json["error"]["context"]["error"].is_string());

    // =========================================================================
    // 12. GET /api/v1/protected - valid token -> 200 OK
    // =========================================================================
    let (status, json) = call(&app, get_authed("/api/v1/protected", &access_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json, json!({"status": "ok", "data": "alice"}));

    // =========================================================================
    // 13. GET /api/v1/auth/me - valid token -> 200 OK
    // =========================================================================
    let (status, json) = call(&app, get_authed("/api/v1/auth/me", &access_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["status"], "ok");
    assert_eq!(json["data"]["username"], "alice");
    assert!(json["data"]["user_id"].is_string());

    // =========================================================================
    // 14. POST /api/v1/auth/refresh - invalid token -> 401 TOKEN_INVALID
    // =========================================================================
    let (status, json) = call(
        &app,
        post_json(
            "/api/v1/auth/refresh",
            json!({"refresh_token": "garbage.token.here"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["status"], "error");
    assert_eq!(json["error"]["error_id"], "TOKEN_INVALID");
    assert!(json["error"]["context"]["error"].is_string());

    // =========================================================================
    // 15. POST /api/v1/auth/refresh - valid -> 200 OK with new tokens
    // =========================================================================
    let (status, json) = call(
        &app,
        post_json(
            "/api/v1/auth/refresh",
            json!({"refresh_token": refresh_token}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["status"], "ok");
    assert!(json["data"]["access_token"].is_string());
    assert!(json["data"]["refresh_token"].is_string());
    assert_eq!(json["data"]["token_type"], "Bearer");
    assert_ne!(json["data"]["refresh_token"], refresh_token);
    let new_refresh = json["data"]["refresh_token"].as_str().unwrap().to_string();

    // =========================================================================
    // 16. POST /api/v1/auth/refresh - reuse consumed token -> 401
    // =========================================================================
    let (status, json) = call(
        &app,
        post_json(
            "/api/v1/auth/refresh",
            json!({"refresh_token": refresh_token}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        json,
        json!({"status": "error", "error": {"error_id": "INVALID_REFRESH_TOKEN", "context": {"error": null}}})
    );

    // =========================================================================
    // 17. POST /api/v1/auth/logout - valid refresh -> 200 OK
    // =========================================================================
    let (status, json) = call(
        &app,
        post_json("/api/v1/auth/logout", json!({"refresh_token": new_refresh})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json, json!({"status": "ok", "data": null}));

    // =========================================================================
    // 18. POST /api/v1/auth/refresh - after logout -> 401
    // =========================================================================
    let (status, json) = call(
        &app,
        post_json(
            "/api/v1/auth/refresh",
            json!({"refresh_token": new_refresh}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        json,
        json!({"status": "error", "error": {"error_id": "INVALID_REFRESH_TOKEN", "context": {"error": null}}})
    );
}
