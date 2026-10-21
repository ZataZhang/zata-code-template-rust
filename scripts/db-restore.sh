#!/usr/bin/env bash
# 从备份文件恢复整套服务里的 PostgreSQL。**会覆盖当前数据库的内容。**
#
# 用法：just db-restore backups/app-20261016-120000Z.dump
#       ./scripts/db-restore.sh <文件> --yes     # 跳过确认（脚本里用）
#
# 过程：停 backend（避免恢复期间还有写入）→ 先确认文件可读 → 清掉现有对象并恢复 → 启动 backend。
# backend 启动时会执行迁移，所以恢复到「比当前代码旧」的备份也能自动升级到最新结构。
set -euo pipefail
cd "$(dirname "$0")/.."

FILE="${1:-}"
if [ -z "$FILE" ] || [ ! -f "$FILE" ]; then
  echo "用法：$0 <备份文件> [--yes]" >&2
  exit 2
fi

COMPOSE=(docker compose --env-file .env.compose)

# 先确认备份文件本身是完整的，再动现有数据。
"${COMPOSE[@]}" exec -T postgres pg_restore --list <"$FILE" >/dev/null \
  || { echo "备份文件无法读取，已中止（现有数据未改动）" >&2; exit 1; }

if [ "${2:-}" != "--yes" ]; then
  printf '这会用 %s 覆盖当前数据库。输入 yes 继续：' "$FILE"
  read -r answer
  [ "$answer" = "yes" ] || { echo "已取消"; exit 1; }
fi

echo "==> 停止 backend"
"${COMPOSE[@]}" stop backend
# 失败时也要把 backend 拉起来，不要让服务一直停着。
trap '"${COMPOSE[@]}" start backend >/dev/null 2>&1 || true' EXIT

echo "==> 恢复"
"${COMPOSE[@]}" exec -T postgres pg_restore -U app -d app --clean --if-exists --no-owner --exit-on-error <"$FILE"

echo "==> 启动 backend（会自动执行迁移）"
"${COMPOSE[@]}" start backend
trap - EXIT
echo "恢复完成。"
