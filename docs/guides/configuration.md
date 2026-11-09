# 配置

优先级从低到高：**代码默认值 < `config.toml` < 可选 Nacos TOML < 环境变量**。未启用 Nacos 时，配置加载不访问外部服务。

- 覆盖单个值：`APP_<分区>__<字段>`，双下划线表示层级，如 `APP_SERVER__PORT=9000`
- 连接串用约定俗成的名字：`DATABASE_URL`、`REDIS_URL`
- 本机仅将 Nacos 地址和认证凭据放 `.env.local`（启动时自动加载，不提交）；生产通过环境变量或部署平台密钥注入
- `NACOS_SERVER_ADDR` 配置后，服务通过 Nacos 3.x Client API 读取项目 `[nacos]` 中指定的应用 TOML；定位信息不是环境变量，连接地址和凭据不会存入 Nacos。非本机 Nacos 地址必须使用 HTTPS
- 指定别的配置文件：`APP_CONFIG_FILE=/path/to/config.toml`
- 含密码的字段（连接串、引导管理员密码）在 `Debug` 输出里被隐藏，不会进日志

完整的带注释示例见仓库根的 [`config.toml`](../../config.toml) 与 [`.env.example`](../../.env.example)。

## Agent 服务

Agent 服务（`agent-service`，默认端口 **8091**）读取根 [`config.toml`](../../config.toml) 的 `[agent]` 子树；accounts 与 tracing 的配置不会进入该进程。数据库连接由独立 `DATABASE_URL`（或 Nacos 应用 TOML 的 `agent.database.url`）提供，启动时必须可达并执行 `services/agent/migrations/`。秘密只由环境或部署平台注入：

| 配置 | 默认 | 说明 |
|---|---|---|
| `DATABASE_URL` | 未配置 | **必填**，Agent 专属 PostgreSQL；不要与 accounts/tracing 共用数据库 |
| `AGENT_SERVICE_TOKENS_JSON` | `[]` | 调用服务清单 `[{"service_id":"web","token":"…"}]`；token 至少 32 字符，浏览器永远不持有 |
| `AGENT_TRACING_PRODUCER_TOKEN` | 未配置 | 向 agent-tracing 投递事件的 producer token；缺省时不启动投递 worker，事件留在 outbox |
| `AGENT_CREDENTIAL_<ALIAS>` | 未配置 | 外部 A2A Runtime 的 Bearer 密钥，按 `credential_alias` 读取；缺省时该 Runtime 明确不可用 |
| `APP_AGENT__PLATFORM_ADMIN_IDS` | `[]` | 平台管理员 accounts ID 列表；只允许创建组织与授权组织管理员，不自动获得组织 scope |
| `APP_AGENT__SERVER__HOST` / `PORT` | `127.0.0.1` / `8091` | 容器镜像绑定 `0.0.0.0:8091` |
| `APP_AGENT__IDENTITY__USER_AUTH_URL` / `ADMIN_AUTH_URL` | accounts 的 `/auth/me`、`/admin/auth/me` | 用户与管理员身份分别校验 |
| `APP_AGENT__IDENTITY__REQUEST_TIMEOUT_SECS` | `3` | 调用 accounts 的超时 |
| `APP_AGENT__STORAGE__ROOT` | `./data/blobs` | 资源、Skill 资产与 Run 产物目录；Compose 挂载独立卷 |
| `APP_AGENT__STORAGE__MAX_RESOURCE_BYTES` / `MAX_ARTIFACT_BYTES` | 10 MiB | 单文件上限 |
| `APP_AGENT__EXECUTION__A2A_TIMEOUT_SECS` | `60` | 单次 A2A 请求超时 |
| `APP_AGENT__TRACING__INGEST_URL` | `http://127.0.0.1:8090/api/v1/ingest` | tracing producer 基础地址 |
| `APP_AGENT__TRACING__POLL_INTERVAL_MS` / `BATCH_EVENTS` | `1000` / `100` | outbox 轮询与批次上限 |
| `[[agent.builtin_runtimes]]` | 见 `config.toml` | 部署声明的只读 builtin Runtime（工具白名单与步骤上限） |

完整说明见 [`agent-service.md`](agent-service.md)；所有调用方都要先通过服务 token 校验，再经 accounts 公开 HTTP 校验用户/管理员 Cookie。

## Agent tracing 服务

tracing 后端服务从 [`services/agent-tracing/config.toml`](../../services/agent-tracing/config.toml) 读取默认配置，独立于账号服务与 Agent 服务数据库与配置。密钥变量只在 tracing 进程或部署平台中注入：

