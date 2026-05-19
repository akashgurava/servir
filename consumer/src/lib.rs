pub mod db;

use axum::{Router, extract::State, routing::get};
use serde::Serialize;
use servir::{ApiResponse, AuthUser, ServirError};
use sqlx::SqlitePool;
use tracing::instrument;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
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
    let profile = db::find_or_create_profile(&state.pool, claims.sub(), claims.username()).await?;
    Ok(ApiResponse::ok(UserResponse {
        id: profile.id,
        username: profile.username,
    }))
}

pub fn app_routes(state: AppState) -> Router {
    Router::new().route("/user", get(user)).with_state(state)
}
