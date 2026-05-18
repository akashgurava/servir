use std::sync::Arc;
use std::task::{Context, Poll};

use axum::Router;
use axum::extract::{FromRequestParts, State};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::http::{Request, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tower::{Layer, Service};
use tracing::instrument;

use super::core::{
    AuthConfig, Claims, TokenKind, consume_refresh_token, create_user,
    delete_refresh_tokens_for_user, find_user_by_id, find_user_by_username, hash_password,
    issue_access_token, issue_refresh_token, store_refresh_token, verify_password, verify_token,
};
use crate::error::{AuthTokenError, AuthUserError, ServirError};
use crate::{ApiResponse, AppJson};

// ---------------------------------------------------------------------------
// Layer
// ---------------------------------------------------------------------------

/// Tower layer that injects [`AuthConfig`] into request extensions.
///
/// Apply to any router whose handlers use the [`AuthUser`] extractor.
/// Without this layer, `AuthUser` extraction returns 500.
#[derive(Clone)]
pub(crate) struct AuthLayer {
    config: Arc<AuthConfig>,
}

impl AuthLayer {
    pub(crate) fn new(config: AuthConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthService<S>;

    fn layer(&self, inner: S) -> AuthService<S> {
        AuthService {
            inner,
            config: Arc::clone(&self.config),
        }
    }
}

/// The service created by [`AuthLayer`]. Injects config into request extensions.
#[derive(Clone)]
pub struct AuthService<S> {
    inner: S,
    config: Arc<AuthConfig>,
}

impl<S, B, ResBody> Service<Request<B>> for AuthService<S>
where
    S: Service<Request<B>, Response = Response<ResBody>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<B>) -> S::Future {
        req.extensions_mut().insert(Arc::clone(&self.config));
        self.inner.call(req)
    }
}

// ---------------------------------------------------------------------------
// Extractor
// ---------------------------------------------------------------------------

/// Axum extractor that validates the `Authorization: Bearer <token>` header
/// and injects the access token [`Claims`] into the handler.
///
/// Requires [`AuthLayer`] to be applied to the router.
/// Returns 500 if the layer is absent (misconfiguration) and 401 for any
/// missing, malformed, expired, or wrong-kind token.
pub struct AuthUser(pub Claims);

impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = ServirError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let config = parts
            .extensions
            .get::<Arc<AuthConfig>>()
            .ok_or_else(AuthTokenError::missing_layer)?;

        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .ok_or_else(AuthTokenError::missing_header)?;

        let claims = verify_token(token, config)?;

        if *claims.kind() != TokenKind::Access {
            return Err(AuthTokenError::wrong_kind());
        }

        Ok(AuthUser(claims))
    }
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AuthRouteState {
    pool: SqlitePool,
    config: Arc<AuthConfig>,
}

#[derive(Deserialize)]
struct RegisterRequest {
    username: String,
    password: String,
}

#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Deserialize)]
struct RefreshRequest {
    refresh_token: String,
}

#[derive(Deserialize)]
struct LogoutRequest {
    refresh_token: String,
}

#[derive(Serialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    token_type: &'static str,
}

#[derive(Serialize)]
struct MeResponse {
    user_id: String,
    username: String,
}

/// Mounts auth routes onto a self-contained router with its own state.
///
/// Routes:
/// - `POST /auth/register` — create credentials, return tokens
/// - `POST /auth/login` — verify credentials, return tokens
/// - `POST /auth/refresh` — rotate refresh token
/// - `POST /auth/logout` — revoke all refresh tokens
/// - `GET  /auth/me` — return current user info from access token claims
pub(crate) fn mount_auth_routes(pool: SqlitePool, config: AuthConfig) -> Router {
    let state = AuthRouteState {
        pool,
        config: Arc::new(config),
    };
    Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/refresh", post(refresh))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
        .with_state(state)
}

