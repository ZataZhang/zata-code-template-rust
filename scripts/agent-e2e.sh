#!/usr/bin/env bash
# Agent 服务真实端到端：启动测试中间件、accounts、agent-service、agent-tracing、
# 三个前端与一个仅模拟外部端点的 A2A 协议替身，然后用真实浏览器/HTTP 跑场景。
#
# 用法：
#   just agent-e2e
#   just agent-e2e --scenario organization-runtime-session-lifecycle
#   just agent-e2e --scenario authorization-and-redaction
#   just agent-e2e --scenario tracing-outage-recovery
#
# 可选环境变量：
#   SHOT_DIR                截图目录（透传给场景脚本）
#   AGENT_E2E_REAL_RUNTIME  设为 1 时额外验证真实部署的 A2A Runtime；
#                           还需 AGENT_E2E_REAL_RUNTIME_URL 与 AGENT_E2E_REAL_RUNTIME_SOURCE，
#                           可选 AGENT_E2E_REAL_RUNTIME_VERSION
#                           与 AGENT_E2E_REAL_RUNTIME_CREDENTIAL_ALIAS
#   CHROME_PATH             透传给 Playwright
#
# 所有数据库与存储都使用 docker-compose.testing.yml 的独立实例；不会触碰生产系统，
# 也不会把 token 写进仓库文件或日志。
set -euo pipefail
cd "$(dirname "$0")/.."

SCENARIO=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --scenario)
      SCENARIO="${2:-}"
      [ -n "$SCENARIO" ] || { echo "--scenario 缺少取值" >&2; exit 2; }
      shift 2
      ;;
    --scenario=*)
      SCENARIO="${1#--scenario=}"
      shift
      ;;
    *)
      echo "未知参数：$1" >&2
      exit 2
      ;;
  esac
done

case "$SCENARIO" in
  ""|organization-runtime-session-lifecycle|authorization-and-redaction|tracing-outage-recovery|business-service-sample) ;;
  *)
    echo "未知场景：$SCENARIO" >&2
    echo "可用：organization-runtime-session-lifecycle、authorization-and-redaction、tracing-outage-recovery、business-service-sample" >&2
    exit 2
    ;;
esac

free_port() {
  python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'
}
port_in_use() { lsof -nP -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1; }

ACCOUNTS_PORT="${ACCOUNTS_PORT:-$(free_port)}"
ALT_ACCOUNTS_PORT="${ALT_ACCOUNTS_PORT:-$(free_port)}"
AGENT_PORT="${AGENT_PORT:-$(free_port)}"
TRACING_PORT="${TRACING_PORT:-$(free_port)}"
WEB_PORT="${WEB_PORT:-$(free_port)}"
ADMIN_PORT="${ADMIN_PORT:-$(free_port)}"
TRACING_UI_PORT="${TRACING_UI_PORT:-$(free_port)}"
A2A_PORT="${A2A_PORT:-$(free_port)}"
SAMPLE_PORT="${SAMPLE_PORT:-$(free_port)}"

for port in "$ACCOUNTS_PORT" "$ALT_ACCOUNTS_PORT" "$AGENT_PORT" "$TRACING_PORT" "$WEB_PORT" "$ADMIN_PORT" "$TRACING_UI_PORT" "$A2A_PORT" "$SAMPLE_PORT"; do
  if port_in_use "$port"; then
    echo "端口 $port 已被占用；可通过 ACCOUNTS_PORT / ALT_ACCOUNTS_PORT / AGENT_PORT / TRACING_PORT / WEB_PORT / ADMIN_PORT / TRACING_UI_PORT / A2A_PORT / SAMPLE_PORT 覆盖" >&2
    exit 2
  fi
done

COMPOSE=(docker compose -f docker-compose.testing.yml)
STAMP="$(date +%s)-$$"
ADMIN_USERNAME="agent-e2e-admin-$STAMP"
ADMIN_PASSWORD="agent-e2e-admin-pass-$STAMP"
ALT_ADMIN_USERNAME="agent-e2e-alt-admin-$STAMP"
ALT_ADMIN_PASSWORD="agent-e2e-alt-admin-pass-$STAMP"
MEMBER_EMAIL="agent-e2e-member-$STAMP@example.test"
MEMBER_PASSWORD="agent-e2e-member-pass-$STAMP"
OTHER_EMAIL="agent-e2e-other-$STAMP@example.test"
OTHER_PASSWORD="agent-e2e-other-pass-$STAMP"
REDIS_DB=7

