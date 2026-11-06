# Agent 服务

`agent-service`（`services/agent/`）是独立的 Rust 后端微服务，默认端口 **8091**。它拥有 Organization / Runtime / Business Agent / Session / Canonical Run 的目录与运行状态，是这些能力的唯一服务入口。其他业务服务通过 HTTP 使用它，不复制 Agent 实现，也不依赖它的私有 crate。

运行时只读取根 [`config.toml`](../../config.toml) 的 `[agent]` 子树；accounts 与 agent-tracing 不会读取这部分配置。服务拥有独立的 `agent` PostgreSQL 数据库、迁移和 Blob 存储卷，不使用 accounts/tracing 的表、外键或 Redis。

## 1. 身份与授权

每个业务请求都必须同时通过两层身份校验；两者独立，不合并：

| 层 | 校验方式 | 失败结果 |
|---|---|---|
| 调用服务 | `Authorization: Bearer <token>`，token 必须出现在 `AGENT_SERVICE_TOKENS_JSON` 清单中 | `401 service_unauthorized` |
| public 用户 | 原始 `session` Cookie 被转发到 accounts `GET /auth/me` | `401 unauthenticated`；accounts 不可达时 `503 identity_unavailable` |
| 管理员 | 原始 `admin_session` Cookie 被转发到 accounts `GET /admin/auth/me` | 同上 |

- 浏览器永远不持有服务 token：public 的 `/api/agent/*` 由 Next.js Route Handler 注入，admin 的 `/agent-api/*` 由 nginx 注入，Vite 开发代理同样在服务端注入。
- 服务从不信任 `X-User-Id`、请求体自报的用户 ID 或浏览器传来的调用服务身份。
- 平台管理员（`[agent].platform_admin_ids`）只能创建组织和授予组织管理员 scope，**不自动获得任何组织的 scope**。
- 组织管理员必须先有 `organization_admin_scopes` 行才能管理该组织；否则返回 `403 organization_scope_denied`。
- public 用户必须先加入组织（membership）；Agent 可见性 = 组织 allow ∪ 部门 allow ∪ 用户 allow，再被用户 deny 收窄。部门规则不沿父子层级继承，user deny 优先于任何 allow。常见错误码：`organization_membership_required`、`agent_not_authorized`、`agent_access_denied`。
- Session/Run 按 owner 隔离：非所有者读取一律得到 `404 session_not_found` / `run_not_found`，不泄露目标是否存在。

## 2. 数据所有权

Agent 数据库中的表都由本服务拥有，其他服务不得跨库查询或写入：

- 目录与授权：`organizations`、`organization_units`、`organization_memberships`、`organization_admin_scopes`、`runtime_connections`、`business_agents`、`agent_access_rules`、`agent_entry_policies`、`agent_delegation_policies`、`agent_skills`、`agent_skill_assets`、`agent_skill_assignments`。
- 对话与运行：`chat_sessions`、`chat_messages`、`agent_runs`、`agent_run_events`、`agent_trace_outbox`、`agent_run_idempotency`。
- 内容与审计：`user_resources`、`run_artifacts`、`agent_audit_records`。

`agent_runs` 保存创建时的 Agent / Runtime / 输入 / Skill / 资源 / 路由与授权快照，是 Run 状态与执行事件的唯一事实源。没有 AgentVersion 表；普通 Agent 更新不产生“发布版本”。用户资源、Skill 资产和 Run 产物正文存放在 `[agent.storage].root` 指向的 Blob 卷，元数据表只保存文件名、MIME、大小和 SHA-256。存储键由服务生成；文件系统适配器逐级校验根目录边界，拒绝路径穿越和指向根目录外部的符号链接。

## 3. HTTP API

OpenAPI 文件：[`services/agent/openapi.yaml`](../../services/agent/openapi.yaml)。错误统一为 `{"error": {"code", "message"}}`。

健康与元信息（不需要身份）：

- `GET /live`、`GET /health`、`GET /ready`（检查 Agent PostgreSQL，503 时不泄露内部原因）
- `GET /openapi.yaml`

public（service Bearer + 用户 Cookie），前缀 `/api/v1`：

