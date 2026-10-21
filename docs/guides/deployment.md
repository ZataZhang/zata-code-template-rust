# 部署

这份指南覆盖三件事：用 `docker-compose.yml` 一条命令跑起整套服务、上生产要改什么、前面再加一层 HTTPS 代理时怎么配。配置项详见 [`configuration.md`](configuration.md)，数据库备份见 [`backup.md`](backup.md)。

## 1. 本机或单机：一条命令

```bash
just stack-up
```

首次会生成 `.env.compose`（权限 600，已被 `.gitignore` 忽略）：密钥是随机值，并打印管理员账号。随后构建四个镜像并等待全部健康，约几分钟。

| 入口 | 地址 | 说明 |
|---|---|---|
| 前台站点 | `http://localhost:3000` | 经 `edge`（nginx）进入；`/api/*` 直接转给后端 |
| 管理后台 | `http://localhost:8081` | 自带 nginx；用 `.env.compose` 里的引导管理员登录 |
| 后端 | 不对外发布 | 只能经上面两个入口访问 |

端口在 `.env.compose` 的 `WEB_PORT`、`ADMIN_PORT` 里改。常用命令：`just stack-logs [服务名]`、`just stack-down`（保留数据）、`just stack-reset`（**清空数据库与会话**）。

### 服务关系

```text
浏览器 → edge(nginx) ─ /api/ ──────────────→ backend:8080 → postgres, redis
              └─ /  → web(Next.js)
浏览器 → admin(nginx) ─ /api/ ─────────────→ backend:8080
```

- **backend 的健康检查**请求 `/ready`，同时检查 PostgreSQL 与 Redis；任何一个不可用约 30 秒后变为 `unhealthy`，恢复后自动回到 `healthy`。`web`、`admin` 等它健康才启动。
- 启动时 backend 自动执行数据库迁移；配置了数据库就要求启动时可达。
- Redis 开了持久化（AOF），重启后用户不会被全部踢下线。

## 2. 为什么前台前面必须有 nginx（重要）

后端按 IP 限流，依赖 `X-Forwarded-For` 最右一项（`trust_forwarded_for=true`）。这个头只有在**有一层自己的代理覆盖写它**时才可信：

- **Next.js 的 rewrite 代理不会追加这个头**，客户端发来的值原样透传。直接暴露 Next.js 再开 `trust_forwarded_for`，攻击者每次换一个伪造 IP 就能绕过限流（已在真实部署上复现：伪造头时 21 次登录失败全部通过；放上 `edge` 之后第 21 次起 429）。
- 不开 `trust_forwarded_for` 又会让所有用户共用代理的那一个 IP 额度。

所以：`web` 不发布端口，浏览器只能经 `edge` 或 `admin` 访问；这两个 nginx 都用 `proxy_set_header X-Forwarded-For $remote_addr` **覆盖**客户端的值；backend 不发布端口，外部无法绕过它们直连。**改动这些时不要破坏这三点。**

## 3. 上生产

在 `.env.compose`（或部署平台的环境变量）里至少确认：

| 项 | 要求 |
|---|---|
| `POSTGRES_PASSWORD`、`REDIS_PASSWORD`、`ADMIN_BOOTSTRAP_PASSWORD` | 强随机值，交给密钥管理，不要提交 Git。缺失时 compose 直接报错而不是用默认值 |
| `COOKIE_SECURE=true` | **必须**，且浏览器必须经 HTTPS 访问，否则会话 Cookie 可能明文传输（管理员 Cookie 沿用这个开关） |
| `TRUSTED_PROXY_CIDR` | 前面有 HTTPS 终止代理时填它的地址段（见下） |
| 备份 | 配好定时备份并演练过恢复，见 [`backup.md`](backup.md) |
| 管理员密码 | 引导账号只在**不存在**时创建，事后改环境变量不会改密码；目前没有轮换机制 |

## 4. 前面再加一层 HTTPS 代理（Traefik / Caddy / 云负载均衡）

浏览器 → HTTPS 代理 → `edge` / `admin`。这时 nginx 看到的对端是 HTTPS 代理，所以要告诉它信任谁：

```env
# .env.compose：HTTPS 代理所在网络的地址段
TRUSTED_PROXY_CIDR=172.18.0.0/16
```

nginx 会在这个地址段的对端发来的 `X-Forwarded-For` 里还原出真实客户端地址（`real_ip_recursive`），再覆盖写给 backend。**只填你自己的代理所在的段**：填 `0.0.0.0/0` 等于信任所有人，客户端又能伪造来源 IP 了。不填（默认 `127.0.0.1`）则只认直接连进来的对端。

HTTPS 代理需要把 `edge` 与 `admin` 当作两个站点（常见做法是两个域名）转发，并传 `X-Forwarded-For`（Traefik、Caddy 默认会传）。把 `docker-compose.yml` 里 `edge`、`admin` 的 `ports` 改成只在内网可达（去掉发布，或绑 `127.0.0.1`）。

验证办法：用同一个真实客户端连续输错密码，应在设定次数后得到 429，而换一个 IP 的客户端不受影响。

## 5. 单独部署各个镜像

| 镜像 | 构建 | 运行时配置 |
|---|---|---|
| 后端 | `just docker-build` | `DATABASE_URL`、`REDIS_URL` 等，见 [`configuration.md`](configuration.md)；监听 8080，以非 root 运行 |
| 前台 | `just web-docker-build <后端地址>` | 后端地址是**构建参数**（Next.js 把 rewrites 固化进产物），改了要重新构建 |
| 管理后台 | `just web-admin-docker-build` | 运行时环境变量 `BACKEND_UPSTREAM`（默认 `backend:8080`）、`TRUSTED_PROXY_CIDR` |

镜像里没有 `curl`；编排平台的存活/就绪探针直接请求后端的 `/live`、`/ready`。

## 6. 排错

- `just stack-up` 报 `请在 .env.compose 里设置 …`：该密钥为空。删除 `.env.compose` 重新生成，或补上。
- backend 一直 `unhealthy`：`just stack-logs backend`；多半是数据库/Redis 密码与数据卷里已有的不一致（改过密码但没清数据卷：`just stack-reset` 会**清空数据**，生产请改用 `ALTER USER` 同步密码）。
- 前台能打开但登录 502：`edge` 解析不到 backend（它只在 Compose 网络里可见，不要单独 `docker run`）。
- 登录时报「尝试次数过多」：被限流了，窗口（默认 15 分钟）过后自动恢复；本机反复试错可以在 `config.toml` 调大上限。