OPENSSL_BIN="${OPENSSL_BIN:-openssl}"
WEB_SERVICE_TOKEN="$("$OPENSSL_BIN" rand -hex 24)"
ADMIN_SERVICE_TOKEN="$("$OPENSSL_BIN" rand -hex 24)"
DIRECT_SERVICE_TOKEN="$("$OPENSSL_BIN" rand -hex 24)"
SAMPLE_SERVICE_TOKEN="$("$OPENSSL_BIN" rand -hex 24)"
PRODUCER_TOKEN="$("$OPENSSL_BIN" rand -hex 24)"
FIXTURE_TOKEN="$("$OPENSSL_BIN" rand -hex 24)"
AGENT_DATABASE_URL="postgres://agent:agentpass@127.0.0.1:5435/agent_dev"
TRACING_DATABASE_URL="postgres://agent_tracing:tracepass@127.0.0.1:5434/agent_tracing_dev"

REPO_ROOT="$(pwd)"
LOG_DIR="$(mktemp -d)"
BLOBS_DIR="$LOG_DIR/blobs"
AGENT_LOG_FILE="$LOG_DIR/agent.log"
TRACING_LOG_FILE="$LOG_DIR/tracing.log"
TRACING_PID_FILE="$LOG_DIR/tracing.pid"
TRACING_START_SCRIPT="$LOG_DIR/start-tracing.sh"
PUBLIC_LOG_FILE="$LOG_DIR/public.log"
ADMIN_LOG_FILE="$LOG_DIR/admin.log"
TRACING_UI_LOG_FILE="$LOG_DIR/tracing-ui.log"
FIXTURE_LOG_FILE="$LOG_DIR/a2a-fixture.log"
SAMPLE_LOG_FILE="$LOG_DIR/sample-business.log"
ACCOUNTS_LOG_FILE="$LOG_DIR/accounts.log"

ACCOUNTS_PID=""
ALT_ACCOUNTS_PID=""
AGENT_PID=""
PUBLIC_PID=""
ADMIN_PID=""
TRACING_UI_PID=""
FIXTURE_PID=""
SAMPLE_PID=""