| 配置 | 默认 | 说明 |
|---|---|---|
| `APP_DATABASE__URL` | 未配置 | 必须提供 tracing 专属 PostgreSQL 连接串；服务启动时执行 `services/agent-tracing/migrations/` |
| `APP_ACCOUNTS__ADMIN_AUTH_URL` | `http://accounts:8080/admin/auth/me` | tracing API 校验管理员 Cookie 的公开 accounts 接口；独立部署时改为可访问的 accounts URL |
| `APP_PRODUCER__CREDENTIALS_JSON` | `[]` | producer JSON 数组；每项含 `source_id`、`project_id`、`token`。token 至少 32 字符，明文只存在启动配置，不进入数据库或日志 |
| `APP_OTLP__TRACES_ENDPOINT` | 未配置 | 完整 OTLP/HTTP traces URL；默认完全关闭外部导出 |
| `APP_SERVER__HOST` / `APP_SERVER__PORT` | `127.0.0.1` / `8090` | 独立运行监听地址；容器镜像绑定 `0.0.0.0:8090` |

producer 凭据 JSON 示例：

```json
[{"source_id":"support-platform","project_id":"production","token":"使用至少32字符的URL-safe随机值"}]
```

JSON 不要写入服务 TOML 或提交到仓库。完整事件、错误码与续传规则见 [`services/agent-tracing/README.md`](../../services/agent-tracing/README.md)。

## MCP 服务

MCP 网关从 [`services/mcp/config.toml`](../../services/mcp/config.toml) 读取非敏感默认值，不读取根 `config.toml` 或 Nacos 应用 TOML；它拥有独立数据库与凭据加密主密钥。秘密只由环境或部署平台注入：

| 配置 | 默认 | 说明 |
|---|---|---|
| `MCP_DATABASE_URL` | 未配置 | **必填**，MCP 专属 PostgreSQL；启动时执行 `services/mcp/migrations/`，不要与其它服务共用 |
| `MCP_CREDENTIAL_KEY_BASE64` | 未配置 | **必填**，AES-256-GCM 主密钥，标准 Base64 编码的 32 字节；轮换后旧密文不可解密 |
| `MCP_CONSUMERS_JSON` | `[]` | **必须非空**，调用方清单 `[{"consumer_id":"web","token":"…","context_signing_secret":"…"}]`；token 与签名密钥至少 32 字符 |
| `APP_MCP__SERVER__HOST` / `PORT` | `127.0.0.1` / `8092` | 容器镜像绑定 `0.0.0.0:8092`，Compose 只发布宿主机回环端口 |
| `APP_MCP__SECURITY__ALLOW_INSECURE_LOOPBACK` | `false` | 仅本地测试：允许访问 loopback 上的 HTTP MCP server；生产保持关闭 |
| `APP_MCP__GATEWAY__ALLOWED_HOSTS` | `["localhost","127.0.0.1","::1"]` | MCP endpoint 的 Host 校验；环境变量可用 JSON 数组覆盖 |
| `APP_MCP__OUTBOUND__*` | 见 `config.toml` | 远端连接/请求超时、SSE 解析前及 JSON 结果编码后字节上限与单连接工具数上限；[JSON 接收和内存限制](mcp-service.md) |

数据库连接串与主密钥在 `Debug` 输出里被隐藏。签名格式、错误码与上游安全策略见 [`mcp-service.md`](mcp-service.md)。

## 后端

| 配置 | 默认 | 说明 |
|---|---|---|
| `DATABASE_URL` | 未配置 | PostgreSQL 连接串。配置后启动时自动执行迁移，所以启动时必须可达；运行中的短暂中断才是惰性重连 |
| `REDIS_URL` | 未配置 | 会话与限流计数。数据库与 Redis **都**配置了才启用认证与管理后台，否则 `/auth/*`、`/admin/*` 返回 503，其余功能照常 |
| `NACOS_SERVER_ADDR` | 未配置 | 配置后启用 Nacos；应用启动时读取 `[nacos].app_data_id`，远端不可用或无读取权限时启动失败 |
| `NACOS_USERNAME` + `NACOS_PASSWORD` 或 `NACOS_ACCESS_TOKEN` | 未配置 | Nacos 认证凭据；username/password 必须成对设置。token 是高级用法 |
| `[nacos]` 的 `namespace_id`、`group`、`app_data_id`、`compose_data_id` | 见 `config.toml` | 项目内固定的非敏感 Nacos 定位信息；切换环境可用 `APP_CONFIG_FILE` 指定另一份配置 |
| `server.host` / `server.port` | `127.0.0.1` / `8080` | 容器里要监听 `0.0.0.0`（镜像已通过 `APP_SERVER__HOST` 设置） |
| `logging.level` / `logging.format` | `info` / `text` | `RUST_LOG` 优先；`format=json` 适合日志采集 |
| `database.max_connections` / `acquire_timeout_secs` | `10` / `3` | |
| `redis.timeout_secs` | `3` | 单次操作（含首次建立连接）的超时 |

