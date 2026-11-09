# MCP Gateway 服务验收证据报告

## 人审导航 / Human Review Navigation

本 PRD 的人审呈递区（§9.1）是服务范围与 API 契约；审查对象是仓库内文档与 OpenAPI，不涉及页面截图。

| 呈递 | 绝对路径 | 打开命令 | 期望值 | 执行侧交叉核对 |
|---|---|---|---|---|
| MCP 服务指南 | `/Users/zata/code/rust-study/.kc-worktrees/issue-4/docs/guides/mcp-service.md` | `open "/Users/zata/code/rust-study/.kc-worktrees/issue-4/docs/guides/mcp-service.md"` | 范围与 API 两节列出连接管理、用户/分组授权、连接检查与 `/mcp` Streamable HTTP 网关，并明确排除公开 MCP 市场与消费方 UI | `rv-3-docs-scope.txt` 已校验 API 路径、`/mcp` 与排除项关键词 |
| API 契约 | `/Users/zata/code/rust-study/.kc-worktrees/issue-4/services/mcp/openapi.yaml` | `open "/Users/zata/code/rust-study/.kc-worktrees/issue-4/services/mcp/openapi.yaml"` | `paths` 包含 7 条 `/api/v1/connections...` 管理路径与 `POST /mcp`，错误码与鉴权说明完整 | 与 `services/mcp/crates/api/src/routes/*.rs` 逐路径比对一致；运行中服务 `GET /openapi.yaml` 返回同一契约 |

本项没有仍需要人审的静态图片；审查按上表直接打开最终树中的 Markdown 与 YAML。PR 由 runner 创建，此处暂无 PR/CI 链接。

## 1. 结论

- §9 全部执行侧验收项已由最终代码状态的真实入口证据覆盖；仅 3 项 Human-Confirmed 保留待人工确认，未代勾。
- `kc evidence check --evidence tasks/evidence/P1-FEAT-20261109-104805-mcp-gateway-service`：3/3 PASSED。
- 最终树（HEAD `48fbe38` + pre-PR 复审修复的未提交工作树改动）重跑 `just test`、`just test real`、`just ci`、`just mcp-docker-build` 均 exit 0；`rv-1`、`rv-2`、`rv-3` 在最终树重新执行并变绿，证据文件哈希不变。
- `just mcp-up` 与 `/live`、`/ready` 在 checkpoint `b1e62f9` 上验证通过；之后的修复仅涉及响应序列化、日志过滤、endpoint 长度校验、e2e 测试与文档，不改变启动、探针或 compose 路径，最终树镜像已重建，最终树 e2e 每场景均通过真实进程 `/ready` 轮询。

## 2. 最终代码状态

| 字段 | 值 |
|---|---|
| HEAD | `48fbe384e808abef833629e321b219df6578c24d`（pre-PR 复审修复为其后未提交工作树改动） |
| 工作树指纹（sha256） | `579109453dafccfda95b2410901f51cd30b1c2480d95952631e922993f9bb4b3` |
| 指纹范围 | tracked + untracked（`git ls-files -co --exclude-standard`），排除 `tasks/evidence/**`、本 PRD 与 `.agent-runner/**` 等记录路径；逐文件 `shasum -a 256` 后与路径组成行，`LC_ALL=C sort` 排序再汇总 sha256 |
| 参与文件数 | 584 |
| Rust / Cargo | 1.99.0 / 1.99.0 |
| Docker | 29.4.2 |
| 记录时间（UTC） | 2026-11-09T08:06:59Z |

证据文件在最终代码状态重跑后生成，摘要如下：

| 证据文件 | SHA-256 |
|---|---|
| `rv-1-authorization.txt` | `48e496cf0d1361129345502d83ef959119f59ed09b8f09421959ad247a338d16` |
| `rv-2-remote-connection.txt` | `fa3ff53b51967ad4cf0266e99aae36bf7fed4360722ca7968318e9c56f957b94` |
| `rv-3-docs-scope.txt` | `0da38920845ad099f72cce65bde234151c821879925b7edbbf3e685974a83039` |

## 3. rv-1：授权与租户隔离