cleanup() {
  local exit_code=$?
  for pid in "$PUBLIC_PID" "$ADMIN_PID" "$TRACING_UI_PID" "$FIXTURE_PID" "$SAMPLE_PID" "$AGENT_PID" "$ALT_ACCOUNTS_PID" "$ACCOUNTS_PID"; do
    [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
  done
  if [ -f "$TRACING_PID_FILE" ]; then
    kill "$(cat "$TRACING_PID_FILE")" 2>/dev/null || true
  fi
  "${COMPOSE[@]}" exec -T redis redis-cli -a redis123 -n "$REDIS_DB" FLUSHDB >/dev/null 2>&1 || true
  "${COMPOSE[@]}" exec -T postgres psql -U testuser -d testdb -tAc \
    "DELETE FROM users WHERE email LIKE 'agent-e2e-%@example.test'; DELETE FROM admin_users WHERE username LIKE 'agent-e2e-admin-%' OR username LIKE 'agent-e2e-alt-admin-%'" >/dev/null 2>&1 || true
  if [ "$exit_code" -ne 0 ]; then
    echo "--- 失败。服务日志（末尾）：" >&2
    for log_file in "$ACCOUNTS_LOG_FILE" "$AGENT_LOG_FILE" "$TRACING_LOG_FILE" "$PUBLIC_LOG_FILE" "$ADMIN_LOG_FILE" "$TRACING_UI_LOG_FILE" "$FIXTURE_LOG_FILE" "$SAMPLE_LOG_FILE"; do
      if [ -f "$log_file" ]; then
        echo "--- $log_file" >&2
        tail -n 20 "$log_file" >&2 || true
      fi
    done
    echo "--- 完整日志保留在 $LOG_DIR" >&2
    rm -rf "$BLOBS_DIR"
    exit "$exit_code"
  fi
  rm -rf "$LOG_DIR"
  exit 0
}
trap cleanup EXIT INT TERM

echo "==> 启动测试中间件（PostgreSQL 5433/5434/5435，Redis 6390）"
"${COMPOSE[@]}" up -d --wait >/dev/null

echo "==> 重置 Agent 与 tracing 测试库 schema，清理 Redis $REDIS_DB"
"${COMPOSE[@]}" exec -T agent-postgres psql -U agent -d agent_dev -v ON_ERROR_STOP=1 \
  -c 'DROP SCHEMA IF EXISTS public CASCADE; CREATE SCHEMA public;' >/dev/null
"${COMPOSE[@]}" exec -T tracing-postgres psql -U agent_tracing -d agent_tracing_dev -v ON_ERROR_STOP=1 \
  -c 'DROP SCHEMA IF EXISTS public CASCADE; CREATE SCHEMA public;' >/dev/null
"${COMPOSE[@]}" exec -T redis redis-cli -a redis123 -n "$REDIS_DB" FLUSHDB >/dev/null 2>&1
"${COMPOSE[@]}" exec -T postgres psql -U testuser -d testdb -tAc \
  "DELETE FROM users WHERE email LIKE 'agent-e2e-%@example.test'; DELETE FROM admin_users WHERE username LIKE 'agent-e2e-admin-%' OR username LIKE 'agent-e2e-alt-admin-%'" >/dev/null

echo "==> 构建 Rust 服务（accounts、agent、agent-tracing、业务微服务示例）"
cargo build -p tpl-accounts --bin accounts-service --quiet
cargo build -p tpl-agent --bin agent-service --quiet
cargo build -p tpl-agent-tracing --bin agent-tracing-service --quiet
cargo build -p agent-business-service --bin agent-business-service --quiet

echo "==> 启动 accounts-service（端口 ${ACCOUNTS_PORT}）"
DATABASE_URL="postgres://testuser:testpass@127.0.0.1:5433/testdb" \
REDIS_URL="redis://:redis123@127.0.0.1:6390/$REDIS_DB" \
APP_SERVER__PORT="$ACCOUNTS_PORT" \
ADMIN_BOOTSTRAP_USERNAME="$ADMIN_USERNAME" \
ADMIN_BOOTSTRAP_PASSWORD="$ADMIN_PASSWORD" \
  ./target/debug/accounts-service >"$ACCOUNTS_LOG_FILE" 2>&1 &
ACCOUNTS_PID=$!

for _ in $(seq 1 120); do
  if curl -fs -o /dev/null "http://127.0.0.1:$ACCOUNTS_PORT/live"; then break; fi
  sleep 0.3
done
curl -fs -o /dev/null "http://127.0.0.1:$ACCOUNTS_PORT/live" || { echo "accounts-service 没有在预期时间内就绪" >&2; exit 1; }

echo "==> 解析平台管理员 ID"
ADMIN_LOGIN_JSON="$(curl -fsS -X POST "http://127.0.0.1:$ACCOUNTS_PORT/admin/auth/login" \
  -H 'content-type: application/json' \
  -d "{\"identifier\":\"$ADMIN_USERNAME\",\"password\":\"$ADMIN_PASSWORD\"}")"
ADMIN_USER_ID="$(printf '%s' "$ADMIN_LOGIN_JSON" | python3 -c 'import json,sys; print(json.load(sys.stdin)["user_id"])')"
[ -n "$ADMIN_USER_ID" ] || { echo "无法解析管理员 ID" >&2; exit 1; }

echo "==> 启动第二个 accounts 实例（引导非平台管理员 ${ALT_ADMIN_USERNAME}，用于组织 scope 隔离）"
DATABASE_URL="postgres://testuser:testpass@127.0.0.1:5433/testdb" \
REDIS_URL="redis://:redis123@127.0.0.1:6390/$REDIS_DB" \
APP_SERVER__PORT="$ALT_ACCOUNTS_PORT" \
ADMIN_BOOTSTRAP_USERNAME="$ALT_ADMIN_USERNAME" \
ADMIN_BOOTSTRAP_PASSWORD="$ALT_ADMIN_PASSWORD" \
  ./target/debug/accounts-service >"$LOG_DIR/accounts-alt.log" 2>&1 &
