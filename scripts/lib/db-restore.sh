#!/usr/bin/env bash
# 从指定备份恢复 accounts 或 Agent tracing PostgreSQL。**会覆盖对应数据库的内容。**
#
# 用法：just db-restore backups/app-20261016-120000Z.dump
#       just db-restore backups/agent-tracing-20261016-120000Z.dump
#
# 恢复前停止对应写服务，避免在恢复期间继续追加数据；完成后重启服务并自动执行迁移。
set -euo pipefail
cd "$(dirname "$0")/../.."

FILE="${1:-}"
if [ -z "$FILE" ] || [ ! -f "$FILE" ]; then
  echo "用法：$0 <备份文件> [--yes]" >&2
  exit 2
fi

COMPOSE_PROJECT="${COMPOSE_PROJECT_NAME:-tpl}"
COMPOSE=(docker compose --env-file /dev/null --project-name "$COMPOSE_PROJECT" --file docker-compose.yml)

case "$(basename "$FILE")" in
  app-*.dump)
    database_container="postgres"
    database_user="app"
    database_name="app"
    application_service="accounts"
    ;;
  agent-tracing-*.dump)
    database_container="tracing-postgres"
    database_user="agent_tracing"
    database_name="agent_tracing"
    application_service="agent-tracing"
    ;;
  *)
    echo "备份文件名必须以 app- 或 agent-tracing- 开头" >&2
    exit 2
    ;;
esac

# 先确认备份文件本身是完整的，再动现有数据。
"${COMPOSE[@]}" exec -T "$database_container" pg_restore --list <"$FILE" >/dev/null \
  || { echo "备份文件无法读取，已中止（现有数据未改动）" >&2; exit 1; }

if [ "${2:-}" != "--yes" ]; then
  printf '这会用 %s 覆盖 %s 数据库。输入 yes 继续：' "$FILE" "$database_name"
  read -r answer
  [ "$answer" = "yes" ] || { echo "已取消"; exit 1; }
fi

echo "==> 停止 $application_service"
"${COMPOSE[@]}" stop "$application_service"
trap '"${COMPOSE[@]}" start "$application_service" >/dev/null 2>&1 || true' EXIT

echo "==> 恢复"
"${COMPOSE[@]}" exec -T "$database_container" pg_restore -U "$database_user" -d "$database_name" --clean --if-exists --no-owner --exit-on-error <"$FILE"

echo "==> 启动 $application_service（会自动执行迁移）"
"${COMPOSE[@]}" start "$application_service"
trap - EXIT
echo "恢复完成。"
