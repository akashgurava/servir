mod db;

use std::net::SocketAddr;

use axum::{Router, extract::State, routing::get};
use serde::Serialize;
use servir::{ApiResponse, AuthUser, Servir, ServirError};
use sqlx::SqlitePool;
use tracing::instrument;

#[derive(Clone)]
struct AppState {
    pool: SqlitePool,
}

#[derive(Serialize)]
struct UserResponse {
    id: String,
    username: String,
}

/// Returns the authenticated user's app profile, creating it on first access.
#[instrument(skip_all, fields(user = %claims.username()))]
async fn user(
    AuthUser(claims): AuthUser,
    State(state): State<AppState>,
) -> Result<ApiResponse<UserResponse>, ServirError> {
    let profile =
        db::find_or_create_profile(&state.pool, claims.sub(), claims.username()).await?;
    Ok(ApiResponse::ok(UserResponse {
        id: profile.id,
        username: profile.username,
    }))
}

#[tokio::main]
async fn main() {
    let app_db_url =
        std::env::var("APP_DATABASE_URL").unwrap_or_else(|_| "sqlite://app.db".to_string());
    let app_pool = db::connect(&app_db_url).await.expect("app db connect");
    db::migrate(&app_pool).await.expect("app db migrate");

    let state = AppState { pool: app_pool };
    let routes = Router::new()
        .route("/user", get(user))
        .with_state(state);

    Servir::builder()
        .service_name("echo-server")
        .addr(SocketAddr::from(([0, 0, 0, 0], 3000)))
        .routes(routes)
        .serve()
        .await
        .expect("server failed");
}