- 组织与 Agent：`GET /organizations`、`GET /organizations/{id}/agents`、`GET /agents/{id}`、`GET /agents/{id}/skills`
- Session：`GET|POST /organizations/{id}/sessions`、`GET|PATCH /sessions/{id}`
- 发送消息：`POST /sessions/{id}/messages`，必须带 `Idempotency-Key` 请求头；可选择 `agent_id`、`resource_ids`、`skill_ids`
- Run：`GET /runs/{id}`、`GET /runs/{id}/events`、`GET /runs/{id}/stream`、`POST /runs/{id}/cancel`、`GET /runs/{id}/artifacts`
- 资源与产物：`GET|POST /organizations/{id}/resources`、`GET|DELETE /resources/{id}`、`GET /artifacts/{id}`

管理面（service Bearer + 管理员 Cookie），前缀 `/admin/api`：

- 身份：`GET /me`
- 组织目录：`/organizations`、`/organizations/{id}/units`、`/organizations/{id}/memberships`、`/organizations/{id}/admin-scopes`
- Runtime：`GET|POST /organizations/{id}/runtimes`、`POST /organizations/{id}/runtimes/{id}/refresh`
- Agent：`GET|POST /organizations/{id}/agents`、`GET|PUT /organizations/{id}/agents/{id}`、`GET .../overview`
- 策略与 Skill：`.../access-rules`、`.../entry-policy`、`.../delegation-policy`、`.../skills`；组织级 `/skills`、`/skills/{id}`、`/skills/{id}/assets`
- 审计：`GET /organizations/{id}/audit`

Agent 创建时绑定 Runtime，后续 `PUT` 不能改绑（`409 runtime_binding_immutable`）。内置 Runtime 由部署在 `[[agent.builtin_runtimes]]` 声明，管理面只读；组织管理员只能接入外部 A2A Runtime。

## 4. Session 与 Run 语义

1. 用户创建 Session 时可选显式 Agent；不选则进入 Auto 入口，服务只在“已授权且 runnable”的候选中按入口策略优先级选择。
2. 每条 user 消息对应一个 Canonical Run。同一 `Idempotency-Key` + 同一规范化请求重放原 Run（`200`，`created: false`）；同一 key 用于不同请求返回 `409 idempotency_conflict`。
3. Run 状态：`queued` → `running` → `succeeded` / `failed` / `cancelled`，取消请求期间为 `cancelling`。`mark_run_started` 是幂等闸门，重复调度不会重复执行。
4. 事件从 1 开始持久单调编号。`GET /runs/{id}/events?after_seq=N` 返回分页事件；`GET /runs/{id}/stream?after_seq=N` 以 SSE 续读，`data` 是完整 `RunEvent` JSON（`run_id`、`seq`、`event_type`、`occurred_at`、`payload`），事件名同时是 `event_type`，终止状态且追平 `last_event_seq` 后关闭，空闲时发送 keep-alive。
5. 取消是协作式的：服务写入 `run.cancelling` 并在工具调用之间停止，已完成的委派或产物副作用不回滚；`POST /cancel` 返回 `cancelled_immediately` 表示是否当场进入终态。

## 5. Runtime 适配器

| 来源 | 协议 | 行为与限制 |
|---|---|---|
| builtin | `sandbox` | 进程内受限平台工具，不取得容器、宿主进程或网络执行权限。只允许部署声明的工具白名单（默认 `text.summarize`、`artifact.write`、`delegate_task`）并受 `max_steps` 限制；越权或超预算 fail closed |
| external | `a2a` | A2A 1.0 JSON-RPC：探测 Agent Card，发送 `SendMessage` / `SendStreamingMessage`，必要时 `GetTask` / `CancelTask`；每次请求带 `A2A-Version: 1.0`，可选 Bearer 凭据按 `credential_alias` 从环境注入 |

外部 Runtime 端点只接受 `http` / `https`，拒绝 URL 内嵌用户信息与 fragment；单次 HTTP 超时默认 60 秒、连接超时 5 秒；Agent Card ≤ 1 MiB、RPC 响应 ≤ 4 MiB、流式文本累计 ≤ 8 MiB；轮询任务最多 300 秒。`SendMessage` 直接返回 Message 时按一次调用完成处理（即使消息携带 `taskId` 也不再轮询）；返回非终态 Task 时才按 `taskId` 轮询。外部 A2A 当前不接受用户上传资源（`a2a_resources_unsupported`），资源引用只在 Sandbox 路径可用。

