# 独立代码审查报告

## 验收范围

依据 `tasks/archive/P1-ARCH-20261029-120000-reusable-accounts-service.md` 评审：把现有认证与管理员账号能力迁移为首个可独立部署的 `accounts-service`，通过可复用 React tarball 供公开站点和第二宿主接入，并保持既有数据迁移、普通用户/管理员隔离、代理限流和 API 行为。

本轮只交付一个业务服务；不把“网关 + accounts”描述成多个业务微服务。没有新增训练或运行追踪服务，也没有新增 SSO、多租户或跨域 Cookie 能力。

## 发现

无未解决的需求、安全、行为、架构、验证或文档缺陷。实现与文档中的早期问题（独立宿主退出后仍停留在注册模式、旧部署地址和旧迁移路径、沿用模块化单体介绍）已在最终源码中修正；最终验证覆盖修正后的源码。

## 验证证据

- `just ci`：通过；Rust lint、工作区测试及文档构建通过，记录见 `just-ci.log`。
- `just test real`：通过；真实 PostgreSQL / Redis 测试中，accounts 4 项、infrastructure 11 项通过，记录见 `just-test-real.log`。
- `just accounts-ui-check`、`just web-check`、`just web-admin-check`：通过。账号 tarball 在独立宿主冻结安装、类型检查和构建通过；前后端类型检查、lint、格式检查和构建通过，记录见 `accounts-ui-check.log` 与 `frontend-validation.md`。
- 真实 Chrome E2E：公开站点 20/20、管理后台 33/33、第二宿主 tarball 7/7 通过；浏览器历史同步修订后第二宿主又有 9/9 通过，包含正确的浏览器后退/前进、真实注册、登录失败和退出会话断言，记录见 `web-e2e.log` 和 `accounts-host-popstate-e2e.log`。
- `accounts-service` Docker 镜像构建、隔离 Compose 启动与 HTTP 检查通过；accounts、web、admin 镜像均成功构建。web 镜像页面与字体请求检查通过，记录见 `accounts-up.log`、`accounts-container-http.log`、`admin-image-build.log`、`web-image-inspect.log` 和 `web-docker-page-smoke.log`。
- 隔离完整 Compose 启动后，edge、web、admin、accounts、PostgreSQL、Redis 均健康。`stack-smoke.log` 中代理、注册登录、管理员停用、Cookie 隔离、伪造 `X-Forwarded-For` 限流及备份恢复检查全部通过。真实 PostgreSQL 停止时 `/api/ready` 从 200 变为 503、响应未泄露内部原因，依赖恢复后回到 200，见 `readiness-failure-control.log`。
- 两份迁移 SQL 与原版本逐字节一致，校验和见 `migration-integrity.md`。隔离完整 Compose 的容器、网络和数据卷均已清理，见 `full-stack-cleanup.log`。
- 最终源码清单包含 351 个文件，源代码在验证后未再变动；最终聚合 SHA256：`2278f2bab59fea60454a550fb4852ca055a031d79581ca7a5f362d5c9aecf040`，详见 `final-source-manifest.sha256`。

## 范围边界与剩余限制

- 当前只有 accounts 一个业务服务。其他业务模块仍需按 `services/<module>` 独立定义 API、数据和迁移，并通过 HTTP 接入。
- `@tpl/accounts-ui` 被配置为 `private: true` / `UNLICENSED`。本轮验证的是实际打包 tarball 安装，没有发布 npm 包；公开分发前需要先确定许可与包仓库。
- 人工验收仍由用户完成；此报告只确认实现和机器验证证据，不代替 PRD 中未勾选的 Human-Confirmed 项。

## 评审总结

🔴 严重: 0  ✅
🟠 高: 0  ✅
🟡 中: 0  ✅
🔵 低: 0  ✅

需求状态: ✅ 通过
验证状态: ✅ 通过
文档状态: ✅ 通过

结论: ✅ approved。当前单一 accounts 业务服务及跨宿主 UI tarball 集成满足已批准范围；没有把本轮交付描述为完整的多业务微服务平台。

```json
{
  "verdict": "approved",
  "summary": "独立 accounts 服务、共享认证 UI tarball、部署代理与身份隔离均符合本轮 PRD；最终源码和真实入口验证通过。",
  "findings": []
}
```
