# 配置

优先级从低到高：**代码默认值 < `config.toml` < 可选 Nacos TOML < 环境变量**。未启用 Nacos 时，配置加载不访问外部服务。

- 覆盖单个值：`APP_<分区>__<字段>`，双下划线表示层级，如 `APP_SERVER__PORT=9000`
- 连接串用约定俗成的名字：`DATABASE_URL`、`REDIS_URL`
- 本机仅将 Nacos 地址和认证凭据放 `.env.local`（启动时自动加载，不提交）；生产通过环境变量或部署平台密钥注入
- `NACOS_SERVER_ADDR` 配置后，服务通过 Nacos 3.x Client API 读取项目 `[nacos]` 中指定的应用 TOML；定位信息不是环境变量，连接地址和凭据不会存入 Nacos。非本机 Nacos 地址必须使用 HTTPS
- 指定别的配置文件：`APP_CONFIG_FILE=/path/to/config.toml`
- 含密码的字段（连接串、引导管理员密码）在 `Debug` 输出里被隐藏，不会进日志

完整的带注释示例见仓库根的 [`config.toml`](../../config.toml) 与 [`.env.example`](../../.env.example)。

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

配置文件只接受受限的字面量 `KEY=value` 行：允许空行与 `#` / `!` 注释；不执行 shell、变量插值或引号转义；拒绝未知/重复键、控制字符和类型不正确的值。必填键为 `POSTGRES_PASSWORD`、`REDIS_PASSWORD`；引导管理员凭据可选。端口必须在 1–65535，`COOKIE_SECURE` 仅接受 `true`/`false`，`LOG_FORMAT` 仅接受 `text`/`json`，`TRUSTED_PROXY_CIDR` 仅接受 IP 地址或 CIDR。

示例结构见 [`deploy/nacos/rust-study-compose.properties.example`](../../deploy/nacos/rust-study-compose.properties.example)；把内容作为 Nacos 配置发布，不要复制为本地部署环境文件。完整 stack 中 accounts 不发布端口，前面有覆盖写代理头的 nginx，因此固定启用 `trust_forwarded_for=true`；独立开发使用 `docker-compose.accounts.yml`，仅绑定回环地址并关闭该信任。

| 变量 | 说明 |
|---|---|
| `POSTGRES_PASSWORD`、`REDIS_PASSWORD` | 必填，缺失时 compose 报错 |
| `ADMIN_BOOTSTRAP_USERNAME`、`ADMIN_BOOTSTRAP_PASSWORD` | 引导管理员 |
| `COOKIE_SECURE` | HTTPS 访问时必须 `true` |
| `TRUSTED_PROXY_CIDR` | 前面有 HTTPS 终止代理时填它的地址段，默认只认直接对端 |
| `WEB_PORT`、`ADMIN_PORT` | 宿主机端口，默认 3000 / 8081 |
| `LOG_FORMAT` | `json`（默认）或 `text` |

应用 Nacos 配置使用 TOML，例如 `[database]` 下的 `url` 和 `[redis]` 下的 `url`；Compose Nacos 配置使用 Properties。两类配置共享 `[nacos]` 中的 namespace/group 和 `.env.local` 中的 server/认证参数，但读取不同 Data ID。Nacos 用户名/密码或令牌只放在 `.env.local` 或部署平台的密钥管理中，不要写进远端业务配置。

## 前端

- 前台（Next.js）：后端地址是**构建参数** `BACKEND_URL`（构建时固化）；本机开发用环境变量 `BACKEND_URL`，默认 `http://localhost:8080`。
- 管理后台（Vite）：开发时 `BACKEND_PORT`（默认 8080）、`FRONTEND_ADMIN_PORT`（默认 5173）；镜像运行时用 `BACKEND_UPSTREAM`、`TRUSTED_PROXY_CIDR`。
