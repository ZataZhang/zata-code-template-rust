# 部署

这份指南覆盖 accounts 与 Agent tracing 两个后端、三个独立前端入口的 Docker Compose 部署、生产要求和 HTTPS 代理。部署配置存于 Nacos；本机 `.env.local` 只提供 Nacos 地址和认证凭据。namespace、group、应用 TOML Data ID 与 Compose Properties Data ID 固定在项目 `config.toml` 的 `[nacos]` 区域。配置项见 [`configuration.md`](configuration.md)，账号边界见 [`accounts-integration.md`](accounts-integration.md)，数据库备份见 [`backup.md`](backup.md)。

## 1. 本机或单机部署

准备 Docker Compose、Just、Rust/Cargo，并在 Nacos 中发布 `config.toml` 的 `[nacos].compose_data_id` 所指 Properties 配置（默认 `rust-study-compose.properties`）。配置格式和允许的键见 [`configuration.md`](configuration.md)。`.env.local` 示例：

```env
NACOS_SERVER_ADDR=http://127.0.0.1:8848
NACOS_USERNAME=
NACOS_PASSWORD=
```

生产环境使用 HTTPS 的 Nacos 地址，并通过部署平台的密钥管理注入认证值。不要把 PostgreSQL、Redis 或管理员密码写进 `.env.local`。`compose-config` 在每次运维命令前读取 Nacos；配置只存在当前进程和 Docker Compose 子进程环境，不生成本地部署环境文件。它使用 `--env-file /dev/null` 禁止 Compose 回退读取本地 `.env` 文件。Nacos 不可达、认证失败、配置不存在或格式错误时，命令会在运行 Docker 前失败。

```bash
just stack-up
```

首次构建 accounts、agent-tracing 与三个独立前端镜像，并启动两套独立 PostgreSQL，通常需要几分钟。Nacos 中没有设置管理员引导凭据时，不会自动创建管理员；producer 清单为空数组时，管理 UI 可用，但没有 Agent 可以写入事件。

### 从旧版 Compose 升级

旧版业务容器名为 `backend`，新版改为 `accounts`。更新部署前，在旧版工作目录运行 `docker compose --env-file .env.compose down`，不要加 `-v`，这样可以移除旧容器并保留 PostgreSQL、Redis 数据卷。把旧版配置迁入 Nacos 后，使用同一个 `COMPOSE_PROJECT_NAME`（默认 `tpl`）运行新版 `just stack-up`，它会继续使用原数据卷。迁移后 `.env.compose` 不再是部署配置来源。

| 入口 | 地址 | 说明 |
|---|---|---|
| 前台站点 | `http://localhost:3000` | 经 `edge`（nginx）进入；`/api/*` 转给账号服务 |
| 管理后台 | `http://localhost:8081` | 自带 nginx；使用 Nacos 中配置的引导管理员登录 |
| Tracing Studio | `http://localhost:8082` | 独立 Vite/React 服务；同源代理管理员认证、轨迹查询和 producer 写入 |
| Producer ingest API | `http://localhost:8082/api/v1/ingest/runs/{run_id}/events` | tracing UI nginx 保留 `/api/v1/ingest/*` 原路径并代理到 agent-tracing |
| accounts API / tracing API | 不直接发布宿主机端口 | 经各自前端 nginx 或受控内部网络访问 |

常用命令：`just stack-logs [服务名]`、`just tracing-up` / `just tracing-down`、`just stack-down`（保留数据）、`just stack-reset`（**删除两套数据库与 Redis 数据卷**）。这些命令同样需要先从 Nacos 读取配置。

宿主机端口和 Compose 项目可对单条命令临时覆盖，不会写回 Nacos：

```bash
COMPOSE_PROJECT_NAME=tpl-review COMPOSE_WEB_PORT=13000 COMPOSE_ADMIN_PORT=13001 COMPOSE_TRACING_UI_PORT=13002 just stack-up
```