ALT_ACCOUNTS_PID=$!
for _ in $(seq 1 120); do
  if curl -fs -o /dev/null "http://127.0.0.1:$ALT_ACCOUNTS_PORT/live"; then break; fi
  sleep 0.3
done
curl -fs -o /dev/null "http://127.0.0.1:$ALT_ACCOUNTS_PORT/live" || { echo "第二个 accounts-service 没有在预期时间内就绪" >&2; exit 1; }

ALT_ADMIN_LOGIN_JSON="$(curl -fsS -X POST "http://127.0.0.1:$ALT_ACCOUNTS_PORT/admin/auth/login" \
  -H 'content-type: application/json' \
  -d "{\"identifier\":\"$ALT_ADMIN_USERNAME\",\"password\":\"$ALT_ADMIN_PASSWORD\"}")"
ALT_ADMIN_USER_ID="$(printf '%s' "$ALT_ADMIN_LOGIN_JSON" | python3 -c 'import json,sys; print(json.load(sys.stdin)["user_id"])')"
[ -n "$ALT_ADMIN_USER_ID" ] || { echo "无法解析非平台管理员 ID" >&2; exit 1; }
[ "$ALT_ADMIN_USER_ID" != "$ADMIN_USER_ID" ] || { echo "非平台管理员与平台管理员 ID 相同" >&2; exit 1; }

cat >"$TRACING_START_SCRIPT" <<EOF
#!/usr/bin/env bash
set -euo pipefail
cd "$REPO_ROOT"
export APP_CONFIG_FILE="$REPO_ROOT/services/agent-tracing/config.toml"
export APP_SERVER__HOST=127.0.0.1
export APP_SERVER__PORT=$TRACING_PORT
export APP_DATABASE__URL="$TRACING_DATABASE_URL"
export APP_ACCOUNTS__ADMIN_AUTH_URL="http://127.0.0.1:$ACCOUNTS_PORT/admin/auth/me"
export APP_PRODUCER__CREDENTIALS_JSON='[{"source_id":"agent-service","project_id":"agent-runtime","token":"$PRODUCER_TOKEN"}]'
exec "$REPO_ROOT/target/debug/agent-tracing-service"
EOF
chmod +x "$TRACING_START_SCRIPT"

echo "==> 启动 agent-tracing-service（端口 ${TRACING_PORT}）"
"$TRACING_START_SCRIPT" >"$TRACING_LOG_FILE" 2>&1 &
echo $! >"$TRACING_PID_FILE"

echo "==> 启动 agent-service（端口 ${AGENT_PORT}，tracing ingest 指向 ${TRACING_PORT}）"
DATABASE_URL="$AGENT_DATABASE_URL" \
APP_AGENT__SERVER__HOST=127.0.0.1 \
APP_AGENT__SERVER__PORT="$AGENT_PORT" \
APP_AGENT__IDENTITY__USER_AUTH_URL="http://127.0.0.1:$ACCOUNTS_PORT/auth/me" \
APP_AGENT__IDENTITY__ADMIN_AUTH_URL="http://127.0.0.1:$ACCOUNTS_PORT/admin/auth/me" \
APP_AGENT__STORAGE__ROOT="$BLOBS_DIR" \
APP_AGENT__PLATFORM_ADMIN_IDS="[$ADMIN_USER_ID]" \
APP_AGENT__TRACING__INGEST_URL="http://127.0.0.1:$TRACING_PORT/api/v1/ingest" \
APP_AGENT__TRACING__POLL_INTERVAL_MS=200 \
AGENT_TRACING_PRODUCER_TOKEN="$PRODUCER_TOKEN" \
AGENT_CREDENTIAL_FIXTURE="$FIXTURE_TOKEN" \
AGENT_SERVICE_TOKENS_JSON="[{\"service_id\":\"web\",\"token\":\"$WEB_SERVICE_TOKEN\"},{\"service_id\":\"admin\",\"token\":\"$ADMIN_SERVICE_TOKEN\"},{\"service_id\":\"e2e-direct\",\"token\":\"$DIRECT_SERVICE_TOKEN\"},{\"service_id\":\"sample-business\",\"token\":\"$SAMPLE_SERVICE_TOKEN\"}]" \
  ./target/debug/agent-service >"$AGENT_LOG_FILE" 2>&1 &
