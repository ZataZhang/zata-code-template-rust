#!/usr/bin/env bash
# 整套服务（docker-compose.yml）的冒烟测试：对着**正在运行**的 stack 走真实入口，验证
#   1. 前台与管理后台的页面、API 代理都通；
#   2. 注册/登录/停用用户的完整流程，管理员与普通用户的 Cookie 互不通用；
#   3. 伪造 X-Forwarded-For 不能绕过按 IP 的限流（这是前台前面必须有 nginx 的原因，见 docs/guides/deployment.md）；
#   4. 数据库备份 → 删数据 → 恢复 → 数据回来，且坏备份会被拒绝而不动现有数据。
#
# 用法：just stack-up && just stack-smoke
# 外层 compose-config 已从 Nacos 注入参数；会写入并删除 smoke-*@example.test 测试用户。
set -euo pipefail
cd "$(dirname "$0")/../.."

COMPOSE_PROJECT="${COMPOSE_PROJECT_NAME:-tpl}"
WEB="http://127.0.0.1:${WEB_PORT:-3000}"
ADMIN="http://127.0.0.1:${ADMIN_PORT:-8081}"
JSON='Content-Type: application/json'
dc() { docker compose --env-file /dev/null --project-name "$COMPOSE_PROJECT" --file docker-compose.yml "$@"; }
sql() { dc exec -T postgres psql -U app -d app -tAc "$1"; }

WORK="$(mktemp -d)"
STAMP="$(date +%s)"
EMAIL="smoke-$STAMP@example.test"
PASSWORD="correct-horse-9"
FAILED=0

