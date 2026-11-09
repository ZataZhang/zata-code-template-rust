#!/usr/bin/env bash
# MCP 服务真实端到端：启动测试中间件里的 MCP 专属 PostgreSQL，重置 schema，
# 然后运行真实 mcp-service 进程与本地假 MCP 上游之间的场景。
#
# 用法：
#   just mcp-e2e
#   just mcp-e2e --scenario authorization
#   just mcp-e2e --scenario remote-connection
#
# 可选环境变量：
#   MCP_E2E_DATABASE_URL  覆盖 MCP 测试库连接串（默认使用 docker-compose.testing.yml 的 5436 实例）
#
# 只有远端 MCP provider 是本地假实现；服务的 HTTP 入口、调用方验签、授权策略、
# PostgreSQL 迁移与仓储、凭据加密、出站地址校验和 rmcp 传输都是真实实现。
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
  ""|authorization|remote-connection) ;;
  *)
    echo "未知场景：$SCENARIO" >&2
    echo "可用：authorization、remote-connection" >&2
    exit 2
    ;;
esac

COMPOSE=(docker compose -f docker-compose.testing.yml)
MCP_DATABASE_URL="${MCP_E2E_DATABASE_URL:-postgres://mcp:mcppass@127.0.0.1:5436/mcp_dev}"

echo "==> 启动测试中间件（MCP PostgreSQL 5436）"
"${COMPOSE[@]}" up -d --wait mcp-postgres >/dev/null

echo "==> 构建 mcp-service"
cargo build -p tpl-mcp --bin mcp-service --quiet

run_scenario() {
  local scenario_name=$1
  local test_name
  case "$scenario_name" in
    authorization) test_name="real_authorization_scenario" ;;
    remote-connection) test_name="real_remote_connection_scenario" ;;
    *) echo "未知场景：$scenario_name" >&2; exit 2 ;;
  esac

  echo "==> 重置 MCP 测试库 schema（场景：${scenario_name}）"
  "${COMPOSE[@]}" exec -T mcp-postgres psql -U mcp -d mcp_dev -v ON_ERROR_STOP=1 \
    -c 'DROP SCHEMA IF EXISTS public CASCADE; CREATE SCHEMA public;' >/dev/null

  echo "==> 运行场景：${scenario_name}（真实 PostgreSQL + mcp-service + 本地假上游）"
  TEST_MCP_DATABASE_URL="$MCP_DATABASE_URL" \
    cargo test -p tpl-mcp --test http_end_to_end "$test_name" -- --ignored --nocapture
}

if [ -z "$SCENARIO" ]; then
  run_scenario authorization
  run_scenario remote-connection
else
  run_scenario "$SCENARIO"
fi

echo "==> MCP 端到端场景通过"