AGENT_PID=$!

echo "==> 启动 A2A 协议替身（端口 ${A2A_PORT}，仅外部端点）"
A2A_FIXTURE_PORT="$A2A_PORT" A2A_FIXTURE_TOKEN="$FIXTURE_TOKEN" \
  node scripts/agent-e2e/a2a-fixture.mjs >"$FIXTURE_LOG_FILE" 2>&1 &
FIXTURE_PID=$!

echo "==> 启动业务微服务示例（端口 ${SAMPLE_PORT}，仅调用 Agent HTTP 契约）"
BUSINESS_SERVICE_BIND="127.0.0.1:$SAMPLE_PORT" \
AGENT_BASE_URL="http://127.0.0.1:$AGENT_PORT" \
AGENT_SERVICE_TOKEN="$SAMPLE_SERVICE_TOKEN" \
  ./target/debug/agent-business-service >"$SAMPLE_LOG_FILE" 2>&1 &
SAMPLE_PID=$!

for _ in $(seq 1 160); do
  accounts_ready=0
  agent_ready=0
  tracing_ready=0
  fixture_ready=0
  sample_ready=0
  curl -fs -o /dev/null "http://127.0.0.1:$ACCOUNTS_PORT/live" && accounts_ready=1
  curl -fs -o /dev/null "http://127.0.0.1:$AGENT_PORT/live" && agent_ready=1
  curl -fs -o /dev/null "http://127.0.0.1:$TRACING_PORT/live" && tracing_ready=1
  curl -fs -o /dev/null "http://127.0.0.1:$A2A_PORT/_state" && fixture_ready=1
  curl -fs -o /dev/null "http://127.0.0.1:$SAMPLE_PORT/healthz" && sample_ready=1
  if [ "$accounts_ready" = 1 ] && [ "$agent_ready" = 1 ] && [ "$tracing_ready" = 1 ] && [ "$fixture_ready" = 1 ] && [ "$sample_ready" = 1 ]; then break; fi
  sleep 0.3
done
curl -fs -o /dev/null "http://127.0.0.1:$AGENT_PORT/ready" || { echo "agent-service 没有在预期时间内就绪" >&2; exit 1; }
curl -fs -o /dev/null "http://127.0.0.1:$TRACING_PORT/ready" || { echo "agent-tracing-service 没有在预期时间内就绪" >&2; exit 1; }
curl -fs -o /dev/null "http://127.0.0.1:$A2A_PORT/_state" || { echo "A2A 替身没有在预期时间内就绪" >&2; exit 1; }
curl -fs -o /dev/null "http://127.0.0.1:$SAMPLE_PORT/healthz" || { echo "业务微服务示例没有在预期时间内就绪" >&2; exit 1; }

echo "==> 构建公开站点（代理到 accounts ${ACCOUNTS_PORT}）"
( cd frontends/public && BACKEND_URL="http://127.0.0.1:$ACCOUNTS_PORT" pnpm build >"$LOG_DIR/public-build.log" 2>&1 ) \
  || { cat "$LOG_DIR/public-build.log" >&2; exit 1; }
# next start 不支持 output: standalone；按 Dockerfile 相同方式运行 standalone 产物，
# 并链接构建时未打进 standalone 的静态资源。
rm -rf frontends/public/.next/standalone/public/.next/static frontends/public/.next/standalone/public/public
ln -s "$(pwd)/frontends/public/.next/static" frontends/public/.next/standalone/public/.next/static
ln -s "$(pwd)/frontends/public/public" frontends/public/.next/standalone/public/public
( cd frontends/public && AGENT_SERVICE_URL="http://127.0.0.1:$AGENT_PORT" AGENT_SERVICE_TOKEN="$WEB_SERVICE_TOKEN" \
  PORT="$WEB_PORT" HOSTNAME=127.0.0.1 exec node .next/standalone/public/server.js ) >"$PUBLIC_LOG_FILE" 2>&1 &