## 认证与限流（`[auth]`）

| 配置 | 默认 | 说明 |
|---|---|---|
| `session_ttl_secs` | 7 天 | 同时是 Cookie 的 `Max-Age` |
| `cookie_name` | `session` | 与管理员的 Cookie 名必须不同 |
| `cookie_secure` | `false` | **生产（HTTPS）必须 `true`**；监听非本机地址却没开时启动会告警 |
| `login_max_failures_per_ip` / `_per_account` | 20 / 10 | 每个窗口内允许的失败次数；超限 429 且即使密码正确也拒绝 |
| `login_window_secs` | 900 | |
| `register_max_per_ip` / `register_window_secs` | 10 / 3600 | 注册请求次数（成败都算） |
| `trust_forwarded_for` | `false` | 开启后取 `X-Forwarded-For` **最右**一项作为客户端 IP。**前提**：前面有会覆盖写该头的代理，且后端不能被绕过直连；Next.js 的 rewrites 不满足，见 [部署](deployment.md) |

## 管理后台（`[admin]`）

| 配置 | 默认 | 说明 |
|---|---|---|
| `ADMIN_BOOTSTRAP_USERNAME` / `ADMIN_BOOTSTRAP_PASSWORD` | 未配置 | 同时配置才生效（也可写成 `APP_ADMIN__BOOTSTRAP_*`）。启动时创建这个管理员，**已存在则什么都不改**（绝不覆盖密码）；用户名或密码不合规会让启动失败。密码是密钥，不要写进 `config.toml` |
| `session_ttl_secs` | 12 小时 | 比普通用户短 |
| `cookie_name` | `admin_session` | 必须与 `auth.cookie_name` 不同；`Secure` 与是否信任代理头沿用 `[auth]` |
| `login_max_failures_per_ip` / `_per_account` | 10 / 5 | 比普通登录更严；管理员与普通用户的失败计数互不影响 |
| `login_window_secs` | 900 | |

## Docker Compose 部署设置

`just stack-*`、`just accounts-*`、备份、恢复和 stack smoke 命令会先通过 Rust 的 `compose-config` 开发期工具，从 `[nacos].compose_data_id`（默认 `rust-study-compose.properties`）读取 Properties，然后把白名单配置只放进当前 Docker Compose 子进程的环境。部署设置不落本地文件；Compose 使用 `--env-file /dev/null`，不会回退读取 `.env` 或 `.env.compose`。这个工具需要本机安装 Rust/Cargo。

Compose 使用与应用 TOML 相同的 Nacos server 与认证参数，但通过 `[nacos].compose_data_id` 读取独立 Data ID。`[nacos]` 的 namespace/group 与两类 Data ID 都留在项目配置文件中；`.env.local` 只存 Nacos server 地址与认证凭据，不存 PostgreSQL、Redis 或管理员密码。Nacos 不可达、认证失败、配置不存在或内容无效时，入口会在运行 Docker 前停止，不会改用本地文件。

配置文件只接受受限的字面量 `KEY=value` 行：允许空行与 `#` / `!` 注释；不执行 shell、变量插值或引号转义；拒绝未知/重复键、控制字符和类型不正确的值。必填键为 `POSTGRES_PASSWORD`、`REDIS_PASSWORD`、`AGENT_TRACING_DB_PASSWORD`、`AGENT_DB_PASSWORD`、`AGENT_WEB_SERVICE_TOKEN`、`AGENT_ADMIN_SERVICE_TOKEN`、`AGENT_TRACING_PRODUCER_TOKEN`；引导管理员凭据可选，producer 清单默认 `[]`，`AGENT_PLATFORM_ADMIN_IDS` 默认 `[1]`。MCP 的三项秘密不参与 compose-config 的必填校验，而由 `mcp-service` 在启动时检查；缺失时容器启动失败。端口必须在 1–65535，`COOKIE_SECURE` 仅接受 `true`/`false`，`LOG_FORMAT` 仅接受 `text`/`json`，`TRUSTED_PROXY_CIDR` 仅接受 IP 地址或 CIDR。