`COMPOSE_ACCOUNTS_PORT` 可临时覆盖独立 API 模式端口。项目名只接受小写字母、数字、连字符和下划线。旧的 `ENV_FILE` 变量已停用；设置时，compose-config 会明确报错。

### 单独启动账号服务

`just accounts-up` 启动账号服务及 PostgreSQL、Redis，不启动站点和管理后台；它通过 `docker-compose.accounts.yml` 只向 `127.0.0.1:8082` 发布 API，并关闭 `X-Forwarded-For` 信任。它与完整站点默认共用 `tpl` 项目和同一批数据卷，是替代运行模式；在完整站点运行时执行会重建同一个 `accounts` 容器，`just accounts-down` 也会停止该容器。要并行运行，请为两种模式使用不同的 `COMPOSE_PROJECT_NAME`，并给独立 API 选一个空闲端口：

```bash
COMPOSE_PROJECT_NAME=accounts-dev COMPOSE_ACCOUNTS_PORT=28082 just accounts-up
```

### 服务关系

```text
浏览器 → edge(nginx) ─ /api/ ──────────────→ accounts:8080 → accounts postgres, redis
              └─ /  → web(Next.js)
浏览器 → admin(nginx) ─ /api/ ─────────────→ accounts:8080
浏览器 → tracing-ui(nginx) ─ /api/admin/auth/* → accounts:8080
                       ├─ /api/v1/ingest/* ────→ agent-tracing:8090 → tracing postgres
                       ├─ /api/admin/agent-traces/* → agent-tracing:8090
                       └─ / → Trace Studio
Agent producer ──────────────────────────────────→ tracing-ui/api/v1/ingest/*
```

- `accounts` 的 `/ready` 检查自己的 PostgreSQL 与 Redis；`agent-tracing` 的 `/ready` 检查 tracing 专属 PostgreSQL。任一数据库不可用约 30 秒后相应服务变为 `unhealthy`。
- 两个后端启动时分别只执行自己数据库中的迁移；`agent-tracing` 不连接或迁移 accounts 数据库。
- Redis 使用 AOF 持久化；重启后用户会话通常仍然有效。

## 2. 为什么前台前面必须有 nginx

后端按 IP 限流，完整站点启用 `trust_forwarded_for=true`。只有由可信代理覆盖写 `X-Forwarded-For`，该头才可信：

- **Next.js rewrite 不会追加这个头**，客户端发来的值会原样透传。直接暴露 Next.js 再开启 `trust_forwarded_for`，攻击者可伪造 IP 绕过限流。
- `edge` 与管理后台的 nginx 会覆盖写这个头，完整站点不发布 `accounts` 端口，外部请求不能绕过代理直连。

所以 `web` 不发布宿主机端口，浏览器只能经 `edge` 或 `admin` 访问。前面再加 HTTPS 代理时，按下一节只在 Nacos 配置中信任实际代理网段。

## 3. 生产要求

部署前在 Nacos Compose Data ID 中确认：

| 项 | 要求 |
|---|---|
| `POSTGRES_PASSWORD`、`REDIS_PASSWORD` | accounts 使用的强随机值；迁移现有部署时保留原值，不要提交到 Git 或本机配置文件 |
| `AGENT_TRACING_DB_PASSWORD` | tracing 专属数据库的独立 URL-safe 强随机值；不要与 accounts 密码复用 |
| `AGENT_TRACING_PRODUCERS_JSON` | 每个 producer 绑定固定 `source_id` / `project_id` 的 Bearer Token；至少 32 字符，放在受控 Nacos 配置或密钥管理中 |
| `ADMIN_BOOTSTRAP_PASSWORD` | 如需引导管理员，使用强随机值；账号只在**不存在**时创建，事后改配置不会改已有密码 |
| `COOKIE_SECURE=true` | HTTPS 部署必须开启；否则会话 Cookie 可能经明文传输 |
| `TRUSTED_PROXY_CIDR` | 配置 HTTPS 终止代理的 IP/CIDR；默认只认直接连接对端 |
| 备份 | 配置定时备份并演练恢复，见 [`backup.md`](backup.md) |

