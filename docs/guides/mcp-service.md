# MCP 服务

`mcp-service`（`services/mcp/`）是独立的 Rust 后端微服务，默认监听 **127.0.0.1:8092**。它集中管理远端 MCP（Model Context Protocol）连接、写-only 凭据与用户/分组授权，并通过 `/mcp` 暴露一个按调用方身份聚合工具的 Streamable HTTP 网关。

服务拥有独立的 `mcp` PostgreSQL 数据库、迁移、配置与镜像；不读取 accounts / Agent 的数据库、Redis、配置或私有 crate。消费项目后端在完成自己的用户认证后，用服务 token 与签名上下文调用本服务。

## 1. 范围与非目标

本服务提供：

- 远端 MCP 连接的创建、读取、更新、停用、删除；
- Bearer 凭据的设置与轮换（只写入，响应只报告是否已配置）；
- 用户与分组授权（按消费服务与租户隔离）；
- 连接检查与远端工具发现；
- 聚合 MCP 网关：`tools/list` 按授权过滤，`tools/call` 再次校验授权。

明确不包含：

- 公开 MCP 市场、连接目录浏览或任何消费方 UI；
- `stdio` transport、旧版 HTTP+SSE transport；
- `prompts` / `resources` 代理；
- 用户、组织与登录管理（由消费项目自己负责）；
- 从 ai-assistant 或其他项目迁移现有连接数据；
- 浏览器直接访问本服务。

## 2. 身份与授权

每次请求必须同时携带两层凭据，二者缺一不可：

| 层 | 请求头 | 校验方式 | 失败 |
|---|---|---|---|
| 消费服务 | `Authorization: Bearer <token>` | token 的 SHA-256 与配置清单中的某个消费方常量时间匹配 | `401 unauthorized` |
| 用户上下文 | `x-user-context: <payload>.<signature>` | 用该消费方的 `context_signing_secret` 验 HMAC-SHA-256，再校验 issuer/audience、有效期与 `consumer_id` 绑定 | `401 unauthorized` |

用户上下文格式：

```text
base64url_nopad(payload_json) + "." + base64url_nopad(hmac_sha256(secret, payload_segment))
```

payload 是紧凑 JSON：

```json
{
  "iss": "tpl-consumer-backend",
  "aud": "tpl-mcp-gateway",
  "consumer_id": "web-backend",
  "tenant_id": "tenant-a",
  "subject_id": "user-1",
  "groups": ["team-a"],
  "role": "user",
  "iat": 1762656000,
  "exp": 1762656060
}
```

- `tenant_id`、`subject_id` 与分组 ID 都是消费项目的不透明字符串；本服务不解释它们，也不查询任何外部数据库。
- `role` 只接受 `user` 或 `admin`。管理接口要求 `admin`；网关接受两者。
- 默认有效期上限 300 秒（`exp - iat`），允许 30 秒时钟偏差；上下文超过 8 KiB、签名不匹配或过期都会被拒绝。
- 浏览器永远不持有服务 token 或签名密钥；它们只存在于消费项目后端与部署平台。

签名示例（Python，仅用于联调，密钥从环境变量读取）：

```python
import base64, hashlib, hmac, json, os, time

secret = os.environ["MCP_CONTEXT_SIGNING_SECRET"].encode()
now = int(time.time())
claims = {
    "iss": "tpl-consumer-backend", "aud": "tpl-mcp-gateway",
    "consumer_id": "web-backend", "tenant_id": "tenant-a",
    "subject_id": "user-1", "groups": ["team-a"], "role": "user",
    "iat": now, "exp": now + 60,
}
payload = base64.urlsafe_b64encode(
    json.dumps(claims, separators=(",", ":")).encode()
).rstrip(b"=").decode()
signature = base64.urlsafe_b64encode(
    hmac.new(secret, payload.encode(), hashlib.sha256).digest()
).rstrip(b"=").decode()
print(f"{payload}.{signature}")
```

## 3. 数据所有权

MCP 数据库中的表都由本服务拥有，其他服务不得跨库查询或写入：

- `mcp_connections`：连接元数据、认证方式、加密后的凭据、状态与创建者；
- `mcp_connection_assignments`：连接 × 主体（`user` / `group`）授权。

凭据明文没有任何数据库列或数据库表示。连接按 `(consumer_id, tenant_id)` 隔离；一个消费方无法读取另一个消费方或租户的行。授权主体使用字符串 ID，不建立到 accounts 的外键。

## 4. HTTP API

OpenAPI 文件：[`services/mcp/openapi.yaml`](../../services/mcp/openapi.yaml)，运行中的服务也在 `GET /openapi.yaml` 提供同一份契约。REST 错误统一为 `{"error": {"code", "message"}}`。

