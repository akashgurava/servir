#!/usr/bin/env bash
set -euo pipefail

BASE="http://localhost:3333/api/v1"
TEST_DIR="/tmp/test_consumer"
CONTAINER="echo-test"
IMAGE="echo-server-test"
PASS=0
FAIL=0

check() {
  local name="$1" expected="$2" actual="$3"
  if [ "$actual" = "$expected" ]; then
    echo "  PASS  $name"
    PASS=$((PASS + 1))
  else
    echo "  FAIL  $name"
    echo "        expected: $expected"
    echo "        got:      $actual"
    FAIL=$((FAIL + 1))
  fi
}

json_field() {
  python3 -c "import sys,json; print(json.load(sys.stdin)$1)"
}

cleanup() {
  echo ""
  echo "Cleaning up..."
  docker stop "$CONTAINER" 2>/dev/null || true
  docker rm "$CONTAINER" 2>/dev/null || true
  rm -rf "$TEST_DIR"
  echo "Done."
}

trap cleanup EXIT

# ─────────────────────────────────────────────────────────────────────────────
echo "╔══════════════════════════════════════╗"
echo "║   echo-server integration tests     ║"
echo "╚══════════════════════════════════════╝"
echo ""

# ─────────────────────────────────────────────────────────────────────────────
echo "▶ Building Docker image..."
docker build -f consumer/Dockerfile -t "$IMAGE" . --quiet
echo "  Image built: $IMAGE"
echo ""

# ─────────────────────────────────────────────────────────────────────────────
echo "▶ Starting container..."
mkdir -p "$TEST_DIR/data/db" "$TEST_DIR/data/logs"
docker run -d \
  --name "$CONTAINER" \
  -p 3333:3000 \
  -v "$TEST_DIR/data:/data" \
  "$IMAGE" \
  --auth-db /data/db/auth.db \
  --data-db /data/db/data.db \
  --log-dir /data/logs \
  > /dev/null
sleep 2
echo "  Container running: $CONTAINER"
echo ""

# ─────────────────────────────────────────────────────────────────────────────
echo "▶ Verifying database files..."
check "auth.db exists" "true" "$([ -f $TEST_DIR/data/db/auth.db ] && echo true || echo false)"
check "data.db exists" "true" "$([ -f $TEST_DIR/data/db/data.db ] && echo true || echo false)"
echo ""

# ─────────────────────────────────────────────────────────────────────────────
echo "▶ Verifying log files..."
LOG_FILE=$(ls "$TEST_DIR/data/logs"/echo-server.log.* 2>/dev/null | head -1)
check "log file created" "true" "$([ -n "$LOG_FILE" ] && echo true || echo false)"
check "log file has JSON content" "true" "$([ -n "$LOG_FILE" ] && head -1 "$LOG_FILE" | python3 -c 'import sys,json; json.load(sys.stdin); print("true")' 2>/dev/null || echo false)"
echo ""

# ─────────────────────────────────────────────────────────────────────────────
echo "▶ Testing routes..."
echo ""

echo "  [health]"
RESP=$(curl -sf "$BASE/health")
check "GET /health returns ok" "ok" "$(echo "$RESP" | json_field "['status']")"
echo ""

echo "  [register]"
RESP=$(curl -sf -X POST "$BASE/auth/register" \
  -H "Content-Type: application/json" \
  -d '{"username":"testuser","password":"testpass123"}')
check "POST /auth/register succeeds" "ok" "$(echo "$RESP" | json_field "['status']")"
ACCESS=$(echo "$RESP" | json_field "['data']['access_token']")
REFRESH=$(echo "$RESP" | json_field "['data']['refresh_token']")
echo ""

echo "  [register duplicate]"
RESP=$(curl -s -X POST "$BASE/auth/register" \
  -H "Content-Type: application/json" \
  -d '{"username":"testuser","password":"testpass123"}')