Sandbox 指令是显式文本约定，不是任意代码执行：普通文本由 `text.summarize` 摘要；```artifact filename="..." media_type="..."``` 代码块写出产物；`@delegate <child-agent>: <task>` 触发受治理委派。委派目标必须同组织，在父 Agent 的委派策略白名单内，并受 `max_depth` 与 `max_child_runs` 限制；被拒绝时写 `delegation.rejected` 事件。

### 5.1 独立业务服务接入示例

[`examples/agent-business-service`](../../examples/agent-business-service/README.md) 是一个不连接数据库、不引用 `tpl-agent-*` 私有 crate 的业务微服务示例。它只配置 `AGENT_BASE_URL` 与 `AGENT_SERVICE_TOKEN`，把用户的 `Cookie` 和 `Idempotency-Key` 原样转发到 public API，并按
`frontends/public/lib/api/agent.ts` 的类型解析 Run 创建、查询、取消响应与 `{"error":{"code","message"}}` 错误封套。
`just agent-e2e --scenario business-service-sample` 会把同一用户的请求分别经示例与 Agent 直连发送，比对响应字段、幂等重放、取消状态和 404/401/400 错误码。

## 6. Skill、资源与产物

- Skill 是组织级运行机制与说明，不迁移来源仓库的业务 Skill 内容；Agent 只可以使用被显式授权的 Skill，Run 快照记录实际选择的 Skill 及其资产。
- 用户资源接受的 MIME：`text/plain`、`text/markdown`、`application/json`、`image/png`、`image/jpeg`、`application/pdf`；单文件默认上限 10 MiB。文件名会被规范化，存储路径不使用用户文件名；所有读取校验 owner。
- Run 产物默认上限 10 MiB，下载时校验 owner；产物元数据包含 SHA-256。
- JSON 请求体上限 256 KiB；上传路由单独放宽到 `max_resource_bytes + 1 MiB` 包装开销。

## 7. tracing 事件与 outbox

- Run 事件与 outbox 在同一本地事务提交；后台 worker 按 Run 与持久序号分批投递到 `[agent.tracing].ingest_url` 的 `/runs/{run_id}/events`。
- 只允许契约内的事件类型与低敏字段：关联 ID、状态、时间、摘要、checksum。含 `token`、`password`、`authorization`、`cookie`、`secret`、`api_key`、`reasoning` 等键的载荷会被拒绝；需要展示的完成内容在 tracing 中固定为 `[已脱敏]`。完整提示词、资源正文、工具参数/结果和凭据不进入事件或 outbox。
- 单片事件 ≤ 256 KiB，单批 ≤ 100 条；tracing 返回的 `accepted_through_seq` 作为 ack 检查点，失败按退避重试并记录稳定错误码。
- tracing 不可用只让 outbox 事件保持待投递，**不阻塞、不失败** Run 的本地完成。未配置 producer token 时投递 worker 不启动。
- Agent 服务不直接写 tracing 数据库。

## 8. 配置

完整示例见根 [`config.toml`](../../config.toml) 的 `[agent]` 与 [`.env.example`](../../.env.example)。秘密只从环境或部署平台注入：