匿名接口：

- `GET /live`、`GET /health`、`GET /ready`（检查 MCP PostgreSQL，503 时不泄露内部原因）
- `GET /openapi.yaml`

管理接口（service Bearer + `admin` 签名上下文），前缀 `/api/v1/connections`：

- `GET|POST /api/v1/connections`
- `GET|PATCH|DELETE /api/v1/connections/{connection_id}`
- `POST /api/v1/connections/{connection_id}/disable`
- `PUT /api/v1/connections/{connection_id}/credential`
- `GET /api/v1/connections/{connection_id}/tools`（检查连接并调用远端 `tools/list`，不执行工具）
- `GET /api/v1/connections/{connection_id}/assignments`
- `PUT|DELETE /api/v1/connections/{connection_id}/assignments/{subject_type}/{subject_id}`（`subject_type` 为 `user` 或 `group`）

创建或更新 endpoint 时先执行出站地址策略校验，再写入数据库。`PATCH` 的 `status: "disabled"` 与停用接口等价；从 `none` 切换到 `bearer` 前必须先通过凭据接口保存凭据。授权新增与移除是幂等的，重复请求同样返回 `204`。

错误码：`invalid_request`、`unsafe_endpoint`、`unauthorized`、`forbidden`、`not_found`、`conflict`、`credential_unavailable`、`upstream_unavailable`、`payload_too_large`、`internal`。REST JSON 请求体上限 256 KiB；凭据必须为可见 ASCII，长度 1–4096。

## 5. MCP 网关

`POST /mcp` 是标准 Streamable HTTP endpoint，身份要求与 REST 相同。客户端执行 `initialize`、`tools/list`、`tools/call`：

- 对外工具名格式为 `mcp_<connection-uuid-hex>_<远端工具名>`；连接 ID 前缀在连接生命周期内稳定，不同连接的同名工具不会冲突。
- `tools/list` 只返回当前租户、当前主体或其分组已授权且状态为 `active` 的连接工具。
- `tools/call` 解析工具名后**重新查询授权**，不复用列表缓存；未授权返回 JSON-RPC `-32003`（`data.status = 403`）。
- 单个连接发现失败时只跳过该连接并记录告警，不影响其他连接；所有连接都不可用时返回空列表。
- 调用时远端不可达返回 `-32005`（`data.status = 502`），工具不存在返回 `-32002`，参数错误返回 `-32602`。
- 服务会校验 `Host`（`gateway.allowed_hosts`）与 `Origin`；默认只接受 `localhost`、`127.0.0.1`、`::1`。生产容器内设为服务名，浏览器不应直连。
- MCP 请求体上限由 `gateway.max_request_body_bytes` 控制（默认 1 MiB）。

## 6. 凭据与出站安全

