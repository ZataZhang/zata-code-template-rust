# Agent 业务微服务示例

这是一个**独立业务微服务**的最小示例：它不连接数据库、不引用 `tpl-agent-*` 私有 crate，
只通过 Agent 服务的 public HTTP 契约创建、查询与取消 Run。

## 它演示什么

| 业务入口 | 转发的 Agent 契约 |
|---|---|
| `POST /api/business/sessions/{session_id}/messages` | `POST /api/v1/sessions/{session_id}/messages`（要求 `Idempotency-Key`） |
| `GET /api/business/runs/{run_id}` | `GET /api/v1/runs/{run_id}` |
| `POST /api/business/runs/{run_id}/cancel` | `POST /api/v1/runs/{run_id}/cancel` |

请求中的用户 `Cookie` 与 `Idempotency-Key` 会原样转发；服务 Bearer token 只由本进程注入，
不会暴露给浏览器。上游成功响应按 `frontends/public/lib/api/agent.ts` 的类型解析，未声明字段
原样保留；上游错误响应按 `{"error":{"code","message"}}` 透传状态码与稳定错误码。

## 配置

| 环境变量 | 必填 | 说明 |
|---|---|---|
| `AGENT_BASE_URL` | 是 | Agent 服务基址，例如 `http://127.0.0.1:8091` |
| `AGENT_SERVICE_TOKEN` | 是 | 调用 Agent 的服务凭据 |
| `BUSINESS_SERVICE_BIND` | 否 | 监听地址，默认 `127.0.0.1:8092` |

本示例**没有** `DATABASE_URL`、`AGENT_DATABASE_URL` 或其他数据库配置，也不依赖 SQLx/Redis。
它不实现自己的业务身份体系；接入真实业务时应在这一层补上业务会话与授权，再决定是否复用最终用户
Cookie。

## 运行

```bash
AGENT_BASE_URL=http://127.0.0.1:8091 \
AGENT_SERVICE_TOKEN=<service-token> \
cargo run -p agent-business-service
```

真实入口验证由 `scripts/agent-e2e.sh --scenario business-service-sample` 完成：脚本会启动完整
Agent 栈与本示例，然后用同一用户 Cookie 分别调用本示例与 Agent 直连接口，比对 Run 创建、重放、
查询、取消和错误码。
