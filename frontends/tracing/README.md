# frontend-tracing

独立部署的 Agent Run 轨迹管理界面，使用 Vite + React。它只通过同源 HTTP API 访问服务：管理员登录由 accounts-service 处理，Run/Thread 查询和 producer 事件写入由 agent-tracing 处理。

## 本地开发

```bash
just db-up
just run tracing
```

`just run tracing` 会启动 accounts API（供管理员登录）、tracing API 和 Vite UI。默认界面地址是 `http://127.0.0.1:5174`。开发代理将 `/api/admin/auth/*` 转给 accounts，管理员轨迹查询转给 agent-tracing；producer 接入路径 `/api/v1/ingest/*` 保留 API 前缀。

单独启动 UI：

```bash
just web-tracing-install
just web-tracing-dev
```

## 界面能力

- 分页查看 Run 和 Thread，按状态、文本与创建时间筛选。
- 查看 Run 生命周期摘要、模型/工具/产物/判定树、低敏诊断和节点检查器。
- 查看 Thread 消息、委派与步骤账本；支持消息内容搜索、时间/序号模式和时间范围拖选。
- 终态 Run 可下载完整 canonical JSON 事件；导出内容按敏感业务数据处理。

UI 不能替代 producer 凭据。未来 Agent 服务使用 `POST /api/v1/ingest/runs/{run_id}/events` 和绑定 source/project 的 Bearer Token，详见 [`../../services/agent-tracing/README.md`](../../services/agent-tracing/README.md)。

## 生产镜像

在仓库根目录构建：

```bash
docker build -f frontends/tracing/Dockerfile -t tpl-tracing-ui:local frontends/tracing
```

nginx 把 `/api/v1/ingest/*` 原样转给 tracing API；`/api/admin/auth/*` 转给 accounts；其他 `/api/*` 管理路径去掉 `/api` 后转给 tracing API。部署 HTTPS 终止代理时，通过 `TRUSTED_PROXY_CIDR` 配置可信网段；nginx 会覆盖写 `X-Forwarded-For`。
