#!/usr/bin/env bash
# 首次启动整套服务前生成 .env.compose：把 .env.compose.example 里留空的密钥换成随机值。
# 已存在则什么都不改（绝不覆盖你已有的密钥）。
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -f .env.compose ]; then
  exit 0
fi

random_secret() { openssl rand -hex 16; }

umask 077
sed \
  -e "s|^POSTGRES_PASSWORD=$|POSTGRES_PASSWORD=$(random_secret)|" \
  -e "s|^REDIS_PASSWORD=$|REDIS_PASSWORD=$(random_secret)|" \
  -e "s|^ADMIN_BOOTSTRAP_PASSWORD=$|ADMIN_BOOTSTRAP_PASSWORD=$(random_secret)|" \
  .env.compose.example >.env.compose

echo "已生成 .env.compose（权限 600，已被 .gitignore 忽略）。"
echo "管理后台账号：$(grep '^ADMIN_BOOTSTRAP_USERNAME=' .env.compose | cut -d= -f2-) / $(grep '^ADMIN_BOOTSTRAP_PASSWORD=' .env.compose | cut -d= -f2-)"