PUBLIC_PID=$!

echo "==> 构建管理后台（/api → accounts，/agent-api → agent）"
( cd frontends/admin && pnpm build >"$LOG_DIR/admin-build.log" 2>&1 ) \
  || { cat "$LOG_DIR/admin-build.log" >&2; exit 1; }
( cd frontends/admin && BACKEND_PORT="$ACCOUNTS_PORT" \
  AGENT_SERVICE_URL="http://127.0.0.1:$AGENT_PORT" AGENT_SERVICE_TOKEN="$ADMIN_SERVICE_TOKEN" \
  exec pnpm exec vite preview --host 127.0.0.1 --port "$ADMIN_PORT" --strictPort ) >"$ADMIN_LOG_FILE" 2>&1 &
ADMIN_PID=$!

echo "==> 构建 tracing UI（/api → tracing，/api/admin/auth → accounts）"
( cd frontends/tracing && pnpm build >"$LOG_DIR/tracing-ui-build.log" 2>&1 ) \
  || { cat "$LOG_DIR/tracing-ui-build.log" >&2; exit 1; }
( cd frontends/tracing && AGENT_TRACING_PORT="$TRACING_PORT" ACCOUNTS_PORT="$ACCOUNTS_PORT" \
  FRONTEND_TRACING_PORT="$TRACING_UI_PORT" \
  exec pnpm exec vite preview --host 127.0.0.1 --port "$TRACING_UI_PORT" --strictPort ) >"$TRACING_UI_LOG_FILE" 2>&1 &
TRACING_UI_PID=$!

for _ in $(seq 1 200); do
  public_ready=0
  admin_ready=0
  tracing_ui_ready=0
  curl -fs -o /dev/null "http://127.0.0.1:$WEB_PORT/login" && public_ready=1
  curl -fs -o /dev/null "http://127.0.0.1:$ADMIN_PORT/sign-in" && admin_ready=1
  curl -fs -o /dev/null "http://127.0.0.1:$TRACING_UI_PORT/" && tracing_ui_ready=1
  if [ "$public_ready" = 1 ] && [ "$admin_ready" = 1 ] && [ "$tracing_ui_ready" = 1 ]; then break; fi
  sleep 0.5
done
curl -fs -o /dev/null "http://127.0.0.1:$WEB_PORT/login" || { echo "公开站点没有在预期时间内就绪" >&2; exit 1; }
curl -fs -o /dev/null "http://127.0.0.1:$ADMIN_PORT/sign-in" || { echo "管理后台没有在预期时间内就绪" >&2; exit 1; }
curl -fs -o /dev/null "http://127.0.0.1:$TRACING_UI_PORT/" || { echo "tracing UI 没有在预期时间内就绪" >&2; exit 1; }

echo "==> 通过公开站点注册两个成员账号"
register_user() {
  local email=$1 password=$2
  curl -fsS -X POST "http://127.0.0.1:$WEB_PORT/api/auth/register" \
    -H 'content-type: application/json' \
    -d "{\"email\":\"$email\",\"password\":\"$password\"}"
}
MEMBER_USER_ID="$(register_user "$MEMBER_EMAIL" "$MEMBER_PASSWORD" | python3 -c 'import json,sys; print(json.load(sys.stdin)["user"]["id"])')"
OTHER_USER_ID="$(register_user "$OTHER_EMAIL" "$OTHER_PASSWORD" | python3 -c 'import json,sys; print(json.load(sys.stdin)["user"]["id"])')"
[ -n "$MEMBER_USER_ID" ] && [ -n "$OTHER_USER_ID" ] || { echo "无法解析成员 ID" >&2; exit 1; }

