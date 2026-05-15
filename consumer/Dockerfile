# syntax=docker/dockerfile:1
#
# Build context: workspace root when developing inside the servir workspace.
# When extracted to a standalone repo (servir dep changed to a published version),
# the build context is the project root.
#
# Workspace build:
#   docker build -f consumer/Dockerfile -t echo-server .
#
# Standalone build:
#   docker build -t echo-server .
#
# Run:
#   docker run -e ECHO_SERVER_PORT=8080 -p 8080:8080 echo-server
#   docker run -p 3000:3000 echo-server

# ── Stage 1: planner ──────────────────────────────────────────────────────────
# cargo-chef computes the minimal dependency set so the cook stage can cache
# dependency compilation independently of source changes.
FROM lukemathwalker/cargo-chef:latest-rust-1-slim AS planner
WORKDIR /app
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 2: builder ──────────────────────────────────────────────────────────
FROM lukemathwalker/cargo-chef:latest-rust-1-slim AS builder
WORKDIR /app

# Restore cached dependencies. This layer is only invalidated when
# Cargo.toml or Cargo.lock change — source edits do not bust it.
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Build the binary. Only this layer reruns on source-only changes.
COPY . .
RUN cargo build --release -p echo-server

# ── Stage 3: runtime ──────────────────────────────────────────────────────────
# distroless/cc contains glibc + libgcc — sufficient for standard Rust binaries.
# No shell is present, which eliminates shell-injection attack surface.
#
# For interactive debugging, swap to: debian:bookworm-slim
FROM gcr.io/distroless/cc-debian12 AS runtime

COPY --from=builder /app/target/release/echo-server /usr/local/bin/server

ENV ECHO_SERVER_PORT=3000
EXPOSE 3000

ENTRYPOINT ["/usr/local/bin/server"]