数据库、Redis 和管理员密码只由 compose-config 进程注入容器。不要通过 `docker inspect`、调试输出或日志导出这些环境字段。

## 4. 前面增加 HTTPS 代理

浏览器 → HTTPS 代理 → `edge` / `admin` / `tracing-ui`。在 Nacos Compose 配置中填写 HTTPS 代理地址段，例如：

```properties
TRUSTED_PROXY_CIDR=172.18.0.0/16
```

nginx 会从受信任的代理请求里还原客户端地址，再覆盖写 `X-Forwarded-For`。仅填写自己代理所在的网段；`0.0.0.0/0` 会让客户端伪造来源 IP。该值只接受 IP 地址或 CIDR。HTTPS 代理需要将 `edge`、`admin` 与 `tracing-ui` 作为三个站点转发；producer 向 tracing UI 的 `/api/v1/ingest/*` 地址发送 Bearer Token，不要把 token 放入浏览器代码或查询参数。同时限制三个 UI 宿主机端口，只允许 HTTPS 代理连接。

验证时，从同一真实客户端连续输错密码，应在设定次数后得到 429；换一个客户端 IP 后不应共享同一 IP 计数。

## 5. 单独部署各个镜像

| 镜像 | 构建 | 运行时配置 |
|---|---|---|
| 账号服务 | `just docker-build` | `DATABASE_URL`、`REDIS_URL` 等；监听 8080，以非 root 运行 |
| 前台 | `just web-docker-build <账号服务地址>` | 构建参数 `BACKEND_URL` 指向账号服务；修改后需重新构建 |
| 管理后台 | `just web-admin-docker-build` | `BACKEND_UPSTREAM`（默认 `accounts:8080`）、`TRUSTED_PROXY_CIDR` |
| Agent tracing 后端 | `just tracing-docker-build` | tracing 独立 `APP_DATABASE__URL`、`APP_ACCOUNTS__ADMIN_AUTH_URL`、`APP_PRODUCER__CREDENTIALS_JSON`；监听 8090，以非 root 运行 |
| Tracing Studio | `just web-tracing-docker-build` | `ACCOUNTS_UPSTREAM`、`TRACING_UPSTREAM`、`TRUSTED_PROXY_CIDR`；同时代理 producer 写入口 |

单独运行镜像时由部署平台提供运行时变量。Nacos Compose 读取器只管理本仓库 Compose 配置，不是账号服务镜像的一部分。容器镜像不带 `curl`；编排平台的探针直接请求 `/live`、`/ready`。

## 6. 排错

- `compose-config` 报 Nacos 配置加载失败：检查 `.env.local` 中 Nacos server/认证参数、网络访问，以及 `config.toml` `[nacos]` 的 namespace/group/Data ID 与读取权限。不会回退读取 `.env` 或 `.env.compose`。
- 配置格式错误：检查是否有未知/重复键、缺少数据库或 Redis 密码、无效端口、无效 `COOKIE_SECURE`/`LOG_FORMAT` 或无效代理地址段。
- `accounts` 一直 `unhealthy`：`just stack-logs accounts`；常见原因是 Nacos 密码与数据卷里已有的数据库密码不一致。生产不要用 `just stack-reset` 排错，因为它会清空数据；应通过受控数据库操作同步密码。
- `agent-tracing` 一直 `unhealthy`：`just stack-logs agent-tracing tracing-postgres`；检查 tracing 专属密码和 `AGENT_TRACING_DB_PASSWORD`，不要改用 accounts 数据库。
- producer 收到 `401 producer_unauthorized`：核对 `AGENT_TRACING_PRODUCERS_JSON` 中的 source/project/token；未出现在当前配置的 source 会被禁用。
- producer 收到 `409 sequence_gap`：按 `expected_seq` 从 producer 持久队列重发连续事件；同一序号不同内容不能覆盖已提交事件。
- 前台能打开但登录 502：`edge` 解析不到 accounts。accounts 只在 Compose 网络内可见。
- 登录时报「尝试次数过多」：命中了限流；窗口过后自动恢复。
