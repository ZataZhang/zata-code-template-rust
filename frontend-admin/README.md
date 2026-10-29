# 管理后台前端

本目录是管理后台（Vite + React 19 + TanStack Router + Tailwind v4 + shadcn/ui），基于
[satnaing/shadcn-admin](https://github.com/satnaing/shadcn-admin) 精简而来。它是**独立的 pnpm 项目**，只通过 `/api/*` HTTP 与 accounts-service 通信。

目前只有两个页面：管理员登录、用户管理（搜索、状态筛选、分页、启用/停用）。**只保留有真实后端支撑的页面**——新增页面应当先有后端接口，不要把演示页面加回来。

## 本地开发

```bash
# 仓库根目录
just web-admin-install   # 首次：按 pnpm-lock.yaml 安装依赖
just accounts-up         # 另开终端：启动账号服务（需要 DATABASE_URL、REDIS_URL 以及引导管理员）
just web-admin-dev       # 打开 http://localhost:5173
```

管理员只能由后端启动时的环境变量创建：

```bash
export ADMIN_BOOTSTRAP_USERNAME=admin ADMIN_BOOTSTRAP_PASSWORD='至少8位的初始密码'
```

开发服务器把 `/api/*` 代理到 `http://localhost:8080` 的账号服务（`BACKEND_PORT` 可改，`FRONTEND_ADMIN_PORT` 改本站端口）。

## 检查与测试

| 命令                   | 内容                                                                                                       |
| ---------------------- | ---------------------------------------------------------------------------------------------------------- |
| `just web-admin-check` | 类型 + lint + 未使用代码（knip）+ 格式 + 生产构建 + i18n key 对齐                                          |
| `just web-admin-test`  | 浏览器组件测试（vitest + Playwright Chromium；首次先 `pnpm test:browser:install`）                         |
| `just web-e2e`         | 真实 Chrome 端到端（含公开站点）：登录、搜索/筛选、禁用/启用及其对普通用户的真实影响、语言切换、退出、限流 |

## 与后端的契约

- 错误体是 `{"error": {"code": "...", "message": "..."}}`。界面按 `code` 查 `apiErrors.<code>` 翻译，没有对应翻译才退回后端的原文（`src/lib/api-error-message.ts`）。新增后端错误码时同步补 `src/locales/{en,zh}.json`。
- 认证用后端签发的 HTTP-only `admin_session` Cookie；前端读不到令牌，登录态以 `/admin/auth/me` 为准。
- 用户 ID 是字符串；创建时间是 UTC 的 ISO 8601 字符串。
- 改后端 `/admin/*` 契约时必须同步改 `src/api/` 并重跑 `just web-e2e`：前端类型是手写的，编译器发现不了契约漂移。

## 项目结构

```text
frontend-admin/
├── src/
│   ├── api/          # HTTP 客户端与接口函数（auth.ts、users.ts）
│   ├── components/   # 布局、通用组件与 shadcn/ui
│   ├── features/     # auth（登录）、users（用户管理）、errors
│   ├── i18n/ locales/ # react-i18next 与 en/zh 文案
│   ├── routes/       # TanStack Router 文件路由（routeTree.gen.ts 自动生成）
│   └── stores/       # 登录态
├── e2e/              # 真实浏览器端到端脚本（由 scripts/web-e2e.sh 调用）
├── Dockerfile  nginx.conf
```

## 容器化

```bash
just web-admin-docker-build
docker run -p 8081:80 -e BACKEND_UPSTREAM=accounts:8080 tpl-admin:local
```

镜像用 nginx 托管静态产物，`/api/*` 由 nginx 反向代理到 `BACKEND_UPSTREAM`（运行时环境变量，默认 `accounts:8080`），所以账号服务不需要开放 CORS。

**注意**：后端默认只信任 TCP 对端地址做限流，经过这个代理后它看到的是 nginx 的地址，所有管理员会共用同一个 IP 额度。部署在代理之后时要设置 `APP_AUTH__TRUST_FORWARDED_FOR=true`，并确保后端不能被绕过直连。生产环境（HTTPS）必须设置 `APP_AUTH__COOKIE_SECURE=true`（管理员 Cookie 沿用这个开关）。
