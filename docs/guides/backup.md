# 备份与恢复

accounts 与 Agent tracing 分别拥有独立 PostgreSQL 数据库，所以每次备份会为两者各生成一份 dump。Redis 里只有会话与限流计数，丢了只是让用户重新登录。

备份与恢复命令会和其他 Compose 运维命令一样，先通过 `compose-config` 从 Nacos 读取部署配置。执行机必须能访问 Nacos，并在 `.env.local` 配置 Nacos 地址与认证凭据；namespace/group/Data ID 从项目配置 `[nacos]` 读取。部署值不会保存为本地 `.env.compose` 文件。即使当前操作只针对数据库，也不能在 Nacos 不可用时绕过配置读取。

## 备份

```bash
just db-backup                       # 写出 app-<UTC 时间>.dump 与 agent-tracing-<UTC 时间>.dump
RETENTION_DAYS=14 just db-backup     # 同时清理两个前缀、超过 14 天的旧备份（默认 30）
```

脚本（`scripts/db-backup.sh`）通过 Rust `compose-config` 加载 Nacos 配置后执行；需要安装 Rust/Cargo、Just 与 Docker Compose。它会分别在 `postgres` 与 `tracing-postgres` 容器中执行 `pg_dump --format=custom`，并先写临时文件、校验 dump 含有预期数据表后才保存正式文件。账号 dump 检查 `users` 表，tracing dump 检查 `agent_run_events` 表。

文件权限为 600；脚本只清理自己生成的 `app-*.dump` 与 `agent-tracing-*.dump`。`backups/` 已被 `.gitignore` 忽略。**备份不加密**，包含用户邮箱、密码哈希、Agent 消息和工具参数/结果原文；放到异地或对象存储前务必加密（如 `age`、`gpg`）并限制访问。

## 恢复

```bash
just db-restore backups/app-20261029-120000Z.dump
just db-restore backups/agent-tracing-20261029-120000Z.dump
```

**会覆盖文件名前缀指定的数据库。** `app-*.dump` 停止 accounts 后恢复账号库；`agent-tracing-*.dump` 停止 agent-tracing 后恢复 tracing 库。脚本先确认 dump 可读；不可读时中止且不改现有数据。恢复后重启对应服务，由服务自动执行自身迁移。任何一步失败都会尽量重启对应服务。

恢复 accounts 后检查 `just stack-logs accounts` 并尝试管理员登录；恢复 tracing 后检查 `just stack-logs agent-tracing` 与 Run/Thread 查询。两份文件应使用相同 UTC 时间戳以选取同一轮逻辑备份；两套数据库之间不提供跨库原子快照。

## 定时备份

脚本只做一次备份，调度交给部署机，选一种：

```cron
# crontab：每天 03:10 备份，保留 14 天
10 3 * * * cd /srv/app && RETENTION_DAYS=14 ./scripts/db-backup.sh >>/var/log/app-backup.log 2>&1
```

也可以用 systemd timer，或平台自带的定时任务。定时执行的主机要能访问 Nacos；把 Nacos 地址和认证参数放在该主机受限的 `.env.local` 或密钥管理中，不要复制 Compose 业务密码到本机。**备份跟数据库放在同一台机器上不算备份**：再用 `rclone` / `aws s3 cp` 等把 `backups/` 同步到异地，并加密。

## 演练

没演练过的备份等于没有。上线前和之后每个季度至少做一次：

1. `just db-backup`
2. 在测试环境 `just stack-reset && just stack-up`
3. 分别对两份 dump 执行 `just db-restore <文件>`
4. 确认管理员能登录并看到用户列表，tracing Run 与 Thread 数据能查询

## 不用本仓库 Compose 的部署

accounts 与 tracing 数据库分别在云端或别处时，为两个连接串各运行一份 `pg_dump` / `pg_restore`。云数据库通常自带自动备份与时间点恢复，优先用它，再额外保留可迁移逻辑备份。