check "POST /auth/register duplicate rejected" "ALREADY_EXISTS" "$(echo "$RESP" | json_field "['error']['error_id']")"
echo ""

echo "  [login]"
RESP=$(curl -sf -X POST "$BASE/auth/login" \
  -H "Content-Type: application/json" \
  -d '{"username":"testuser","password":"testpass123"}')
check "POST /auth/login succeeds" "ok" "$(echo "$RESP" | json_field "['status']")"
echo ""

echo "  [login wrong password]"
RESP=$(curl -s -X POST "$BASE/auth/login" \
  -H "Content-Type: application/json" \
  -d '{"username":"testuser","password":"wrongpass"}')
check "POST /auth/login wrong password rejected" "INVALID_CREDENTIALS" "$(echo "$RESP" | json_field "['error']['error_id']")"
echo ""

echo "  [me]"
RESP=$(curl -sf "$BASE/auth/me" -H "Authorization: Bearer $ACCESS")
check "GET /auth/me succeeds" "ok" "$(echo "$RESP" | json_field "['status']")"
check "GET /auth/me returns username" "testuser" "$(echo "$RESP" | json_field "['data']['username']")"
echo ""

echo "  [user]"
RESP=$(curl -sf "$BASE/user" -H "Authorization: Bearer $ACCESS")
check "GET /user creates profile" "ok" "$(echo "$RESP" | json_field "['status']")"
check "GET /user returns username" "testuser" "$(echo "$RESP" | json_field "['data']['username']")"
echo ""

echo "  [user no auth]"
RESP=$(curl -s "$BASE/user")
check "GET /user without token returns 401" "MISSING_AUTHORIZATION_HEADER" "$(echo "$RESP" | json_field "['error']['error_id']")"
echo ""

echo "  [refresh]"
RESP=$(curl -sf -X POST "$BASE/auth/refresh" \
  -H "Content-Type: application/json" \
  -d "{\"refresh_token\":\"$REFRESH\"}")
check "POST /auth/refresh succeeds" "ok" "$(echo "$RESP" | json_field "['status']")"
NEW_REFRESH=$(echo "$RESP" | json_field "['data']['refresh_token']")
echo ""

echo "  [refresh reuse]"
RESP=$(curl -s -X POST "$BASE/auth/refresh" \
  -H "Content-Type: application/json" \
  -d "{\"refresh_token\":\"$REFRESH\"}")
check "POST /auth/refresh consumed token rejected" "INVALID_REFRESH_TOKEN" "$(echo "$RESP" | json_field "['error']['error_id']")"
echo ""

echo "  [logout]"
RESP=$(curl -sf -X POST "$BASE/auth/logout" \
  -H "Content-Type: application/json" \
  -d "{\"refresh_token\":\"$NEW_REFRESH\"}")
check "POST /auth/logout succeeds" "ok" "$(echo "$RESP" | json_field "['status']")"
echo ""

echo "  [refresh after logout]"
RESP=$(curl -s -X POST "$BASE/auth/refresh" \
  -H "Content-Type: application/json" \
  -d "{\"refresh_token\":\"$NEW_REFRESH\"}")
check "POST /auth/refresh after logout rejected" "INVALID_REFRESH_TOKEN" "$(echo "$RESP" | json_field "['error']['error_id']")"
echo ""

echo "  [404]"
RESP=$(curl -s "$BASE/nonexistent")
check "GET /nonexistent returns 404" "NOT_FOUND" "$(echo "$RESP" | json_field "['error']['error_id']")"
echo ""

# ─────────────────────────────────────────────────────────────────────────────
echo "╔══════════════════════════════════════╗"
if [ $FAIL -eq 0 ]; then
  echo "║   All $PASS tests passed              ║"
else
  echo "║   $PASS passed, $FAIL failed              ║"
fi
echo "╚══════════════════════════════════════╝"

[ $FAIL -eq 0 ]
