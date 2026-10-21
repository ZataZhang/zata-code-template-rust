#!/usr/bin/env bash
# 备份整套服务（docker-compose.yml）里的 PostgreSQL：pg_dump 自定义格式，写到 backups/。
#
# 用法：just db-backup            # 备份并清理超过保留天数的旧备份
#       RETENTION_DAYS=14 just db-backup
#
# 备份文件只含数据库；Redis 里只有会话与限流计数，丢了只是让用户重新登录，不需要备份。
# 备份**不加密**：含用户邮箱与密码哈希，放到别处（异地、对象存储）时务必加密，见 docs/guides/backup.md。
set -euo pipefail
cd "$(dirname "$0")/.."

COMPOSE=(docker compose --env-file .env.compose)
BACKUP_DIR="${BACKUP_DIR:-backups}"
RETENTION_DAYS="${RETENTION_DAYS:-30}"
STAMP="$(date -u +%Y%m%d-%H%M%SZ)"
TARGET="$BACKUP_DIR/app-$STAMP.dump"

mkdir -p "$BACKUP_DIR"
umask 077
# 先写临时文件，成功且校验通过后再改名：失败时不会留下一个看起来正常的坏备份。
TEMP="$TARGET.partial"
trap 'rm -f "$TEMP"' EXIT

"${COMPOSE[@]}" exec -T postgres pg_dump -U app -d app --format=custom --no-owner >"$TEMP"

# 校验：能列出目录说明文件是完整可读的 dump，且确实包含用户表。
if ! "${COMPOSE[@]}" exec -T postgres pg_restore --list <"$TEMP" | grep -q ' users$\| TABLE public users '; then
  echo "备份校验失败：dump 里找不到 users 表，已丢弃" >&2
  exit 1
fi
mv "$TEMP" "$TARGET"
trap - EXIT
echo "备份完成：$TARGET（$(du -h "$TARGET" | cut -f1)）"

# 清理旧备份。只动本脚本生成的文件名，不碰别的东西。
find "$BACKUP_DIR" -maxdepth 1 -name 'app-*.dump' -type f -mtime "+$RETENTION_DAYS" -print -delete \
  | sed 's/^/已清理过期备份：/'
