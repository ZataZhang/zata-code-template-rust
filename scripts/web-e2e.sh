#!/usr/bin/env bash
# 前端真实浏览器端到端测试：启动数据库、后端和两个前端，用 Chrome 走完公开站点（注册/登录/退出/限流）
# 与管理后台（登录、搜索/筛选、禁用/启用、语言切换、退出、限流）的完整流程。
#
# 用法：just web-e2e
# 需要：Docker、Node + pnpm、本机装有 Chrome（或用 CHROME_PATH 指定）、已执行过 `just web-install` 与 `just web-admin-install`。
# 可选环境变量：BACKEND_PORT、WEB_PORT、ADMIN_PORT（默认自动挑空闲端口）、SHOT_DIR（保存截图的目录）。
#
# 它会：
#   1. 用独立的 Redis 库（6 号）和自动挑选的空闲端口，不碰 0 号库，也不占用常用端口；
#   2. 把后端的「单账号登录失败上限」（普通用户与管理员各自）调成 3，方便测试限流，并用环境变量引导一个测试管理员；
#   3. 结束时（含失败、Ctrl+C）停掉全部进程、清空 6 号库、删除测试创建的用户与管理员。
set -euo pipefail
cd "$(dirname "$0")/.."

# 向系统要一个空闲端口（绑定 0 号端口再读回）。
free_port() {
  python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'
}
BACKEND_PORT="${BACKEND_PORT:-$(free_port)}"
WEB_PORT="${WEB_PORT:-$(free_port)}"
ADMIN_PORT="${ADMIN_PORT:-$(free_port)}"
ADMIN_USERNAME="browser-admin"
ADMIN_PASSWORD="admin-correct-horse-9"
REDIS_DB=6
COMPOSE=(docker compose -f docker-compose.testing.yml)
LOG_DIR="$(mktemp -d)"
BACKEND_PID=""
WEB_PID=""
ADMIN_WEB_PID=""

port_in_use() { lsof -nP -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1; }
for port in "$BACKEND_PORT" "$WEB_PORT" "$ADMIN_PORT"; do
  if port_in_use "$port"; then
    echo "端口 $port 已被占用，请用 BACKEND_PORT / WEB_PORT / ADMIN_PORT 换一个" >&2
    exit 2
  fi
done

cleanup() {
  local exit_code=$?
  [ -n "$WEB_PID" ] && kill "$WEB_PID" 2>/dev/null || true
  [ -n "$ADMIN_WEB_PID" ] && kill "$ADMIN_WEB_PID" 2>/dev/null || true
  [ -n "$BACKEND_PID" ] && kill "$BACKEND_PID" 2>/dev/null || true
  "${COMPOSE[@]}" exec -T redis redis-cli -a redis123 -n "$REDIS_DB" FLUSHDB >/dev/null 2>&1 || true
  "${COMPOSE[@]}" exec -T postgres psql -U testuser -d testdb -tAc \
    "DELETE FROM users WHERE email LIKE 'browser-%@example.test'; DELETE FROM admin_users WHERE username = '$ADMIN_USERNAME'" >/dev/null 2>&1 || true
  if [ "$exit_code" -ne 0 ]; then
    echo "--- 失败。后端日志（末尾）：" >&2; tail -n 15 "$LOG_DIR/backend.log" 2>/dev/null >&2 || true
    echo "--- 前端日志（末尾）：" >&2; tail -n 15 "$LOG_DIR/web.log" 2>/dev/null >&2 || true
    echo "--- 管理后台日志（末尾）：" >&2; tail -n 15 "$LOG_DIR/admin-web.log" 2>/dev/null >&2 || true
  fi
  rm -rf "$LOG_DIR"
  exit "$exit_code"
}
trap cleanup EXIT INT TERM

echo "==> 启动 PostgreSQL / Redis"
"${COMPOSE[@]}" up -d --wait >/dev/null
"${COMPOSE[@]}" exec -T redis redis-cli -a redis123 -n "$REDIS_DB" FLUSHDB >/dev/null 2>&1

echo "==> 构建并启动后端（端口 $BACKEND_PORT，账号失败上限 3，引导管理员 $ADMIN_USERNAME）"
cargo build -p tpl-app --quiet
DATABASE_URL="postgres://testuser:testpass@localhost:5433/testdb" \
REDIS_URL="redis://:redis123@localhost:6390/$REDIS_DB" \
APP_SERVER__PORT="$BACKEND_PORT" \
APP_AUTH__LOGIN_MAX_FAILURES_PER_ACCOUNT=3 \
APP_ADMIN__LOGIN_MAX_FAILURES_PER_ACCOUNT=3 \
ADMIN_BOOTSTRAP_USERNAME="$ADMIN_USERNAME" \
ADMIN_BOOTSTRAP_PASSWORD="$ADMIN_PASSWORD" \
  ./target/debug/server >"$LOG_DIR/backend.log" 2>&1 &
BACKEND_PID=$!

echo "==> 构建并启动前端（端口 $WEB_PORT，代理到后端 $BACKEND_PORT）"
# 后端地址要在构建时传入：Next 的 rewrites 会在构建时固化。
( cd frontend-public && BACKEND_URL="http://127.0.0.1:$BACKEND_PORT" pnpm build >"$LOG_DIR/web-build.log" 2>&1 ) \
  || { cat "$LOG_DIR/web-build.log" >&2; exit 1; }
( cd frontend-public && exec pnpm exec next start -p "$WEB_PORT" ) >"$LOG_DIR/web.log" 2>&1 &
WEB_PID=$!

echo "==> 构建并启动管理后台（端口 $ADMIN_PORT，/api 代理到后端 $BACKEND_PORT）"
( cd frontend-admin && pnpm build >"$LOG_DIR/admin-build.log" 2>&1 ) \
  || { cat "$LOG_DIR/admin-build.log" >&2; exit 1; }
( cd frontend-admin && BACKEND_PORT="$BACKEND_PORT" exec pnpm exec vite preview --host 127.0.0.1 --port "$ADMIN_PORT" --strictPort ) >"$LOG_DIR/admin-web.log" 2>&1 &
ADMIN_WEB_PID=$!

for _ in $(seq 1 100); do
  if curl -fs -o /dev/null "http://127.0.0.1:$BACKEND_PORT/live" && curl -fs -o /dev/null "http://127.0.0.1:$WEB_PORT/login" && curl -fs -o /dev/null "http://127.0.0.1:$ADMIN_PORT/sign-in"; then
    break
  fi
  sleep 0.3
done
curl -fs -o /dev/null "http://127.0.0.1:$BACKEND_PORT/live" || { echo "后端没有在预期时间内就绪" >&2; exit 1; }
curl -fs -o /dev/null "http://127.0.0.1:$WEB_PORT/login" || { echo "前端没有在预期时间内就绪" >&2; exit 1; }
curl -fs -o /dev/null "http://127.0.0.1:$ADMIN_PORT/sign-in" || { echo "管理后台没有在预期时间内就绪" >&2; exit 1; }

echo "==> 运行浏览器测试：公开站点"
( cd frontend-public && BASE_URL="http://127.0.0.1:$WEB_PORT" node e2e/auth.e2e.mjs )

echo "==> 运行浏览器测试：管理后台"
( cd frontend-admin && \
  BASE_URL="http://127.0.0.1:$ADMIN_PORT" BACKEND_URL="http://127.0.0.1:$BACKEND_PORT" \
  ADMIN_USERNAME="$ADMIN_USERNAME" ADMIN_PASSWORD="$ADMIN_PASSWORD" node e2e/admin.e2e.mjs )
