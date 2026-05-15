# consumer

Starter template for a `servir`-based server. Copy this directory to bootstrap a new consumer.

---

## Creating a new consumer

1. Copy this directory to a new repository.
2. In `Cargo.toml`: rename `name`, update `[[bin]]` name to match, change `servir = { path = ".." }` to the published version.
3. In `docker-compose.yml`: change `context: ..` to `context: .`.
4. Rename `ECHO_SERVER_PORT` to `<YOUR_SERVER>_PORT` throughout.
5. Replace routes and state in `src/main.rs` with your own.

---

## Docker

### Consumer copy (standalone repo)

After completing the setup steps above (published `servir` dep, `context: .` in compose):

```bash
# Build
docker build -t my-server .

# Run
docker run -p 3000:3000 my-server
docker run -e MY_SERVER_PORT=8080 -p 8080:8080 my-server

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
docker run -e ECHO_SERVER_PORT=8080 -p 8080:8080 my-server

# Compose
docker compose -f consumer/docker-compose.yml up -d
docker compose -f consumer/docker-compose.yml down
```