- 命令：`bash tasks/evidence/P1-FEAT-20261109-104805-mcp-gateway-service/scripts/rv-1-authorization.sh`
- 入口：`just mcp-e2e --scenario authorization`，真实 `mcp-service` 进程 + 真实 PostgreSQL 迁移 + 真实 HMAC 签名上下文 + 真实 rmcp 网关/客户端。
- 关键检查点：
  - 无认证请求返回 401。
  - 管理员通过 REST 创建连接并授权 `user-allowed`。
  - 获准用户 `tools/list` 只看到带连接前缀的 `lookup_weather`，`tools/call` 返回上游结果。
  - 同租户未授权用户与另一租户同名用户的 `tools/list` 为空，`tools/call` 返回 `-32003` / HTTP 403。
  - 第二消费服务在同一租户字符串下登记连接并授权同名用户后，双方用户各自只看到、且只能调通自己的连接；跨消费服务调用返回 `-32003` / HTTP 403。
  - 服务以 `RUST_LOG=debug` 运行，日志仍不含工具参数或工具结果。
- 负控：测试边界临时把未授权用户断言反转为“必须看到工具”，实际失败为 `assertion failed`、`test result: FAILED`、退出码 101；恢复后同一条命令变绿（最终树重跑确认）。
- 限制：远端 provider 为本地 fake，未覆盖真实第三方网络的 TLS/证书差异。

## 4. rv-2：连接管理、加密凭据、出站安全与重启

- 命令：`bash tasks/evidence/P1-FEAT-20261109-104805-mcp-gateway-service/scripts/rv-2-remote-connection.sh`
- 入口：`just mcp-e2e --scenario remote-connection`。
- 关键检查点：
  - 生产安全策略在写入阶段拒绝 loopback endpoint，假上游请求计数保持 0。
  - HTTPS 云元数据（`169.254.169.254`、`100.100.100.200`）、私网（`10.0.0.1`、`192.168.1.1`）、IPv4 映射（`::ffff:a9fe:a9fe`）与 ULA（`fd00:ec2::254`）地址均在写入阶段返回 400 `unsafe_endpoint`，上游请求计数保持 0。
  - 开发 loopback 模式仍拒绝私网 `https://10.0.0.1/mcp`。
  - 创建响应只包含 `credential_configured=true`，不返回凭据明文。
  - PostgreSQL 中只保存 57 字节密文，且不含凭据明文。
  - 管理 API 通过真实 rmcp 握手发现 `lookup_weather`。
  - 服务进程重启后，新进程从同一数据库读取连接、解密凭据，并由新 MCP 客户端完成 `tools/list` 与 `tools/call`。
  - 真实不可用的上游连接不提供工具、调用返回 `-32005` / HTTP 502，健康连接仍可发现和调用。
  - 两次进程在 `RUST_LOG=debug` 下的日志均不含凭据、工具参数或工具结果。
- 负控 A：临时把 endpoint 地址策略改为全部放行，地址矩阵首个目标 `https://169.254.169.254/latest/meta-data` 返回 201 而非 400，实际失败为 `目标 ... 必须在写入阶段被拒绝`、`left: 201`、`right: 400`、退出码 101；恢复后变绿。
- 负控 B：临时移除 gateway 对单个连接失败的跳过逻辑，不可用上游使 `tools/list` 整体失败，实际错误为 `-32005`、退出码 101；恢复后变绿。
- 限制：上游为本地 fake；未验证真实外部 provider 的网络差异与主密钥轮换后的旧密文行为。

## 5. rv-3：发布契约范围审查

- 命令：`bash tasks/evidence/P1-FEAT-20261109-104805-mcp-gateway-service/scripts/rv-3-docs-scope.sh`
- 关键检查点：
  - OpenAPI 包含 8 条必需路径（连接 CRUD、停用、凭据、工具检查、授权、`/mcp`）。
  - 路由器的 7 条 `/api/v1/connections...` 路径与 OpenAPI 管理路径完全一致，`/mcp` 由 `nest_service("/mcp")` 挂载。
  - 指南显式覆盖连接管理、凭据与 `/mcp`。
  - 指南显式排除公开 MCP 市场、消费方 UI、`stdio`、HTTP+SSE 与 `prompts`/`resources`。
- 负控：临时从 OpenAPI 删除 `/mcp` 后，脚本输出 `失败：OpenAPI 缺少路径 /mcp` 并以非零退出；恢复后变绿。
- 限制：自动化只核对路径、方法与关键词，文档措辞清晰度由人工 reviewer 判定。