| 配置 | 默认 | 说明 |
|---|---|---|
| `DATABASE_URL` 或 `APP_AGENT__DATABASE__URL` | 未配置 | **必填**；Agent 专属 PostgreSQL。启动时执行 `services/agent/migrations/` |
| `APP_AGENT__SERVER__HOST` / `PORT` | `127.0.0.1` / `8091` | 容器镜像绑定 `0.0.0.0:8091` |
| `AGENT_SERVICE_TOKENS_JSON` | `[]` | 调用服务凭据数组，每项 `{"service_id","token"}`；token 至少 32 字符 |
| `APP_AGENT__IDENTITY__USER_AUTH_URL` | `http://127.0.0.1:8080/auth/me` | public 用户身份校验地址 |
| `APP_AGENT__IDENTITY__ADMIN_AUTH_URL` | `http://127.0.0.1:8080/admin/auth/me` | 管理员身份校验地址 |
| `APP_AGENT__IDENTITY__REQUEST_TIMEOUT_SECS` | `3` | 调用 accounts 的超时 |
| `APP_AGENT__STORAGE__ROOT` | `./data/blobs` | 资源、Skill 资产与产物目录 |
| `APP_AGENT__STORAGE__MAX_RESOURCE_BYTES` / `MAX_ARTIFACT_BYTES` | 10 MiB | 单文件上限 |
| `APP_AGENT__EXECUTION__A2A_TIMEOUT_SECS` | `60` | 单次 A2A 请求超时 |
| `APP_AGENT__TRACING__INGEST_URL` | `http://127.0.0.1:8090/api/v1/ingest` | tracing producer 基础地址 |
| `AGENT_TRACING_PRODUCER_TOKEN` | 未配置 | 投递 token；必须与 tracing 的 producer 清单一致 |
| `APP_AGENT__TRACING__POLL_INTERVAL_MS` / `BATCH_EVENTS` | `1000` / `100` | outbox 轮询与批次上限 |
| `AGENT_CREDENTIAL_<ALIAS>` | 未配置 | 外部 A2A Runtime 的 Bearer 密钥；缺省时该 Runtime 明确不可用 |
| `APP_AGENT__PLATFORM_ADMIN_IDS` | `[]` | 平台管理员 accounts ID 列表 |
| `[[agent.builtin_runtimes]]` | 见 `config.toml` | 部署声明的只读内置 Runtime（name / display_name / adapter_kind / enabled / allowed_tools / max_steps） |

Nacos 应用 TOML 可提供 `agent.database.url` 等非敏感默认值；不要把连接串、producer token 或服务 token 写进仓库文件。

## 9. 运行、探针与回滚

- 启动时要求 Agent 数据库可达并执行迁移；失败即快速退出，交给编排平台重启。数据库短暂中断时进程不退出，`/ready` 返回 503，恢复后自动变回 200。
- 收到 SIGTERM / Ctrl+C 后停止接收新请求并退出；后台调度与 outbox worker 随 shutdown 信号停止。
- 日志支持 `text` / `json`；连接串、token 清单与 producer token 的 `Debug` 输出被隐藏，错误响应不含内部细节。
- **回滚**：Agent 迁移只向前，不提供 down migration。回滚应用时先停止 `agent-service`（或从 nginx/Route Handler 下线 `/api/agent`、`/agent-api` 入口），重新部署上一个镜像版本，保留 `agent-postgres-data` 与 `agent-blob-data` 卷；新版本已写入的数据由旧版本按兼容字段读取。若回滚包含不兼容的 schema 变更，先按备份流程恢复数据库快照，再启动旧镜像；不要用 `stack-reset` 排错，它会删除数据卷。
- `just agent-docker-build` / `just agent-up` / `just agent-down` 管理独立镜像与 Compose 服务；`just agent-e2e` 跑真实浏览器/API 端到端场景。

## 10. 未迁移的能力

本期不迁移 ai-assistant 的 accounts 实现、Outlook 宿主、用户 CLI、业务 Skill 内容、无关产品工作流或历史数据；不提供任意 shell、任意网络请求或越过沙箱边界的执行；不把 agent-tracing 当作 Run 的唯一事实库或同步执行依赖。组织/部门目录与授权属于本服务范围，但成员关系的最终维护方式仍以 PRD 决定记录为准。

## 11. 排错

| 现象 | 检查 |
|---|---|
| `503 agent_unavailable`（public） | Next Route Handler 是否设置了 `AGENT_SERVICE_TOKEN`，`AGENT_SERVICE_URL` 是否可达 |
| `401 service_unauthorized` | 调用方 token 是否在 `AGENT_SERVICE_TOKENS_JSON` 中，值是否一致 |
| `503 identity_unavailable` | `[agent.identity]` 的 accounts 地址与超时；accounts 是否就绪 |
| 管理员得到 `403 organization_scope_denied` | 该管理员是否被显式授予组织 scope；平台管理员不会自动获得 |
| Agent 选择不到 | Agent 是否 `enabled`、Runtime 是否可用、用户是否有 allow 规则、入口策略是否 `auto_visible` |
| tracing 长时间缺事件 | `just agent-e2e` 的 tracing-outage 场景；检查 producer token、`ingest_url` 与 outbox 日志；Run 本身应仍能完成 |
| 上传 `415 unsupported_media_type` / `413 payload_too_large` | 资源 MIME 白名单与 `max_resource_bytes` |
