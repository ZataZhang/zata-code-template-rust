# 备份与恢复

数据库里有真实用户和管理员数据，所以备份不是可选项。**只有 PostgreSQL 需要备份**：Redis 里只有会话与限流计数，丢了只是让用户重新登录。

备份与恢复命令会和其他 Compose 运维命令一样，先通过 `compose-config` 从 Nacos 读取部署配置。执行机必须能访问 Nacos，并在 `.env.local` 配置 Nacos 地址与认证凭据；namespace/group/Data ID 从项目配置 `[nacos]` 读取。部署值不会保存为本地 `.env.compose` 文件。即使当前操作只针对数据库，也不能在 Nacos 不可用时绕过配置读取。

## 备份

```bash
just db-backup                       # 写到 backups/app-<UTC 时间>.dump
RETENTION_DAYS=14 just db-backup     # 同时清理超过 14 天的旧备份（默认 30）
```

脚本（`scripts/db-backup.sh`）通过 Rust `compose-config` 加载 Nacos 配置后执行；需要安装 Rust/Cargo、Just 与 Docker Compose。做的事：

- 在 postgres 容器里 `pg_dump --format=custom`（压缩，可选择性恢复）
- 先写临时文件，**校验能列出目录且包含 `users` 表后才改名**：失败时不会留下一个看起来正常的坏备份
- 文件权限 600；只清理自己生成的 `app-*.dump`，不动别的文件

`backups/` 已被 `.gitignore` 忽略。**备份不加密**，里面有用户邮箱和密码哈希：放到别的机器或对象存储时务必先加密（如 `age`、`gpg`），并限制访问。

## 恢复

```bash
just db-restore backups/app-20261016-120000Z.dump
```

**会覆盖当前数据库。** 过程：停 accounts → 确认备份文件可读（读不了就中止，**现有数据不动**）→ 清掉现有对象并恢复 → 启动 accounts（自动执行账号迁移，所以恢复到比当前代码旧的备份也能升级到最新结构）。任何一步失败都会尽量把 accounts 重新启动。

恢复后建议立刻检查：`just stack-logs accounts`、登录一次、看用户数。

## 定时备份

脚本只做一次备份，调度交给部署机，选一种：

```cron
# crontab：每天 03:10 备份，保留 14 天
10 3 * * * cd /srv/app && RETENTION_DAYS=14 ./scripts/db-backup.sh >>/var/log/app-backup.log 2>&1
```

也可以用 systemd timer，或平台自带的定时任务。定时执行的主机要能访问 Nacos；把 Nacos 地址和认证参数放在该主机受限的 `.env.local` 或密钥管理中，不要复制 Compose 业务密码到本机。**备份跟数据库放在同一台机器上不算备份**：再用 `rclone` / `aws s3 cp` 等把 `backups/` 同步到异地（记得加密）。

## 演练

没演练过的备份等于没有。上线前和之后每个季度至少做一次：

1. `just db-backup`
2. 在测试环境 `just stack-reset && just stack-up`，然后 `just db-restore <刚才的文件>`
3. 确认用户能登录、管理员能登录并看到用户列表

## 不用本仓库 compose 的部署

数据库在云托管或别处时，直接用 PostgreSQL 自己的工具：

```bash
pg_dump "$DATABASE_URL" --format=custom --no-owner -f app.dump
pg_restore -d "$DATABASE_URL" --clean --if-exists --no-owner app.dump
```

云数据库通常自带自动备份与时间点恢复，优先用它，再用本脚本做一份可迁移的逻辑备份。