pass() { echo "PASS  $1"; }
fail() { echo "FAIL  $1   <-- ${2:-}"; FAILED=1; }
# expect <描述> <期望> <实际>
expect() { if [ "$2" = "$3" ]; then pass "$1"; else fail "$1" "期望 $2，实际 $3"; fi; }
status() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
clear_throttle() {
  dc exec -T redis sh -c 'export REDISCLI_AUTH="$REDIS_PASSWORD"; redis-cli --no-auth-warning --scan --pattern "throttle:*" \
    | xargs -r redis-cli --no-auth-warning del' >/dev/null
}
admin_login_status() {
  local cookie_jar="${1:-}"
  local -a cookie_args=()
  if [ -n "$cookie_jar" ]; then cookie_args=(-c "$cookie_jar"); fi
  python3 -c 'import json, os; print(json.dumps({"identifier": os.environ["ADMIN_BOOTSTRAP_USERNAME"], "password": os.environ["ADMIN_BOOTSTRAP_PASSWORD"]}))' \
    | curl -s -o /dev/null -w '%{http_code}' "${cookie_args[@]}" \
      -X POST "$ADMIN/api/admin/auth/login" -H "$JSON" --data-binary @-
}
cleanup() {
  sql "DELETE FROM users WHERE email LIKE 'smoke-%@example.test'" >/dev/null 2>&1 || true
  clear_throttle 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT
clear_throttle

echo "== 入口与代理"
expect "前台页面" 200 "$(status "$WEB/login")"
expect "前台 /api/ready（经 edge 到账号服务，PostgreSQL 与 Redis 都就绪）" 200 "$(status "$WEB/api/ready")"
expect "管理后台页面" 200 "$(status "$ADMIN/sign-in")"
expect "管理后台 /api/ready" 200 "$(status "$ADMIN/api/ready")"

echo "== 普通用户与管理员"
expect "注册" 201 "$(status -X POST "$WEB/api/auth/register" -H "$JSON" -d "{\"email\":\"$EMAIL\",\"password\":\"$PASSWORD\"}")"
expect "登录" 200 "$(status -c "$WORK/user.jar" -X POST "$WEB/api/auth/login" -H "$JSON" -d "{\"email\":\"$EMAIL\",\"password\":\"$PASSWORD\"}")"
expect "已登录 /auth/me" 200 "$(status -b "$WORK/user.jar" "$WEB/api/auth/me")"
expect "管理员登录" 200 "$(admin_login_status "$WORK/admin.jar")"
USER_ID="$(curl -s -b "$WORK/admin.jar" "$ADMIN/api/admin/users?keyword=$EMAIL" | python3 -c 'import sys,json; print(json.load(sys.stdin)["items"][0]["id"])')"
expect "管理员停用用户" 200 "$(status -b "$WORK/admin.jar" -X POST "$ADMIN/api/admin/users/$USER_ID/disable" -H "$JSON" -d '{}')"
expect "停用后用户已有会话立即失效" 401 "$(status -b "$WORK/user.jar" "$WEB/api/auth/me")"
ADMIN_TOKEN="$(awk '$6=="admin_session"{print $7}' "$WORK/admin.jar")"
expect "管理员的令牌放进普通用户 Cookie 无效" 401 "$(status -H "Cookie: session=$ADMIN_TOKEN" "$WEB/api/auth/me")"
expect "未登录访问管理接口" 401 "$(status "$ADMIN/api/admin/users")"

echo "== 限流：伪造 X-Forwarded-For 不能绕过"
clear_throttle
LAST=""
for attempt in $(seq 1 22); do
  LAST="$(status -X POST "$WEB/api/auth/login" -H "$JSON" -H "X-Forwarded-For: 10.250.0.$attempt" \
    -d "{\"email\":\"nobody$attempt@example.test\",\"password\":\"wrong-wrong\"}")"
done
expect "前台：每次换一个伪造 IP，第 22 次仍被限流" 429 "$LAST"
clear_throttle
for attempt in $(seq 1 12); do
  LAST="$(status -X POST "$ADMIN/api/admin/auth/login" -H "$JSON" -H "X-Forwarded-For: 10.251.0.$attempt" \
    -d "{\"identifier\":\"nobody$attempt\",\"password\":\"wrong-wrong\"}")"
done
expect "管理后台：每次换一个伪造 IP，第 12 次仍被限流" 429 "$LAST"
clear_throttle

echo "== 备份与恢复"
COMPOSE_PROJECT_NAME="$COMPOSE_PROJECT" BACKUP_DIR="$WORK/backups" ./scripts/lib/db-backup.sh >"$WORK/backup.log" 2>&1 || { cat "$WORK/backup.log"; fail "备份"; }
DUMP="$(ls "$WORK"/backups/app-*.dump 2>/dev/null | head -1 || true)"
[ -n "$DUMP" ] && pass "备份文件已生成" || fail "备份文件已生成" "没找到 dump"
BEFORE="$(sql 'SELECT count(*) FROM users')"
sql "DELETE FROM users" >/dev/null
expect "删除数据后用户数为 0" 0 "$(sql 'SELECT count(*) FROM users')"
echo garbage >"$WORK/bad.dump"
if COMPOSE_PROJECT_NAME="$COMPOSE_PROJECT" ./scripts/lib/db-restore.sh "$WORK/bad.dump" --yes >/dev/null 2>&1; then fail "坏备份被拒绝" "脚本竟然成功了"; else pass "坏备份被拒绝"; fi
expect "坏备份没有改动现有数据" 0 "$(sql 'SELECT count(*) FROM users')"
COMPOSE_PROJECT_NAME="$COMPOSE_PROJECT" ./scripts/lib/db-restore.sh "$DUMP" --yes >"$WORK/restore.log" 2>&1 || { cat "$WORK/restore.log"; fail "恢复"; }
expect "恢复后用户数回到备份时的数量" "$BEFORE" "$(sql 'SELECT count(*) FROM users')"
for _ in $(seq 1 30); do
  [ "$(dc ps accounts --format '{{.Health}}')" = healthy ] && break
  sleep 2
done
expect "恢复后账号服务健康" healthy "$(dc ps accounts --format '{{.Health}}')"
expect "恢复后管理员仍能登录" 200 "$(admin_login_status)"

echo
if [ "$FAILED" -eq 0 ]; then echo "全部通过"; else echo "有失败项"; exit 1; fi
