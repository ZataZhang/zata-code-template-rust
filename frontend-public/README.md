# 前台站点（frontend-public）

面向终端用户的站点：营销页、注册、登录，以及登录后的受保护区域。基于 Next.js 16（App Router）+ React 19 + Tailwind CSS v4 + shadcn/ui，中英双语（next-intl）。

它是后端之外的**独立 Web 客户端**，只通过 `/api/*` 的 HTTP 调用 Rust 后端（`crates/api`），不依赖后端的任何代码。

## 运行

```bash
just web-install        # 安装依赖（按 pnpm-lock.yaml 精确安装）
just run                # 终端 1：启动后端，默认 http://localhost:8080（登录/注册需要 DATABASE_URL 和 REDIS_URL，见根目录 README）
just web-dev            # 终端 2：启动前端，默认 http://localhost:3000
```

`/api/*` 由 Next.js 的 rewrites 转发到后端。后端地址默认 `http://localhost:8080`，用 `BACKEND_URL` 覆盖（**构建时**生效，见下文「部署」）。

## 检查

```bash
just web-check          # 类型检查 + lint + 生产构建
just web-e2e            # 真实浏览器端到端测试（需要本机装有 Chrome）
```

`just web-e2e` 会自动启动 PostgreSQL / Redis / 后端 / 前端（用空闲端口和独立的 Redis 库），用 Chrome 走完：注册（含客户端校验）、自动登录、Cookie 属性、刷新保持登录、退出、登录失败提示、连续失败触发限流（中英文提示）、重复注册、账号间互不影响，结束后清理进程和测试数据。脚本在 `e2e/auth.e2e.mjs`。

## 与后端的接口契约

| 前端调用 | 后端接口 | 说明 |
|---|---|---|
| `register()` | `POST /auth/register` → 紧接着 `POST /auth/login` | 后端注册**不会自动登录**，所以前端注册成功后再登录一次 |
| `login()` | `POST /auth/login`，`{email, password}` | 成功时后端设置 HttpOnly 的 `session` Cookie |
| `logout()` | `POST /auth/logout` | 幂等 |
| `getCurrentSession()` | `GET /auth/me` | 受保护区域的布局用它判断是否已登录 |

- 返回结构是 `{"user": {"id", "email"}}`（没有显示名，后端目前没有这个字段）。
- 错误响应是 `{"error": {"code", "message"}}`。前端按 **`code`** 取翻译（`messages/*.json` 的 `apiErrors`），这样提示语言跟随界面语言；没有对应翻译的错误码才用兜底文案。
- 被限流时后端返回 `429`，带 `Retry-After` 头，前端据此显示「请 N 分钟后再试」。
- 密码规则（至少 8 个字符、最多 128 字节）与后端一致，定义在 `lib/api/auth.ts`；改后端规则时要同步这里。
- `/auth/*` 的 401 是预期内的业务结果（密码错误、未登录），**不会**触发全局的「跳转登录页」，否则登录失败时页面会被整体刷新、错误提示丢失。

## 部署

```bash
just web-docker-build http://backend:8080     # 等价于 docker build --build-arg BACKEND_URL=... -t tpl-web:local frontend-public
```

- **后端地址必须在构建时给**：Next.js 把 rewrites 的目标固化进 `.next/routes-manifest.json`，运行时再改环境变量不会生效。
- 镜像是 Next.js standalone 产物，以非 root 用户运行，监听 3000。
- **限流依赖客户端 IP，而 Next.js 不会帮你写 `X-Forwarded-For`**：它的 rewrite 代理把客户端发来的头原样透传。所以只靠这个镜像，后端看到的 TCP 对端永远是 Next.js（所有用户共用一个 IP 额度）；这时若开 `APP_AUTH__TRUST_FORWARDED_FOR=true`，攻击者每次换一个伪造 IP 就能绕过限流。**生产部署要在前面放一层会覆盖写该头的 nginx / Traefik / 云负载均衡**，参考根目录 `docker-compose.yml` 的 `edge` 服务与 `docs/guides/deployment.md`。
- 生产环境（HTTPS）后端必须设置 `APP_AUTH__COOKIE_SECURE=true`，否则会话 Cookie 可能经明文传输。

## 目录

```
app/(marketing)/   营销页（首页、功能、定价、关于）
app/(auth)/        登录、注册
app/(app)/app/     登录后的区域（dashboard、settings），布局里用 /auth/me 做守卫
components/        布局与 shadcn/ui 组件
lib/api/           与后端通信：client（错误解析）、auth（认证接口）、use-api-error-message（错误码→翻译）
messages/          en.json、zh.json
e2e/               真实浏览器端到端测试
```

> 给 AI 代理：这个 Next.js 版本有破坏性变更，写代码前先读 `node_modules/next/dist/docs/` 里的相关指南（见本目录 `AGENTS.md`）。

## 关于管理后台

Python 版模板还有一个管理后台（`frontend-admin`）。它依赖管理员账号、`/admin/*` 接口和运行轨迹接口，这些后端能力在 Rust 版里**尚未迁移**，所以没有复制它。见根目录的 `docs/migration-status.md`。
