# servir

Rust utility library providing common HTTP server infrastructure for self-hosted consumer servers. Handles routing boilerplate, middleware, structured error responses, and observability so consumers can focus on business logic.

## What it provides

| Export | Description |
|---|---|
| `start_server` / `start_server_from_env` | Bind and serve with standard middleware applied |
| `ApiResponse<T>` | Discriminated JSON envelope: `{"status":"ok","data":...}` / `{"status":"error","error":...}` |
| `ServirError` | Structured error type with HTTP mapping isolated from business logic |
| `standard_middleware` | Request ID generation + propagation + tracing |
| `init_tracing(service_name)` | Opinionated tracing-subscriber setup (feature: `telemetry`, on by default) |

## Building a consumer

See [`consumer/README.md`](consumer/README.md) for how to copy the starter template and build a new server on top of this library.

## Features

| Feature | Default | Description |
|---|---|---|
| `telemetry` | on | Enables `init_tracing` via `tracing-subscriber`. Disable if you configure tracing yourself. |
