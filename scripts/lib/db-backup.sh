#!/usr/bin/env bash
# 备份 accounts 与 Agent tracing 各自拥有的 PostgreSQL 数据库，pg_dump 自定义格式写到 backups/。
#
# 用法：just db-backup            # 备份并清理超过保留天数的旧备份
#       RETENTION_DAYS=14 just db-backup
#
# 备份文件只含数据库；Redis 里只有会话与限流计数，丢了只是让用户重新登录，不需要备份。
# 文件含账号哈希与 Agent 消息/工具原文，备份**不加密**，异地传输前必须加密。
set -euo pipefail
cd "$(dirname "$0")/../.."

COMPOSE_PROJECT="${COMPOSE_PROJECT_NAME:-tpl}"
COMPOSE=(docker compose --env-file /dev/null --project-name "$COMPOSE_PROJECT" --file docker-compose.yml)
BACKUP_DIR="${BACKUP_DIR:-backups}"
RETENTION_DAYS="${RETENTION_DAYS:-30}"
STAMP="$(date -u +%Y%m%d-%H%M%SZ)"
TARGET="$BACKUP_DIR/app-$STAMP.dump"
TRACING_TARGET="$BACKUP_DIR/agent-tracing-$STAMP.dump"

mkdir -p "$BACKUP_DIR"
umask 077
# 先写临时文件，成功且校验通过后再改名：失败时不会留下一个看起来正常的坏备份。
TEMP="$TARGET.partial"
TRACING_TEMP="$TRACING_TARGET.partial"
trap 'rm -f "$TEMP" "$TRACING_TEMP"' EXIT

"${COMPOSE[@]}" exec -T postgres pg_dump -U app -d app --format=custom --no-owner >"$TEMP"

# 校验：能列出目录说明文件是完整可读的 dump，且确实包含用户表。
if ! "${COMPOSE[@]}" exec -T postgres pg_restore --list <"$TEMP" | grep -q ' users$\| TABLE public users '; then
  echo "备份校验失败：dump 里找不到 users 表，已丢弃" >&2
  exit 1
fi
mv "$TEMP" "$TARGET"
"${COMPOSE[@]}" exec -T tracing-postgres pg_dump -U agent_tracing -d agent_tracing --format=custom --no-owner >"$TRACING_TEMP"

if ! "${COMPOSE[@]}" exec -T tracing-postgres pg_restore --list <"$TRACING_TEMP" | grep -q ' TABLE public agent_run_events '; then
  echo "tracing 备份校验失败：dump 里找不到 agent_run_events 表，已丢弃 tracing 临时文件" >&2
  exit 1
fi
mv "$TRACING_TEMP" "$TRACING_TARGET"
trap - EXIT
echo "accounts 备份完成：$TARGET（$(du -h "$TARGET" | cut -f1)）"
echo "tracing 备份完成：$TRACING_TARGET（$(du -h "$TRACING_TARGET" | cut -f1)）"

# 清理旧备份。只动本脚本生成的文件名，不碰别的东西。
find "$BACKUP_DIR" -maxdepth 1 -name 'app-*.dump' -type f -mtime "+$RETENTION_DAYS" -print -delete \
  | sed 's/^/已清理过期备份：/'
find "$BACKUP_DIR" -maxdepth 1 -name 'agent-tracing-*.dump' -type f -mtime "+$RETENTION_DAYS" -print -delete \
  | sed 's/^/已清理过期备份：/'