#[instrument(skip_all)]
async fn register(
    State(state): State<AuthRouteState>,
    AppJson(body): AppJson<RegisterRequest>,
) -> Result<ApiResponse<TokenResponse>, ServirError> {
    if body.username.is_empty() {
        return Err(AuthUserError::empty_username());
    }
    if body.password.len() < 8 {
        return Err(AuthUserError::weak_password());
    }

    let hash = hash_password(&body.password)?;
    let user = create_user(&state.pool, &body.username, &hash).await?;

    let access_token = issue_access_token(user.id(), user.username(), &state.config)?;
    let (refresh_token, jti) = issue_refresh_token(user.id(), user.username(), &state.config)?;
    store_refresh_token(
        &state.pool,
        &jti,
        &user,
        state.config.refresh_token_ttl_secs,
    )
    .await?;

    Ok(ApiResponse::ok(TokenResponse {
        access_token,
        refresh_token,
        token_type: "Bearer",
    }))
}

#[instrument(skip_all)]
async fn login(
    State(state): State<AuthRouteState>,
    AppJson(body): AppJson<LoginRequest>,
) -> Result<ApiResponse<TokenResponse>, ServirError> {
    let user = find_user_by_username(&state.pool, &body.username)
        .await?
        .ok_or_else(|| AuthUserError::invalid_credentials(&body.username))?;

    if !verify_password(&body.password, user.password_hash())? {
        return Err(AuthUserError::invalid_credentials(&body.username));
    }

    let access_token = issue_access_token(user.id(), user.username(), &state.config)?;
    let (refresh_token, jti) = issue_refresh_token(user.id(), user.username(), &state.config)?;
    store_refresh_token(
        &state.pool,
        &jti,
        &user,
        state.config.refresh_token_ttl_secs,
    )
    .await?;

    Ok(ApiResponse::ok(TokenResponse {
        access_token,
        refresh_token,
        token_type: "Bearer",
    }))
}

#[instrument(skip_all)]
async fn refresh(
    State(state): State<AuthRouteState>,
    AppJson(body): AppJson<RefreshRequest>,
) -> Result<ApiResponse<TokenResponse>, ServirError> {
    let claims = verify_token(&body.refresh_token, &state.config)?;

    if *claims.kind() != TokenKind::Refresh {
        return Err(AuthTokenError::wrong_kind());
    }

    let valid = consume_refresh_token(&state.pool, &claims.jti()).await?;
    if !valid {
        return Err(AuthTokenError::invalid_refresh_token());
    }

    let user = find_user_by_id(&state.pool, claims.sub())
        .await?
        .ok_or_else(|| AuthTokenError::user_not_found(claims.sub()))?;

    let access_token = issue_access_token(user.id(), user.username(), &state.config)?;
    let (refresh_token, jti) = issue_refresh_token(user.id(), user.username(), &state.config)?;
    store_refresh_token(
        &state.pool,
        &jti,
        &user,
        state.config.refresh_token_ttl_secs,
    )
    .await?;

    Ok(ApiResponse::ok(TokenResponse {
        access_token,
        refresh_token,
        token_type: "Bearer",
    }))
}

/// Revokes all refresh tokens for the user identified by the supplied refresh token.
#[instrument(skip_all)]
async fn logout(
    State(state): State<AuthRouteState>,
    AppJson(body): AppJson<LogoutRequest>,
) -> Result<ApiResponse<()>, ServirError> {
    let claims = verify_token(&body.refresh_token, &state.config)?;

    if *claims.kind() != TokenKind::Refresh {
        return Err(AuthTokenError::wrong_kind());
    }

    delete_refresh_tokens_for_user(&state.pool, claims.sub()).await?;
    Ok(ApiResponse::ok(()))
}

