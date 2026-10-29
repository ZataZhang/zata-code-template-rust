# 账号服务接入

`accounts-service` 提供注册、登录、当前用户查询、退出和管理员接口。它拥有普通用户表、管理员表、账号迁移和 Redis 会话。其他应用通过 HTTP 使用账号 API，不依赖账号服务内部 Rust crate，也不直接读写它的 PostgreSQL 或 Redis。

## 两种部署选择

### 每个应用各自部署账号服务

应用独立构建并运行同一个 `accounts-service` 镜像，配置自己的 PostgreSQL 和 Redis。它复用同一份实现，但数据、会话和管理员账号分开。适合不同应用的账号集合必须隔离的情况。

### 多个应用明确共享一个账号域

多个前端或业务后端通过各自的同源网关访问同一个账号服务实例。这样它们使用同一组普通账号；管理员也是这个账号域里的管理员。当前 schema 没有 `tenant_id` 或 `app_id`，因此无法隔离应用账号、管理员和授权范围。需要租户或应用级隔离时，先设计并实现数据模型和权限校验，再共享实例。

无论选择哪种方式，数据库和 Redis 都由账号服务拥有。其他服务不得对账号表执行查询或迁移，也不得自行解析 Redis 会话键。

## 本地独立运行

`just accounts-up` 只启动账号服务及其 PostgreSQL、Redis 依赖，不启动公开站点或管理后台。开发模式只把 API 发布到 `127.0.0.1:8082`，并关闭 `X-Forwarded-For` 信任：

```bash
just accounts-up
curl -i http://127.0.0.1:8082/live
curl -i http://127.0.0.1:8082/ready
curl -i -c account-session.txt -X POST http://127.0.0.1:8082/auth/register \
  -H 'Content-Type: application/json' \
  -d '{"email":"you@example.test","password":"correct-horse-9"}'
curl -i -c account-session.txt -X POST http://127.0.0.1:8082/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"you@example.test","password":"correct-horse-9"}'
curl -i -b account-session.txt http://127.0.0.1:8082/auth/me
curl -i -b account-session.txt -X POST http://127.0.0.1:8082/auth/logout \
  -H 'Content-Type: application/json' -d '{}'
just accounts-down
```

`accounts-up` 从 Nacos 的 `config.toml` `[nacos].compose_data_id` 读取 PostgreSQL、Redis 与可选管理员引导配置，默认 Data ID 为 `rust-study-compose.properties`。`.env.local` 仅保存 Nacos 地址和认证凭据。请使用自己控制的邮箱；这条示例会在本地数据库创建真实账号。

独立模式与完整站点模式是**替代运行方式**：两者默认都用 Compose 项目名 `tpl` 和相同的 Nacos Compose Data ID。在完整站点运行时执行 `accounts-up` 会重建同一 `accounts` 容器、发布本机 API 端口并关闭代理头信任；执行 `accounts-down` 会停止完整站点正在使用的账号服务。需要切换时按所选模式重新启动服务。完整站点由 `just stack-up` 启动 nginx 代理，且不直接发布账号服务端口。

如需两种模式并存，给它们不同的 `COMPOSE_PROJECT_NAME`；独立模式还可用一次性的 `COMPOSE_ACCOUNTS_PORT` 覆盖回环端口。例如：

```bash
COMPOSE_PROJECT_NAME=accounts-dev COMPOSE_ACCOUNTS_PORT=28082 just accounts-up
```

该命令仍使用同一个远端 Nacos Compose 配置，但在 `accounts-dev` 项目中创建独立的数据卷。需要每个应用使用不同数据库和 Redis 密码时，为该应用准备一份项目配置文件，在 `[nacos]` 中指定它自己的 namespace/group 与 Data ID，并通过 `APP_CONFIG_FILE=/path/to/app-config.toml` 选择这份文件；不要把密码放入本地 Compose 文件。

## 在应用里接入

将账号 API 放在应用自己的同源路径下，例如 `/api/*`。让 nginx 把该路径去掉 `/api` 前缀后转给 `accounts-service:8080`，并覆盖写 `X-Forwarded-For`。`docker-compose.yml` 和 `deploy/edge.nginx.conf` 展示了这个配置。完整部署中的账号服务不发布主机端口，外部请求必须经过可信代理。

浏览器只访问应用自己的来源，使用 `HttpOnly` Cookie；不要把会话令牌存进 localStorage，也不要开启宽泛 CORS。不同应用的不同 host 会有各自的 host-only Cookie 和浏览器登录态。账号服务支持现有普通用户 `/auth/*` 与管理员 `/admin/*` API，二者的账号表、Cookie 名、Redis 命名空间和限流前缀彼此隔离。

普通用户前端可复用账号页面包，包名、导出内容、宿主配置和 `npm pack` 流程见 [`packages/accounts-ui/README.md`](../../packages/accounts-ui/README.md)。第二宿主示例见 [`examples/accounts-host/README.md`](../../examples/accounts-host/README.md)。

## 其他后端需要当前用户身份时

当收到带普通用户 Cookie 的请求时，业务后端可将原始 Cookie 通过可信内网连接转发给账号服务的 `GET /auth/me`，验证会话后使用响应中的用户 ID。Cookie 是 bearer 凭据：只向账号服务发送，使用 TLS 或受控内网，并且不得写入日志、错误消息或追踪属性。

严禁信任浏览器自报的 `X-User-ID`，也不要把管理员 Cookie 用于普通用户校验。管理员需要时使用独立 `/admin/auth/me` 契约及其 Cookie。当前服务没有 JWT/OIDC、跨域 SSO 或服务间用户令牌；没有原始用户会话的后台任务不能假装成已认证用户。

## 配置与升级

独立部署账号服务镜像时，需要配置 `DATABASE_URL`、`REDIS_URL`、监听地址和端口，以及可选管理员引导凭据。账号服务启动时执行 `services/accounts/migrations/` 中的迁移，因此它的数据库必须在启动时可达。不要让其他应用的启动过程执行账号迁移。

Rust 账号业务依赖本仓库里的账号私有 crate。跨仓复用优先部署账号服务镜像并调用稳定 HTTP 契约；只有在同一服务内组装时才直接复用源码。公开认证 UI 通过版本化 npm 包复用，并在升级时查看包文档及 API 兼容说明。

当前外部 API 契约保持 `/auth/*` 和 `/admin/*`。跨域或多租户集成需要先增加明确的域名、CSRF、身份和数据隔离设计，本指南不代表当前实现支持这些模式。
