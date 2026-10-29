# 前台站点（frontend-public）

面向终端用户的站点：营销页、注册、登录，以及登录后的受保护区域。基于 Next.js 16（App Router）+ React 19 + Tailwind CSS v4 + shadcn/ui，中英双语（next-intl）。

它是后端之外的**独立 Web 客户端**，只通过 `/api/*` 的 HTTP 调用 `accounts-service`，不依赖后端 Rust crate。

## 运行

```bash
just web-install        # 安装依赖（按 pnpm-lock.yaml 精确安装）
just accounts-up        # 终端 1：独立启动账号服务，http://localhost:8082
BACKEND_URL=http://localhost:8082 just web-dev  # 终端 2：启动前端，默认 http://localhost:3000
```

本机完整开发流程可直接运行仓库根目录的 `just run`，它会启动 API 和两个站点并打印 URL。`http://localhost:8080` 是 API 根地址，没有网页；健康检查为 `/ready`。若单独使用 Docker 账号服务，则运行 `just accounts-up` 后再按上面的命令将 `BACKEND_URL` 指向 8082。`/api/*` 由 Next.js rewrites 转发到该服务；`BACKEND_URL` 在生产构建时生效，见下文「部署」。账号 UI 包构建产物缺失时，先运行 `just accounts-ui-check`。

## 检查

```bash
just web-check          # 类型检查 + lint + 生产构建
just web-e2e            # 真实浏览器端到端测试（需要本机装有 Chrome）
```

`just web-e2e` 会自动启动 PostgreSQL、Redis、账号服务、公开站点、管理后台和独立 React 宿主（用空闲端口和 Redis 6 号库），用 Chrome 验证公开账号注册/登录/退出、管理员流程及第二宿主通过真实 tarball 注册账号、重新读取 `/auth/me`、退出后 `/auth/me` 返回 401 和错误密码被拒绝。结束时清理进程与测试数据；可用 `SHOT_DIR` 指定截图目录。公开站点脚本在 `e2e/auth.e2e.mjs`。

## 可复用账号页面包

`/login` 与 `/register` 页面消费仓库内的 `@tpl/accounts-ui` 包。表单校验和普通账号 API 客户端归包所有；本应用注入 `/api` 前缀、next-intl 文案、Next.js 路由与 toast 提示。包使用自带样式，可通过 CSS 自定义属性和 `className` 适配宿主，不依赖 Next.js、宿主路由或管理员认证。

包独立构建为 TypeScript ESM，并用 `pnpm run pack:tarball` 生成版本归档；公开站点的本地开发依赖指向包目录，Docker 使用仓库根目录为构建上下文。独立宿主从真实 tarball 安装的示例见 [`examples/accounts-host/README.md`](../examples/accounts-host/README.md)。完整服务地址配置、代理和 Cookie 边界见[账号模块接入指南](../docs/guides/accounts-integration.md)。

站点以本地 `next/font/local` 加载 Geist Sans 和 Geist Mono，字体来自 Vercel `geist@1.7.2`；源文件路径和 SIL OFL 1.1 许可证记录在 [`app/fonts/README.md`](app/fonts/README.md)。

## 与后端的接口契约

| 前端调用 | 账号服务接口 | 说明 |
|---|---|---|
| `register()` | `POST /auth/register` → 紧接着 `POST /auth/login` | 后端注册**不会自动登录**，所以前端注册成功后再登录一次 |
| `login()` | `POST /auth/login`，`{email, password}` | 成功时后端设置 HttpOnly 的 `session` Cookie |
| `logout()` | `POST /auth/logout` | 幂等 |
| `getCurrentSession()` | `GET /auth/me` | 受保护区域的布局用它判断是否已登录 |

- 返回结构是 `{"user": {"id", "email"}}`（没有显示名，后端目前没有这个字段）。
- 错误响应是 `{"error": {"code", "message"}}`。前端按 **`code`** 取翻译（`messages/*.json` 的 `apiErrors`），这样提示语言跟随界面语言；没有对应翻译的错误码才用兜底文案。
- 被限流时后端返回 `429`，带 `Retry-After` 头，前端据此显示「请 N 分钟后再试」。
- 密码规则（至少 8 个字符、最多 128 字节）由 `@tpl/accounts-ui` 提供，与账号服务保持一致。
- `/auth/*` 的 401 是预期内的业务结果（密码错误、未登录），**不会**触发全局的「跳转登录页」，否则登录失败时页面会被整体刷新、错误提示丢失。

## 部署

```bash
just web-docker-build http://accounts:8080    # 根目录为构建上下文，包与站点一起打包
```

- **账号服务地址必须在构建时给**：Next.js 把 rewrites 的目标固化进 `.next/routes-manifest.json`，运行时再改环境变量不会生效。
- 镜像是 Next.js standalone 产物，以非 root 用户运行，监听 3000。
- **限流依赖客户端 IP，而 Next.js 不会帮你写 `X-Forwarded-For`**：它的 rewrite 代理把客户端发来的头原样透传。所以只靠这个镜像，后端看到的 TCP 对端永远是 Next.js（所有用户共用一个 IP 额度）；这时若开 `APP_AUTH__TRUST_FORWARDED_FOR=true`，攻击者每次换一个伪造 IP 就能绕过限流。**生产部署要在前面放一层会覆盖写该头的 nginx / Traefik / 云负载均衡**，参考根目录 `docker-compose.yml` 的 `edge` 服务与 `docs/guides/deployment.md`。
- 生产环境（HTTPS）后端必须设置 `APP_AUTH__COOKIE_SECURE=true`，否则会话 Cookie 可能经明文传输。

## 目录

```
app/(marketing)/   营销页（首页、功能、定价、关于）
app/(auth)/        登录、注册
app/(app)/app/     登录后的区域（dashboard、settings），布局里用 /auth/me 做守卫
components/        布局与 shadcn/ui 组件
components/accounts/  Next.js 路由、next-intl、导航和 toast 与账号 UI 包之间的适配
lib/api/           账号服务客户端适配、错误码→翻译
packages/accounts-ui/  独立账号 React 页面、校验规则与 HTTP 客户端
messages/          en.json、zh.json
e2e/               真实浏览器端到端测试
../examples/accounts-host/  从实际 npm tarball 安装包的第二宿主
```

> 给 AI 代理：这个 Next.js 版本有破坏性变更，写代码前先读 `node_modules/next/dist/docs/` 里的相关指南（见本目录 `AGENTS.md`）。

## 关于管理后台

管理员 UI 位于独立的 `frontend-admin/` 项目，通过 `/admin/*` 接口访问同一个 `accounts-service`。普通用户认证包只包含 `/auth/*` 和 `session` Cookie，不复用管理员页面、Cookie 或会话存储。