/// Returns the current user's identity from their access token claims.
#[instrument(skip_all, fields(user = %claims.username()))]
async fn me(AuthUser(claims): AuthUser) -> ApiResponse<MeResponse> {
    ApiResponse::ok(MeResponse {
        user_id: claims.sub().to_string(),
        username: claims.username().to_string(),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use serde_json::{Value, json};
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::{Sqlite, pool::PoolOptions};
    use tower::ServiceExt;

    use super::*;

    async fn build_test_app() -> Router {
        let config = AuthConfig::new("test-secret-key-must-be-at-least-32-chars!!");

        let pool = PoolOptions::<Sqlite>::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(":memory:")
                    .create_if_missing(true),
            )
            .await
            .expect("in-memory pool");

        config.migrate(&pool).await.expect("auth migrations");

        async fn protected(AuthUser(claims): AuthUser) -> ApiResponse<String> {
            ApiResponse::ok(claims.username().to_string())
        }

        Router::new()
            .route("/protected", get(protected))
            .merge(mount_auth_routes(pool, config.clone()))
            .layer(AuthLayer::new(config))
    }

    async fn call(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
        let response = app.clone().oneshot(req).await.unwrap();
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
    async fn register(app: &Router) -> (String, String) {
        let (_, json) = call(
            app,
            post("/auth/register", json!({"username": "alice", "password": "hunter22"})),
        )
        .await;
        let access = json["data"]["access_token"].as_str().unwrap().to_string();
        let refresh = json["data"]["refresh_token"].as_str().unwrap().to_string();
        (access, refresh)
    }

    #[tokio::test]
    async fn register_returns_tokens() {
        let app = build_test_app().await;
        let (status, json) = call(
            &app,
            post("/auth/register", json!({"username": "bob", "password": "password1"})),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "ok");
        assert!(json["data"]["access_token"].is_string());
        assert!(json["data"]["refresh_token"].is_string());
        assert_eq!(json["data"]["token_type"], "Bearer");
    }

    #[tokio::test]
    async fn register_empty_username_returns_400() {
        let app = build_test_app().await;
        let (status, json) = call(
            &app,
            post("/auth/register", json!({"username": "", "password": "12345678"})),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"]["error_id"], "EMPTY_USERNAME");
    }

    #[tokio::test]
    async fn register_weak_password_returns_400() {
        let app = build_test_app().await;
        let (status, json) = call(
            &app,
            post("/auth/register", json!({"username": "bob", "password": "short"})),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"]["error_id"], "WEAK_PASSWORD");
    }

    #[tokio::test]
    async fn register_duplicate_returns_409() {
        let app = build_test_app().await;
        register(&app).await;

        let (status, json) = call(
            &app,
            post("/auth/register", json!({"username": "alice", "password": "hunter22"})),
        )
        .await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(json["error"]["error_id"], "ALREADY_EXISTS");
        assert_eq!(json["error"]["context"]["username"], "alice");
    }

    #[tokio::test]
    async fn login_correct_credentials_returns_tokens() {
        let app = build_test_app().await;
        register(&app).await;

        let (status, json) = call(
            &app,
            post("/auth/login", json!({"username": "alice", "password": "hunter22"})),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert!(json["data"]["access_token"].is_string());
        assert!(json["data"]["refresh_token"].is_string());
    }

    #[tokio::test]
    async fn login_wrong_password_returns_401() {
        let app = build_test_app().await;
        register(&app).await;

        let (status, json) = call(
            &app,
            post("/auth/login", json!({"username": "alice", "password": "wrongpass"})),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(json["error"]["error_id"], "INVALID_CREDENTIALS");
        assert_eq!(json["error"]["context"]["username"], "alice");
    }

    #[tokio::test]
    async fn login_nonexistent_user_returns_401() {
        let app = build_test_app().await;

        let (status, json) = call(
            &app,
            post("/auth/login", json!({"username": "nobody", "password": "hunter22"})),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(json["error"]["error_id"], "INVALID_CREDENTIALS");
        assert_eq!(json["error"]["context"]["username"], "nobody");
    }

    #[tokio::test]
    async fn protected_with_valid_token() {
        let app = build_test_app().await;
        let (access, _) = register(&app).await;

        let (status, json) = call(&app, get_authed("/protected", &access)).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["data"], "alice");
    }

    #[tokio::test]
    async fn protected_without_token_returns_401() {
        let app = build_test_app().await;

        let req = Request::builder().uri("/protected").body(Body::empty()).unwrap();
        let (status, json) = call(&app, req).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(json["error"]["error_id"], "MISSING_AUTHORIZATION_HEADER");
    }

    #[tokio::test]
    async fn protected_with_invalid_token_returns_401() {
        let app = build_test_app().await;

        let (status, json) = call(&app, get_authed("/protected", "not.a.valid.jwt")).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(json["error"]["error_id"], "TOKEN_INVALID");
    }

    #[tokio::test]
    async fn protected_with_expired_token_returns_401() {
        use jsonwebtoken::{EncodingKey, Header, encode};
        use serde::Serialize;

        #[derive(Serialize)]
        struct TestClaims {
            sub: String,
            username: String,
            exp: usize,
            iat: usize,
            jti: String,
            kind: String,
        }

        let token = encode(
            &Header::default(),
            &TestClaims {
                sub: "user-123".to_string(),
                username: "alice".to_string(),
                exp: 1_000_000,
                iat: 999_000,
                jti: "jti-expired".to_string(),
                kind: "access".to_string(),
            },
            &EncodingKey::from_secret(b"test-secret-key-must-be-at-least-32-chars!!"),
        )
        .unwrap();

        let app = build_test_app().await;
        let (status, json) = call(&app, get_authed("/protected", &token)).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(json["error"]["error_id"], "TOKEN_EXPIRED");
    }

    #[tokio::test]
    async fn me_returns_user_info() {
        let app = build_test_app().await;
        let (access, _) = register(&app).await;

        let (status, json) = call(&app, get_authed("/auth/me", &access)).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["data"]["username"], "alice");
        assert!(json["data"]["user_id"].is_string());
    }

    #[tokio::test]
    async fn refresh_returns_new_tokens() {
        let app = build_test_app().await;
        let (_, refresh) = register(&app).await;

        let (status, json) = call(
            &app,
            post("/auth/refresh", json!({"refresh_token": refresh})),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert!(json["data"]["access_token"].is_string());
        assert_ne!(json["data"]["refresh_token"], refresh);
    }

    #[tokio::test]
    async fn refresh_consumed_token_returns_401() {
        let app = build_test_app().await;
        let (_, refresh) = register(&app).await;

        // First use — valid.
        call(&app, post("/auth/refresh", json!({"refresh_token": refresh}))).await;

        // Second use — consumed.
        let (status, json) = call(
            &app,
            post("/auth/refresh", json!({"refresh_token": refresh})),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(json["error"]["error_id"], "INVALID_REFRESH_TOKEN");
    }

    #[tokio::test]
    async fn refresh_with_access_token_returns_401() {
        use jsonwebtoken::{EncodingKey, Header, encode};
        use serde::Serialize;

        #[derive(Serialize)]
        struct TestClaims {
            sub: String,
            username: String,
            exp: usize,
            iat: usize,
            jti: String,
            kind: String,
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize;

        let token = encode(
            &Header::default(),
            &TestClaims {
                sub: "user-123".to_string(),
                username: "someone".to_string(),
                exp: now + 3600,
                iat: now,
                jti: "jti-access".to_string(),
                kind: "access".to_string(),
            },
            &EncodingKey::from_secret(b"test-secret-key-must-be-at-least-32-chars!!"),
        )
        .unwrap();

        let app = build_test_app().await;
        let (status, json) = call(
            &app,
            post("/auth/refresh", json!({"refresh_token": token})),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(json["error"]["error_id"], "WRONG_TOKEN_KIND");
    }

    #[tokio::test]
    async fn logout_then_refresh_returns_401() {
        let app = build_test_app().await;
        let (_, refresh) = register(&app).await;

        let (status, _) = call(
            &app,
            post("/auth/logout", json!({"refresh_token": refresh})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, json) = call(
            &app,
            post("/auth/refresh", json!({"refresh_token": refresh})),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(json["error"]["error_id"], "INVALID_REFRESH_TOKEN");
    }

    #[tokio::test]
    async fn malformed_json_returns_400() {
        let app = build_test_app().await;

        let req = Request::builder()
            .method("POST")
            .uri("/auth/register")
            .header("content-type", "application/json")
            .body(Body::from("{broken"))
            .unwrap();

        let (status, json) = call(&app, req).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"]["error_id"], "BAD_REQUEST");
    }

    #[tokio::test]
    async fn missing_content_type_returns_400() {
        let app = build_test_app().await;

        let req = Request::builder()
            .method("POST")
            .uri("/auth/register")
            .body(Body::from(r#"{"username":"a","password":"b"}"#))
            .unwrap();

        let (status, json) = call(&app, req).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"]["error_id"], "BAD_REQUEST");
    }
}