export REPO_ROOT
export PUBLIC_URL="http://127.0.0.1:$WEB_PORT"
export ADMIN_URL="http://127.0.0.1:$ADMIN_PORT"
export TRACING_UI_URL="http://127.0.0.1:$TRACING_UI_PORT"
export AGENT_SERVICE_URL="http://127.0.0.1:$AGENT_PORT"
export TRACING_API_URL="http://127.0.0.1:$TRACING_PORT"
export AGENT_SERVICE_TOKEN="$DIRECT_SERVICE_TOKEN"
export AGENT_WEB_SERVICE_TOKEN="$WEB_SERVICE_TOKEN"
export AGENT_ADMIN_SERVICE_TOKEN="$ADMIN_SERVICE_TOKEN"
export PRODUCER_TOKEN
export A2A_ENDPOINT="http://127.0.0.1:$A2A_PORT"
export A2A_STATE_URL="http://127.0.0.1:$A2A_PORT/_state"
export A2A_CREDENTIAL_ALIAS="fixture"
export SAMPLE_SERVICE_URL="http://127.0.0.1:$SAMPLE_PORT"
export SAMPLE_SERVICE_TOKEN
export ADMIN_USERNAME ADMIN_PASSWORD ADMIN_USER_ID
export ALT_ADMIN_USERNAME ALT_ADMIN_PASSWORD ALT_ADMIN_USER_ID
export ALT_ACCOUNTS_URL="http://127.0.0.1:$ALT_ACCOUNTS_PORT"
export MEMBER_EMAIL MEMBER_PASSWORD MEMBER_USER_ID
export OTHER_EMAIL OTHER_PASSWORD OTHER_USER_ID
export AGENT_LOG_FILE TRACING_LOG_FILE
export TRACING_PID_FILE TRACING_START_SCRIPT
export AGENT_DATABASE_URL TRACING_DATABASE_URL
export SCENARIO_STAMP="$STAMP"
export REAL_RUNTIME_ENABLED="${AGENT_E2E_REAL_RUNTIME:-0}"
export REAL_RUNTIME_URL="${AGENT_E2E_REAL_RUNTIME_URL:-}"
export REAL_RUNTIME_VERSION="${AGENT_E2E_REAL_RUNTIME_VERSION:-unknown}"
export REAL_RUNTIME_CREDENTIAL_ALIAS="${AGENT_E2E_REAL_RUNTIME_CREDENTIAL_ALIAS:-}"
export REAL_RUNTIME_SOURCE="${AGENT_E2E_REAL_RUNTIME_SOURCE:-}"
if [ "$REAL_RUNTIME_ENABLED" = "1" ]; then
  [ -n "$REAL_RUNTIME_URL" ] || { echo "AGENT_E2E_REAL_RUNTIME=1 时必须设置 AGENT_E2E_REAL_RUNTIME_URL" >&2; exit 2; }
  [ -n "$REAL_RUNTIME_SOURCE" ] || { echo "AGENT_E2E_REAL_RUNTIME=1 时必须设置 AGENT_E2E_REAL_RUNTIME_SOURCE（Runtime 实现与配置来源）" >&2; exit 2; }
  echo "==> 真实部署 Runtime：类型=a2a 版本=${REAL_RUNTIME_VERSION} 来源=${REAL_RUNTIME_SOURCE}（额外 smoke）"
else
  echo "==> 真实部署 Runtime：not run（设置 AGENT_E2E_REAL_RUNTIME=1 才会执行）"
fi

if [ -n "${SHOT_DIR:-}" ]; then
  mkdir -p "$SHOT_DIR"
  SHOT_DIR="$(cd "$SHOT_DIR" && pwd)"
  export SHOT_DIR
fi

run_scenario() {
  local scenario_name=$1
  local script_name
  case "$scenario_name" in
    organization-runtime-session-lifecycle) script_name="agent-rv1.mjs" ;;
    authorization-and-redaction) script_name="agent-rv2.mjs" ;;
    tracing-outage-recovery) script_name="agent-rv3.mjs" ;;
    business-service-sample) script_name="agent-rv5.mjs" ;;
    *) echo "未知场景：$scenario_name" >&2; exit 2 ;;
  esac
  echo "==> 运行场景：$scenario_name"
  ( cd frontends/admin && node "e2e/$script_name" )
}

if [ -z "$SCENARIO" ]; then
  run_scenario organization-runtime-session-lifecycle
  run_scenario authorization-and-redaction
  run_scenario tracing-outage-recovery
else
  run_scenario "$SCENARIO"
fi

echo "==> Agent 端到端场景通过"
