# consumer

Starter template for a `servir`-based server. Copy this directory to bootstrap a new consumer.

---

## Creating a new consumer

1. Copy this directory to a new repository.
2. In `Cargo.toml`: rename `name`, update `[[bin]]` name to match, change `servir = { path = ".." }` to the published version.
3. In `docker-compose.yml`: change `context: ..` to `context: .`.
4. Replace routes and state in `src/main.rs` with your own.

---

## Configuration

Database paths are resolved in this order: **CLI arg > ENV var > default**.

| CLI arg | Env variable | Default | Description |
|---|---|---|---|
| `--auth-db` | `CONSUMER_AUTH_DB` | `./data/db/auth.db` | Auth DB (servir-managed, auto-created). |
| `--data-db` | `CONSUMER_DATA_DB` | `./data/db/data.db` | App DB (consumer-managed). |

Other environment variables:

| Variable | Default | Description |
|---|---|---|
| `RUST_LOG` | `info` | Tracing filter (e.g. `debug`, `echo_server=trace`). |

The JWT signing secret is auto-generated and stored in the auth database on first run — no manual configuration needed.

The bind address (host + port) is set directly in code via `.addr(SocketAddr)` on the builder.

---

## Usage

```rust
use std::net::SocketAddr;
use axum::{Router, routing::get};
use servir::{ApiResponse, AuthUser, Servir, ServirError};

async fn user(AuthUser(claims): AuthUser) -> Result<ApiResponse<String>, ServirError> {
    Ok(ApiResponse::ok(claims.username().to_string()))
}

#[tokio::main]
async fn main() {
    let routes = Router::new().route("/user", get(user));

    Servir::builder()
        .service_name("my-server")
        .addr(SocketAddr::from(([0, 0, 0, 0], 3000)))
        .routes(routes)
        .serve()
        .await
        .expect("server failed");
}
```

All routes are nested under `/api/v1` automatically. The auth routes (`/api/v1/auth/*`) and `/api/v1/health` are provided by servir.

---

## Docker

### Consumer copy (standalone repo)

After completing the setup steps above (published `servir` dep, `context: .` in compose):

```bash
# Build
docker build -t my-server .

# Run
docker run -p 3000:3000 my-server

# Compose
docker compose up -d
docker compose down
```

### Example (inside the servir workspace)

```bash
# Build — context must be the workspace root to include the servir path dependency
docker build -f consumer/Dockerfile -t my-server .

# Run
docker run -p 3000:3000 my-server

# Compose
docker compose -f consumer/docker-compose.yml up -d
docker compose -f consumer/docker-compose.yml down
```
