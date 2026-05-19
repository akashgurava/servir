# servir

[![CI](https://github.com/akashgurava/servir/actions/workflows/ci.yml/badge.svg)](https://github.com/akashgurava/servir/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/akashgurava/servir/branch/main/graph/badge.svg)](https://codecov.io/gh/akashgurava/servir)

Rust utility library providing common HTTP server infrastructure for self-hosted consumer servers. Handles routing boilerplate, middleware, structured error responses, observability, and user authentication so consumers can focus on business logic.

## What it provides

| Export | Description |
|---|---|
| `Servir` | Builder-pattern entry point — configure, build, and serve |
| `ServerBuilder` | Fluent builder for `Servir` (returned by `Servir::builder()`) |
| `ApiResponse<T>` | Discriminated JSON envelope: `{"status":"ok","data":...}` / `{"status":"error","error":...}` |
| `ServirError` | Structured error type with HTTP mapping isolated from business logic |
| `AppJson<T>` | Drop-in replacement for `axum::Json` that preserves the error envelope on parse failures |
| `init_tracing(service_name)` | Opinionated tracing-subscriber setup (feature: `telemetry`) |
| `AuthUser` | Axum extractor — validates `Authorization: Bearer` and injects `Claims` (feature: `auth`) |
| `Claims` | JWT payload — `sub()` (user ID) and `username()` (feature: `auth`) |

## Quick start

```rust
use std::net::SocketAddr;
use axum::{Router, routing::get};
use servir::{ApiResponse, Servir};

async fn hello() -> ApiResponse<&'static str> {
    ApiResponse::ok("hello")
}

#[tokio::main]
async fn main() {
    let routes = Router::new().route("/hello", get(hello));

    Servir::builder()
        .service_name("my-app")
        .addr(SocketAddr::from(([0, 0, 0, 0], 3000)))
        .auth_database_url("sqlite://./data/db/auth.db")
        .routes(routes)
        .serve()
        .await
        .expect("server failed");
}
```

The builder handles: tracing initialization, auth database setup, mounting auth routes, applying the auth layer, `/health` endpoint, 404 fallback, and standard middleware (request IDs + tracing). All application routes are nested under `/api/v1`.

## Auth routes (feature: `auth`)

When the `auth` feature is enabled, the following routes are mounted automatically:

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| POST | `/api/v1/auth/register` | — | Create credentials, return access + refresh tokens |
| POST | `/api/v1/auth/login` | — | Verify credentials, return tokens |
| POST | `/api/v1/auth/refresh` | — | Rotate refresh token, return new pair |
| POST | `/api/v1/auth/logout` | refresh token in body | Revoke all refresh tokens for the user |
| GET | `/api/v1/auth/me` | Bearer access | Return `{user_id, username}` from token claims |

The JWT signing secret is auto-generated on first run and persisted in the auth SQLite database. No manual secret configuration required.

## Builder options

| Method | Required | Description |
|--------|----------|-------------|
| `.service_name(name)` | no | Service name for tracing spans and log filtering (default: `"servir"`) |
| `.addr(socket_addr)` | yes | Socket address to bind on (e.g. `SocketAddr::from(([0, 0, 0, 0], 3000))`) |
| `.routes(router)` | yes | Application routes (must have state applied via `.with_state()`) |
| `.auth_database_url(url)` | yes | Auth database URL (e.g. `sqlite://./data/db/auth.db`) |
| `.access_token_ttl_secs(secs)` | no | Access token lifetime in seconds (default: 900 / 15 min) |
| `.refresh_token_ttl_secs(secs)` | no | Refresh token lifetime in seconds (default: 604800 / 7 days) |

## Building a consumer

See [`consumer/README.md`](consumer/README.md) for how to copy the starter template and build a new server on top of this library.

## Features

| Feature | Default | Description |
|---|---|---|
| `telemetry` | on | Enables `init_tracing` via `tracing-subscriber`. Disable if you configure tracing yourself. |
| `auth` | on | Full credential management: Argon2id hashing, HS256 JWT, refresh token rotation, auth routes and middleware. |