- 凭据使用 AES-256-GCM 加密，主密钥由 `MCP_CREDENTIAL_KEY_BASE64` 注入；密文与连接 ID、消费服务、租户绑定，换库或改绑无法解密。
- API 响应只返回 `credential_configured`；日志、错误和 `Debug` 输出都不包含凭据明文。
- 出站默认只允许 HTTPS，并拒绝 loopback、私网、链路本地、组播、保留地址与云元数据地址；DNS 解析后固定地址，避免重绑定。
- 不跟随重定向；连接超时默认 5 秒，请求总超时默认 15 秒，SSE 事件与编码后工具结果上限默认 1 MiB（JSON 接收上限见下文），单连接最多暴露 256 个工具。
- 响应上限对 SSE 事件在解析前生效；对非流式 JSON 响应，rmcp 内置客户端在解析前固定限制为 16 MiB，`outbound.max_response_bytes` 在反序列化后再次校验工具发现与调用结果的大小（最小有效值 1024 字节）。因此默认 1 MiB **不是 JSON 接收或内存预算**：单个 JSON 响应可先读取至 16 MiB，反序列化对象与重新编码副本还会额外占用内存。`list_all_tools()` 会先聚合所有分页，16 MiB 是单页上限，分页总量不受它限制；聚合结果在返回后才按配置拒绝。部署时须为并发请求、分页与解析放大预留内存；若需要严格的 1 MiB JSON 接收预算，当前适配器不提供该保证。
- IPv6 `2001::/23` 按 [IANA 特殊用途登记表](https://www.iana.org/assignments/iana-ipv6-special-registry)默认拒绝，仅放行明确全球可达的子分配；`2001:2::/48` benchmarking 地址在注册和每次使用时均拒绝。
- `tools/list` 最多并发查询 8 个连接，完成后统一按工具名排序。单连接失败只跳过该连接；没有额外的列表整体截止时间，超过 8 个连接仍按空出的并发槽继续处理。
- 日志固定把 `rmcp` 目标压回 `info`：即使设置 `RUST_LOG=debug`，也不会输出完整的 JSON-RPC 工具参数或工具结果。
- 仅开发环境可通过 `security.allow_insecure_loopback = true` 允许访问 loopback 上的 HTTP 测试 MCP server；生产必须保持 `false`。

## 7. 配置

服务只读取 `services/mcp/config.toml`（可用 `APP_MCP_CONFIG_FILE` 覆盖）与 `APP_MCP__<分区>__<字段>` 环境变量；标准启动方式是在仓库根运行，因此根 `.env.local` 也会被读取。秘密只从环境注入：

| 变量 | 必填 | 说明 |
|---|---|---|
| `MCP_DATABASE_URL` | 是 | MCP 专属 PostgreSQL，必须为 `postgres://` 或 `postgresql://` |
| `MCP_CREDENTIAL_KEY_BASE64` | 是 | 标准 Base64 的 32 字节主密钥（`openssl rand -base64 32`） |
| `MCP_CONSUMERS_JSON` | 是 | 消费方清单；每个 `token` 与 `context_signing_secret` 至少 32 字符，`consumer_id` 不可重复，空数组视为配置无效 |

常用非秘密配置（完整默认值见 [`services/mcp/config.toml`](../../services/mcp/config.toml)）：

| 键 | 默认 | 说明 |
|---|---|---|
| `server.host` / `server.port` | `127.0.0.1` / `8092` | 监听地址与端口 |
| `logging.level` / `logging.format` | `info` / `text` | `RUST_LOG` 优先；容器内用 `json` |
| `security.context_issuer` / `context_audience` | `tpl-consumer-backend` / `tpl-mcp-gateway` | 必须与消费方签发值一致 |
| `security.context_max_age_secs` / `context_clock_skew_secs` | `300` / `30` | 上下文有效期上限与时钟偏差 |
| `security.allow_insecure_loopback` | `false` | 仅本地测试远端 loopback HTTP server |
| `outbound.connect_timeout_ms` / `request_timeout_ms` | `5000` / `15000` | 远端连接与请求超时 |
| `outbound.max_response_bytes` / `max_tools_per_connection` | `1048576` / `256` | SSE 解析前 / JSON 结果编码后上限与工具数量上限；JSON HTTP 体解析前固定 16 MiB |
| `gateway.allowed_hosts` | `["localhost","127.0.0.1","::1"]` | MCP endpoint 的 Host 允许列表；空列表表示关闭校验 |
| `gateway.max_request_body_bytes` | `1048576` | MCP 请求体上限 |

## 8. 本地启动与验证

```bash
just db-up   # 启动本机测试中间件（含 5436 的 MCP PostgreSQL）
export MCP_DATABASE_URL=postgres://mcp:mcppass@127.0.0.1:5436/mcp_dev
export MCP_CREDENTIAL_KEY_BASE64="$(openssl rand -base64 32)"
export MCP_CONSUMERS_JSON='[{"consumer_id":"web-backend","token":"<至少32字符>","context_signing_secret":"<至少32字符>"}]'
cargo run -p tpl-mcp --bin mcp-service
curl -fsS http://127.0.0.1:8092/ready
```

真实入口端到端场景（真实 PostgreSQL、真实 `mcp-service` 进程、真实 rmcp 客户端与本地假上游）：

```bash
just mcp-e2e --scenario authorization       # 租户/用户/分组授权与 403
just mcp-e2e --scenario remote-connection   # 连接注册、加密、发现、重启后调用与出站拒绝
just test real                              # 所有需要真实 PostgreSQL 的忽略测试
```

## 9. 部署与回滚

- 构建镜像：`just mcp-docker-build`（等价 `docker build -f services/mcp/Dockerfile -t tpl-mcp:local .`）。
- 单独启动：`just mcp-up`；停止：`just mcp-down`。MCP 服务与 `mcp-postgres` 位于根 Compose 的 `mcp` profile，默认整站 `just stack-up` 不会启动它们。
- Nacos Compose 配置中需要 `MCP_DB_PASSWORD`、`MCP_CREDENTIAL_KEY_BASE64`、`MCP_CONSUMERS_JSON`；可选 `MCP_PORT` 覆盖宿主机回环端口。缺失秘密时 MCP 容器明确启动失败，不影响其他服务。
- 就绪检查：`curl -fsS http://127.0.0.1:${MCP_PORT:-8092}/ready`。
- 回滚只需停止 MCP 服务并回退镜像；其他服务不依赖它启动，MCP 数据卷独立保留。更换主密钥后旧密文凭据无法解密，需要重新设置凭据。