## 6. 构建与部署验证

| 检查 | 结果 |
|---|---|
| `just test` | exit 0（2026-11-09，最终树） |
| `just ci` | exit 0（fmt + clippy `-D warnings` + 全量测试 + `cargo doc -D warnings`，最终树重跑） |
| `just test real` | exit 0（最终树重跑）；`tpl-mcp` 的 `real_authorization_scenario`、`real_remote_connection_scenario` 均通过；缺少 `TEST_MCP_DATABASE_URL` 时测试 `expect` 失败而非静默跳过 |
| `just mcp-docker-build` | 最终树重建成功，`tpl-mcp:local` 镜像 manifest list `sha256:f5dd6de0…`（pre-PR 复审修复后重建一次通过） |
| `just mcp-up` | 在 checkpoint `b1e62f9` 上验证：`mcp-service` 与 `mcp-postgres` 启动至 healthy；其后的修复不触及该路径 |
| `curl http://127.0.0.1:18092/ready` | checkpoint 验证为 `{"status":"ready","checks":{"database":"ok"}}`；最终树 e2e 每场景均通过真实进程 `/ready` 轮询 |
| `curl http://127.0.0.1:18092/live` | checkpoint 验证为 `{"status":"ok"}` |

Compose 验证说明：共享开发 Nacos 缺少与本服务无关的 Agent/tracing 必填键，为使已文档化的 `just mcp-up` 在完整 Compose 文件插值下可执行，验证时向本地 Nacos 发布了一个临时隔离组（包含随机测试值）；验证后已删除该临时组与本地临时配置文件，未修改现有配置。MCP 三项秘密在 `compose-config` 中保持可选，缺失时由服务启动失败。

## 7. 变更与限制披露

- 实现使用运行时 SQL，无 `services/mcp/.sqlx/`；PRD 变更树已同步并在 Change Log 记录。
- `compose-config` 原先不识别 MCP 键，`just mcp-up` 无法使用；已补充 `MCP_DB_PASSWORD`、`MCP_CREDENTIAL_KEY_BASE64`、`MCP_CONSUMERS_JSON`、`MCP_PORT` 白名单与 `MCP_PORT` 校验，并保持 MCP 秘密非必填。
- 全仓依赖守卫原先未登记 `mcp-*` 层，`just test` 失败；已登记并通过。
- pre-PR 复审修复：`CreateConnectionRequest`、`SetCredentialRequest` 手写 `Debug` 以隐藏凭据明文，并增加脱敏单测；e2e 增加第二消费服务的独立 token/签名密钥，验证同租户字符串下跨消费服务的连接与调用隔离。
- verifier 复审修复：`ConnectionResponse` 的 `created_at`/`updated_at` 改为 RFC 3339 并加回归测试；rv-2 增加 HTTPS 云元数据、私网、IPv4 映射与 ULA 地址矩阵与开发 loopback 模式私网拒绝；e2e 增加真实不可用上游与健康连接并存的失败隔离场景；日志初始化把 `rmcp` 目标压回 `info`，e2e 以 `RUST_LOG=debug` 运行并断言无载荷泄漏；指南补充 JSON 响应在解析前的 16 MiB 内置上限与 rmcp 日志固定说明。负控为临时注入并用 `cmp` 核对恢复，未保留测试专用生产开关。
- pre-PR 复审（code-reviewer）修复：OpenAPI 的 `/mcp` 工具名格式更正为 `mcp_<connection-uuid-hex>_<远端工具名>`，创建与更新连接补充 `409 conflict` 响应，`endpoint_url` 增加 `maxLength: 2048`、凭据增加 `minLength`/`maxLength` 约束；核心用例在出站策略与数据库之前拒绝超过 2048 字符的 `endpoint_url`（返回 400 `invalid_request`，不再由数据库 CHECK 产生 500），并新增回归单测；指南同步 endpoint 长度上限与重名 409。
- 未连接真实第三方 MCP provider；外部网络、TLS 与供应商差异不在本机证据范围内。
- 未创建 PR；独立 verifier 结论与 PR 呈递由 runner 完成。
- 3 项 Human-Confirmed 保持未勾选，等待用户本人审阅 §9.1 呈递与服务范围。