示例结构见 [`deploy/nacos/rust-study-compose.properties.example`](../../deploy/nacos/rust-study-compose.properties.example)；把内容作为 Nacos 配置发布，不要复制为本地部署环境文件。完整 stack 中 accounts 不发布端口，前面有覆盖写代理头的 nginx，因此固定启用 `trust_forwarded_for=true`；独立开发使用 `docker-compose.accounts.yml`，仅绑定回环地址并关闭该信任。

| 变量 | 说明 |
|---|---|
| `POSTGRES_PASSWORD`、`REDIS_PASSWORD` | 必填，缺失时 compose 报错 |
| `AGENT_TRACING_DB_PASSWORD` | tracing 专属 PostgreSQL 密码；与 accounts 数据库密码分开管理。该值直接用于 PostgreSQL URL，使用 URL-safe 随机字符 |
| `AGENT_DB_PASSWORD` | Agent 专属 PostgreSQL 密码（必填；URL-safe 随机字符；不要与 accounts / tracing 复用） |
| `AGENT_WEB_SERVICE_TOKEN`、`AGENT_ADMIN_SERVICE_TOKEN` | public Route Handler 与 admin nginx 的调用凭据（必填；各服务独立，至少 32 字符；浏览器不可见） |
| `AGENT_TRACING_PRODUCER_TOKEN` | Agent 投递事件的 producer token（必填；必须与 `AGENT_TRACING_PRODUCERS_JSON` 中对应条目一致） |
| `AGENT_TRACING_PRODUCERS_JSON` | producer source/project/token 清单；没有 producer 时为空数组 `[]` |
| `AGENT_PLATFORM_ADMIN_IDS` | 平台管理员 accounts ID 列表，默认 `[1]`；不自动授予组织 scope |
| `ADMIN_BOOTSTRAP_USERNAME`、`ADMIN_BOOTSTRAP_PASSWORD` | 引导管理员 |
| `COOKIE_SECURE` | HTTPS 访问时必须 `true` |
| `TRUSTED_PROXY_CIDR` | 前面有 HTTPS 终止代理时填它的地址段，默认只认直接对端 |
| `WEB_PORT`、`ADMIN_PORT`、`TRACING_UI_PORT` | 宿主机端口，默认 3000 / 8081 / 8082 |
| `MCP_DB_PASSWORD`、`MCP_CREDENTIAL_KEY_BASE64`、`MCP_CONSUMERS_JSON` | 启用 `mcp` profile 时必填；独立数据库密码、32 字节 Base64 主密钥、非空消费方清单 |
| `MCP_PORT` | 可选，MCP 网关宿主机回环端口，默认 8092 |
| `LOG_FORMAT` | `json`（默认）或 `text` |

应用 Nacos 配置使用 TOML，例如 `[database]` 下的 `url` 和 `[redis]` 下的 `url`；Compose Nacos 配置使用 Properties。两类配置共享 `[nacos]` 中的 namespace/group 和 `.env.local` 中的 server/认证参数，但读取不同 Data ID。Nacos 用户名/密码或令牌只放在 `.env.local` 或部署平台的密钥管理中，不要写进远端业务配置。

## 前端

- 前台（Next.js）：后端地址是**构建参数** `BACKEND_URL`（构建时固化）；本机开发用环境变量 `BACKEND_URL`，默认 `http://localhost:8080`。Agent 请求由运行时环境变量 `AGENT_SERVICE_URL`（默认 `http://127.0.0.1:8091`）与 `AGENT_SERVICE_TOKEN`（服务端注入，浏览器不可见）决定。
- 管理后台（Vite）：开发时 `BACKEND_PORT`（默认 8080）、`FRONTEND_ADMIN_PORT`（默认 5173）、`AGENT_SERVICE_URL` / `AGENT_SERVICE_TOKEN`；镜像运行时用 `BACKEND_UPSTREAM`、`AGENT_UPSTREAM`（默认 `agent-service:8091`）、`AGENT_SERVICE_TOKEN`、`TRUSTED_PROXY_CIDR`。
- Tracing Studio（Vite）：开发时 `AGENT_TRACING_PORT`（默认 8090）、`ACCOUNTS_PORT`（默认 8080）、`FRONTEND_TRACING_PORT`（默认 5174）；生产 nginx 把 producer `/api/v1/ingest/*` 原样转给 tracing，把管理员认证转给 accounts，把管理查询代理到 tracing。
