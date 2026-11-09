#!/usr/bin/env bash
# 备份 accounts、Agent 服务与 Agent tracing 各自拥有的 PostgreSQL 数据库，pg_dump 自定义格式写到 backups/。
#
# 用法：just db-backup            # 备份并清理超过保留天数的旧备份
#       RETENTION_DAYS=14 just db-backup
#
# 备份文件只含数据库；Redis 里只有会话与限流计数，丢了只是让用户重新登录，不需要备份。
# 文件含账号哈希、Agent 消息/提示词原文与 tracing 事件，备份**不加密**，异地传输前必须加密。
set -euo pipefail
cd "$(dirname "$0")/../.."

COMPOSE_PROJECT="${COMPOSE_PROJECT_NAME:-tpl}"
COMPOSE=(docker compose --env-file /dev/null --project-name "$COMPOSE_PROJECT" --file docker-compose.yml)
BACKUP_DIR="${BACKUP_DIR:-backups}"
RETENTION_DAYS="${RETENTION_DAYS:-30}"
STAMP="$(date -u +%Y%m%d-%H%M%SZ)"
TARGET="$BACKUP_DIR/app-$STAMP.dump"
AGENT_TARGET="$BACKUP_DIR/agent-$STAMP.dump"
TRACING_TARGET="$BACKUP_DIR/agent-tracing-$STAMP.dump"

mkdir -p "$BACKUP_DIR"
umask 077
# 先写临时文件，成功且校验通过后再改名：失败时不会留下一个看起来正常的坏备份。
TEMP="$TARGET.partial"
AGENT_TEMP="$AGENT_TARGET.partial"
TRACING_TEMP="$TRACING_TARGET.partial"
trap 'rm -f "$TEMP" "$AGENT_TEMP" "$TRACING_TEMP"' EXIT

"${COMPOSE[@]}" exec -T postgres pg_dump -U app -d app --format=custom --no-owner >"$TEMP"

# 校验：完整消费目录输出；grep -q 提前退出会使上游收到 SIGPIPE，
# 在 pipefail 下把包含目标表的大目录误报为失败。仍保留 pg_restore 的失败状态。
if ! "${COMPOSE[@]}" exec -T postgres pg_restore --list <"$TEMP" | grep ' users$\| TABLE public users ' >/dev/null; then
  echo "备份校验失败：dump 里找不到 users 表，已丢弃" >&2
  exit 1
fi
mv "$TEMP" "$TARGET"

"${COMPOSE[@]}" exec -T agent-postgres pg_dump -U agent -d agent --format=custom --no-owner >"$AGENT_TEMP"

if ! "${COMPOSE[@]}" exec -T agent-postgres pg_restore --list <"$AGENT_TEMP" | grep ' TABLE public business_agents ' >/dev/null; then
  echo "Agent 备份校验失败：dump 里找不到 business_agents 表，已丢弃 agent 临时文件" >&2
  exit 1
fi
mv "$AGENT_TEMP" "$AGENT_TARGET"

"${COMPOSE[@]}" exec -T tracing-postgres pg_dump -U agent_tracing -d agent_tracing --format=custom --no-owner >"$TRACING_TEMP"

if ! "${COMPOSE[@]}" exec -T tracing-postgres pg_restore --list <"$TRACING_TEMP" | grep ' TABLE public agent_run_events ' >/dev/null; then
  echo "tracing 备份校验失败：dump 里找不到 agent_run_events 表，已丢弃 tracing 临时文件" >&2
  exit 1
fi
mv "$TRACING_TEMP" "$TRACING_TARGET"
trap - EXIT
echo "accounts 备份完成：$TARGET（$(du -h "$TARGET" | cut -f1)）"
echo "agent 备份完成：$AGENT_TARGET（$(du -h "$AGENT_TARGET" | cut -f1)）"
echo "tracing 备份完成：$TRACING_TARGET（$(du -h "$TRACING_TARGET" | cut -f1)）"

# 清理旧备份。只动本脚本生成的文件名，不碰别的东西。
find "$BACKUP_DIR" -maxdepth 1 -name 'app-*.dump' -type f -mtime "+$RETENTION_DAYS" -print -delete \
  | sed 's/^/已清理过期备份：/'
find "$BACKUP_DIR" -maxdepth 1 -name 'agent-*.dump' -type f ! -name 'agent-tracing-*.dump' -mtime "+$RETENTION_DAYS" -print -delete \
  | sed 's/^/已清理过期备份：/'
find "$BACKUP_DIR" -maxdepth 1 -name 'agent-tracing-*.dump' -type f -mtime "+$RETENTION_DAYS" -print -delete \
  | sed 's/^/已清理过期备份：/'
