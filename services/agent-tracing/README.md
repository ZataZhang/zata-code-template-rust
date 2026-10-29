# agent-tracing

`agent-tracing` 是仓库的第二个后端微服务。它拥有独立 PostgreSQL 数据库、配置、迁移和镜像，不依赖 accounts 的私有 crate 或数据库。未来 Agent producer 通过版本化 HTTP/JSON 追加事件；管理前端是独立的 [`frontend-tracing`](../../frontends/tracing/) 子服务。

## 接入协议

### 认证与身份绑定

每个 producer 用 `Authorization: Bearer <token>` 调用。部署配置把 token 绑定到固定的 `source_id` 与 `project_id`，请求体不能覆盖这两个字段。token 为 32–4096 字节，建议使用 URL-safe 随机值；服务数据库只保存 SHA-256 摘要。更换 producer 清单后重启服务，旧清单中不再出现的 source 会被禁用。

### 持续发送事件

```http
POST /api/v1/ingest/runs/run_01/events
Authorization: Bearer <producer-token>
Content-Type: application/json
```

```json
{
  "schema_version": 1,
  "events": [
    {
      "seq": 1,
      "event_type": "run.created",
      "occurred_at": "2026-10-29T07:00:00Z",
      "trace_id": "0123456789abcdef0123456789abcdef",
      "span_id": "0123456789abcdef",
      "parent_span_id": null,
      "payload": {
        "thread_id": "thread_01",
        "agent_id": "support-agent",
        "agent_snapshot_checksum": "sha256:…",
        "context_pack_checksum": "sha256:…"
      }
    },
    {
      "seq": 2,
      "event_type": "run.started",
      "occurred_at": "2026-10-29T07:00:01Z",
      "trace_id": "0123456789abcdef0123456789abcdef",
      "span_id": "0123456789abcdef",
      "parent_span_id": null,
      "payload": { "started_at": "2026-10-29T07:00:01Z" }
    }
  ]
}
```

成功返回持久化检查点：

```json
{"run_id":"run_01","accepted_through_seq":2,"replayed_events":0}
```

producer 可以在 Run 仍运行时反复 flush。序号从 1 开始，同一批必须连续递增。服务先提交数据库事务再返回 ack；响应丢失时可重发完全相同的事件。完全相同的重试返回当前检查点，不产生重复记录。序号缺口返回 `409 sequence_gap` 与 `expected_seq`；相同序号内容不同返回 `409 sequence_conflict` 与 `conflict_seq`，新批次事务整体回滚。

当前每批最多 100 个事件、请求体最多 2 MiB、单事件最多 256 KiB，HTTP 请求总时限为 30 秒。`run_id` 是路径字段；`source_id`、`project_id` 只能来自 token 登记。事件 envelope JSON Schema 位于 [`event-batch.schema.json`](event-batch.schema.json)，其中列出 30 种事件以及各自 payload 的必填/可选字段。只接受该版本化目录中的事件类型；未知事件类型、非对象 payload、缺少必填字段或携带未知字段均返回 422。payload 值本身保持 JSON 原样，但敏感字段策略以 [`event.rs`](crates/core/src/event.rs) 内的校验实现为准。

示例 producer 续传逻辑：从本地 durable queue 取 `ack + 1` 开始的连续事件，发送后保存 `accepted_through_seq`；仅收到 HTTP 200 且检查点覆盖该批次后才能从本地队列删除。HTTP 401 需轮换凭据；HTTP 409 gap 按 `expected_seq` 重新对齐；HTTP 413 拆小批次；其他网络错误以退避方式重试同一事件。

### 错误码

| HTTP | `error.code` | producer / UI 行为 |
|---|---|---|
| 401 | `producer_unauthorized` | 检查 bearer token 与 source 登记 |
| 409 | `sequence_gap` | 从 `expected_seq` 重新发送 |
| 409 | `sequence_conflict` | 停止该 Run 并检查持久队列内容 |
| 409 | `run_state_conflict` / `run_terminal` / `trace_id_conflict` | 检查生命周期和 trace 上下文 |
| 408 | 空超时响应 | ack 状态未知时重试同一批；精确重试具备幂等性 |
| 413 | `ingest_limit_exceeded` | 缩小事件批次或单事件载荷 |
| 422 | `unsupported_schema_version` / `event_schema_invalid` / `sensitive_field_forbidden` | 修正 schema、事件字段或敏感内容 |
| 500 | `internal_error` | 使用同一批次安全重试并检查服务日志 |

## 管理员 API

- `GET /admin/agent-traces`：Run 分页，支持 `run_id`、`agent_id`、`status`、创建时间和搜索过滤。
- `GET /admin/agent-traces/{run_id}`：Run 摘要、span 树、节点详情和诊断。
- `GET /admin/agent-traces/threads`：Thread 分页，支持 Agent、状态、创建时间和文本过滤。
- `GET /admin/agent-traces/threads/{thread_id}`：消息、委派、Run 和 span 账本。
- `GET /admin/agent-traces/{run_id}/export`：终态 Run 的完整 canonical JSON 导出；最多 10,000 条事件、10 MiB，`no-store` 并写入低敏导出审计。

所有管理员接口把原始 `admin_session` Cookie 转给 accounts 的公开 `/admin/auth/me` 校验。只有平台管理员可以读取全部 source。隐藏推理链不属于事件契约；消息、工具参数/结果与判定输入摘要可由管理员在本地诊断 UI 查看。OTLP 仅导出低敏属性。

## 配置与启动

默认配置文件为本目录的 `config.toml`；密钥通过 `APP_*` 环境变量注入，不写回 TOML：

```bash
APP_DATABASE__URL='postgres://agent_tracing:…@127.0.0.1:5434/agent_tracing' \
APP_ACCOUNTS__ADMIN_AUTH_URL='http://127.0.0.1:8080/admin/auth/me' \
SQLX_OFFLINE=true cargo run -p tpl-agent-tracing --bin agent-tracing-service
```

`APP_PRODUCER__CREDENTIALS_JSON` 格式为 `[ {"source_id":"…","project_id":"…","token":"至少32字符"} ]`。默认 `[]` 表示尚无 producer 可写入。可选 OTLP/HTTP exporter 默认关闭，设置 `APP_OTLP__TRACES_ENDPOINT` 后才向外发送元数据。

服务提供 `/live` 与 `/ready`，启动时只运行自己的 migration。完整 Compose 使用独立 `tracing-postgres`、`agent-tracing` 和 `tracing-ui` 容器；独立发布镜像命令见 [部署指南](../../docs/guides/deployment.md)。
