use std::net::SocketAddr;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use echo_server::{AppState, app_routes, db::Db};
use serde_json::{Value, json};
use servir::Servir;
use tower::ServiceExt;

// ---------------------------------Test app----------------------------------

async fn build_test_app() -> Servir {
    let db = Db::connect("sqlite::memory:").await.unwrap();
    db.migrate().await.unwrap();

    let state = AppState { db };

    Servir::builder()
        .service_name("test-consumer")
        .addr(SocketAddr::from(([127, 0, 0, 1], 0)))
        .auth_database_url("sqlite::memory:")
        .routes(app_routes(state))
        .build()
        .await
        .unwrap()
}

// ---------------------------------Helpers----------------------------------

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

async fn register(app: &Servir) -> (String, String) {
    let (_, json) = call(
        app,
        post(
            "/api/v1/auth/register",
            json!({"username": "alice", "password": "hunter22"}),
        ),
    )
    .await;
    let access = json["data"]["access_token"].as_str().unwrap().to_string();
    let refresh = json["data"]["refresh_token"].as_str().unwrap().to_string();
    (access, refresh)
}

// ---------------------------------Tests----------------------------------

#[tokio::test]
async fn user_endpoint_requires_auth() {
    let app = build_test_app().await;

    let req = Request::builder()
        .uri("/api/v1/user")
        .body(Body::empty())
        .unwrap();
    let (status, json) = call(&app, req).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"]["error_id"], "MISSING_AUTHORIZATION_HEADER");
}

#[tokio::test]
async fn user_endpoint_creates_profile() {
    let app = build_test_app().await;
    let (access, _) = register(&app).await;

    let (status, json) = call(&app, get_authed("/api/v1/user", &access)).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["username"], "alice");
    assert!(json["data"]["id"].is_string());
}

#[tokio::test]
async fn user_endpoint_returns_existing_profile() {
    let app = build_test_app().await;
    let (access, _) = register(&app).await;

    // First call creates.
    let (_, json1) = call(&app, get_authed("/api/v1/user", &access)).await;
    // Second call finds existing.
    let (_, json2) = call(&app, get_authed("/api/v1/user", &access)).await;

    assert_eq!(json1["data"]["id"], json2["data"]["id"]);
    assert_eq!(json1["data"]["username"], json2["data"]["username"]);
}
