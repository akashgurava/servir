# syntax=docker/dockerfile:1
#
# Multi-stage build template for servir-based servers.
# Copy this file to your project root and set BINARY_NAME to your binary.
#
# Build:
#   docker build --build-arg BINARY_NAME=my-server -t my-image .
#
# Run:
#   docker run -p 3000:3000 my-image
#
# The bind address is configured in code via Servir::builder().addr(...).

# ── Stage 1: planner ──────────────────────────────────────────────────────────
# cargo-chef computes the minimal dependency set so the next stage can cache
# dependency compilation independently from source changes.
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

# Build the actual binary.
COPY . .
ARG BINARY_NAME
RUN cargo build --release --bin ${BINARY_NAME}

# ── Stage 3: runtime ──────────────────────────────────────────────────────────
# distroless/cc contains glibc + libgcc — sufficient for standard Rust binaries
# (dynamically linked, default target). No shell is present, which eliminates
# shell-injection attack surface.
#
# For interactive debugging, swap to: debian:bookworm-slim
FROM gcr.io/distroless/cc-debian12 AS runtime

ARG BINARY_NAME
COPY --from=builder /app/target/release/${BINARY_NAME} /usr/local/bin/server

EXPOSE 3000

ENTRYPOINT ["/usr/local/bin/server"]
