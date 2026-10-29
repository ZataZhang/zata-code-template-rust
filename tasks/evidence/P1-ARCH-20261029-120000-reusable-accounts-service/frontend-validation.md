# 前端验证记录

日期：2026-10-29。此记录只覆盖账号 UI 包、公开站点、第二宿主及前端镜像；全栈 Compose smoke 由后端执行器单独记录。

## 包和站点检查

- `just accounts-ui-check`：通过。账号 UI TypeScript 包构建并打出 `@tpl/accounts-ui@0.1.0` tarball；独立 Vite 宿主从 tarball 冻结安装、类型检查和生产构建均通过。输出见 [`accounts-ui-check.log`](./accounts-ui-check.log)。包设为 `private: true`、`UNLICENSED`，本轮只生成本地 tarball，未发布。
- `just web-check`：通过，退出码 0；涵盖公开站点类型检查、lint 和 Next 生产构建。最终文案修订后另运行 `just check-i18n && pnpm build`，137 组中英文 key 对齐且构建通过。命令原始输出未单独保存。
- `just web-admin-check`：通过，退出码 0；类型、lint、knip、格式检查和 Vite 生产构建通过。命令原始输出未单独保存。
- `git diff --check -- frontend-public examples/accounts-host packages/accounts-ui`：通过。

## 真实浏览器验证

- `SHOT_DIR="$(pwd)/tasks/evidence/P1-ARCH-20261029-120000-reusable-accounts-service" just web-e2e`：通过，退出码 0。公开站点 20/20、管理后台 33/33、打包 tarball 第二宿主 7 项通过，包括注册后重新请求 `/auth/me` 得到 200、退出后得到 401、错误密码返回 401 且没有登录态。完整输出见 [`web-e2e.log`](./web-e2e.log)。
- 第二宿主 popstate 修订后，再用本机真实 `accounts-service`（测试 PostgreSQL、Redis DB 7）运行 `pnpm typecheck && pnpm build` 和真实浏览器 E2E：全部通过。E2E 9 项覆盖浏览器后退/前进后 URL 与登录/注册表单同步、tarball 注册、fresh `/auth/me`、退出后 401、错密码 401、无残留会话和无运行时错误。测试账号与引导管理员已清理。输出见 [`accounts-host-popstate-e2e.log`](./accounts-host-popstate-e2e.log)，服务日志见 [`accounts-host-service.log`](./accounts-host-service.log)。
- 最终前端镜像容器的 Playwright 检查通过：`/login` HTTP 200、共享注册表单可渲染、4 个 Geist WOFF2 响应均为 200、浏览器运行时错误为 0。输出见 [`web-docker-page-smoke.log`](./web-docker-page-smoke.log)。

最终人工查看截图仅保留 [`public-account-session.png`](./public-account-session.png) 和 [`second-host-session.png`](./second-host-session.png)。

## 前端 Docker 镜像

- 构建命令：`docker build -f frontend-public/Dockerfile --build-arg BACKEND_URL=http://accounts:8080 -t tpl-web:accounts-module-20261029 .`
- 最终镜像：`tpl-web:accounts-module-20261029`，ID `sha256:4642cf2cd48cabe5dd4b1d4dff582414cb11057564a6cfcb0523621c0c9b59e3`，72,870,005 bytes。inspect 输出见 [`web-image-inspect.log`](./web-image-inspect.log)。Docker build 的完整 stdout 未单独保存；BuildKit 缓存命中后的最终构建退出码为 0。
- Docker 构建输入（`frontend-public/` 与 `packages/accounts-ui/`，排除 node_modules、`.next`、`dist`、测试截图及 tarball）的文件哈希汇总：`e7ee4ae4bc1285cc9395d47ad408eb87079bcc29dcfd3ca6f15b3e07a6c9fd1a`。
- Google Fonts 网络不稳定会导致 `next/font/google` 构建失败。已将 Vercel 官方 `geist@1.7.2` 的 Geist Sans/Mono 可变 WOFF2 文件改为 `next/font/local`，保留 SIL OFL 1.1 许可证及来源说明，见 [`frontend-public/app/fonts/README.md`](../../../frontend-public/app/fonts/README.md)。最终本地构建和 Docker 构建均通过。

早期一次第二宿主运行因退出后仍保持注册模式而未能触发登录请求；修正退出模式、浏览器历史同步和错误密码断言后，上述独立 E2E 全部通过。[`accounts-host-e2e.log`](./accounts-host-e2e.log) 记录修正后的 7 项通过；浏览器历史修订后的最终结果以 9 项 popstate 日志为准。
