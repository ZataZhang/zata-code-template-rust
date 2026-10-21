# 配置

优先级从低到高：**代码默认值 < `config.toml` < 环境变量**。所有字段都有默认值，环境变量没填时配置加载也必须成功。

- 覆盖单个值：`APP_<分区>__<字段>`，双下划线表示层级，如 `APP_SERVER__PORT=9000`
- 连接串用约定俗成的名字：`DATABASE_URL`、`REDIS_URL`
- 本机的环境变量与密钥放 `.env.local`（启动时自动加载，不提交）；生产直接注入环境变量
- 指定别的配置文件：`APP_CONFIG_FILE=/path/to/config.toml`
- 含密码的字段（连接串、引导管理员密码）在 `Debug` 输出里被隐藏，不会进日志

完整的带注释示例见仓库根的 [`config.toml`](../../config.toml) 与 [`.env.example`](../../.env.example)。

## 后端

| 配置 | 默认 | 说明 |
|---|---|---|
| `DATABASE_URL` | 未配置 | PostgreSQL 连接串。配置后启动时自动执行迁移，所以启动时必须可达；运行中的短暂中断才是惰性重连 |
| `REDIS_URL` | 未配置 | 会话与限流计数。数据库与 Redis **都**配置了才启用认证与管理后台，否则 `/auth/*`、`/admin/*` 返回 503，其余功能照常 |
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

## 整套服务的 compose（`.env.compose`）

由 `docker-compose.yml` 读取，模板见 [`.env.compose.example`](../../.env.compose.example)。它把上面的后端配置翻译成容器环境变量，并固定 `trust_forwarded_for=true`（因为 backend 不发布端口、前面一定有会覆盖写该头的 nginx）。

| 变量 | 说明 |
|---|---|
| `POSTGRES_PASSWORD`、`REDIS_PASSWORD` | 必填，缺失时 compose 报错 |
| `ADMIN_BOOTSTRAP_USERNAME`、`ADMIN_BOOTSTRAP_PASSWORD` | 引导管理员 |
| `COOKIE_SECURE` | HTTPS 访问时必须 `true` |
| `TRUSTED_PROXY_CIDR` | 前面有 HTTPS 终止代理时填它的地址段，默认只认直接对端 |
| `WEB_PORT`、`ADMIN_PORT` | 宿主机端口，默认 3000 / 8081 |
| `LOG_FORMAT` | `json`（默认）或 `text` |

## 前端

- 前台（Next.js）：后端地址是**构建参数** `BACKEND_URL`（构建时固化）；本机开发用环境变量 `BACKEND_URL`，默认 `http://localhost:8080`。
- 管理后台（Vite）：开发时 `BACKEND_PORT`（默认 8080）、`FRONTEND_ADMIN_PORT`（默认 5173）；镜像运行时用 `BACKEND_UPSTREAM`、`TRUSTED_PROXY_CIDR`。
