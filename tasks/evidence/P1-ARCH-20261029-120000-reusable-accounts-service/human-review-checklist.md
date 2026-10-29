# 人工审查：可复用账号模块

本页只包含 PRD §9 中尚待人工确认的一项；构建、服务隔离和安全测试由执行器与独立审查负责，详情见 [证据报告](evidence-report.md)。

## 账号模块可以被两个应用接入

你在决定账号服务与前端包是否满足这次跨应用复用目标。若判断有误，下次应用可能仍要复制认证逻辑，或误把共享账户域当成租户隔离。

PRD §9 Human-Confirmed 原文：

> 两个应用安装同一份认证页面包、接入独立账号服务，满足本轮复用目标（§2，rv-1）。

账号服务拥有账号数据与会话，前端包提供共用认证表单。公开站点用本地包依赖，独立 React 宿主安装真实 npm tarball；本轮只有首个账号业务服务。包尚未公开发布；没有新增租户隔离或跨域单点登录。

![公开站点认证后的当前用户](public-account-session.png)

公开站点，real user flow。本地图片，GitHub 上不显示。仓库根目录执行：

```bash
open tasks/evidence/P1-ARCH-20261029-120000-reusable-accounts-service/public-account-session.png
```

![第二宿主认证后的当前用户](second-host-session.png)

独立 React 宿主，real user flow。本地图片，GitHub 上不显示。仓库根目录执行：

```bash
open tasks/evidence/P1-ARCH-20261029-120000-reusable-accounts-service/second-host-session.png
```

复核位置：第一张看 Workspace 下的当前用户邮箱；第二张看 Signed in 下的当前用户邮箱。退出和错误密码的会话失效结果由真实 HTTP/浏览器断言验证，不靠截图推断。

答复格式：`同意：账号模块满足本轮复用目标`，或 `需要调整：<具体差异>`。HTML 页的选择只在浏览器本地保存，不会自动标记 PRD 已验收。
