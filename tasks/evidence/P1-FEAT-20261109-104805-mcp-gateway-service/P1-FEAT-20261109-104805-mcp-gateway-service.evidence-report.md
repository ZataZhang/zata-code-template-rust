# MCP Gateway 服务验收证据报告

## 人审导航 / Human Review Navigation

本 PRD 的人审呈递区（§9.1）是服务范围与 API 契约；审查对象是仓库内文档与 OpenAPI，不涉及页面截图。

| 呈递 | 绝对路径 | 打开命令 | 期望值 | 执行侧交叉核对 |
|---|---|---|---|---|
| MCP 服务指南 | `/Users/zata/code/rust-study/.kc-worktrees/issue-4/docs/guides/mcp-service.md` | `open "/Users/zata/code/rust-study/.kc-worktrees/issue-4/docs/guides/mcp-service.md"` | 范围与 API 两节列出连接管理、用户/分组授权、连接检查与 `/mcp` Streamable HTTP 网关，并明确排除公开 MCP 市场与消费方 UI | `rv-3-docs-scope.txt` 已校验 API 路径、`/mcp` 与排除项关键词 |
| API 契约 | `/Users/zata/code/rust-study/.kc-worktrees/issue-4/services/mcp/openapi.yaml` | `open "/Users/zata/code/rust-study/.kc-worktrees/issue-4/services/mcp/openapi.yaml"` | `paths` 包含 7 条 `/api/v1/connections...` 管理路径与 `POST /mcp`，错误码与鉴权说明完整 | 与 `services/mcp/crates/api/src/routes/*.rs` 逐路径比对一致；运行中服务 `GET /openapi.yaml` 返回同一契约 |

本项没有仍需要人审的静态图片；审查按上表直接打开最终树中的 Markdown 与 YAML。PR 由 runner 创建，此处暂无 PR/CI 链接。

## 1. 结论

- §9 的执行侧验收项由最终工作树的真实入口证据覆盖；3 项 Human-Confirmed 保持未勾选，等待人工确认。
- `rv-1`、`rv-2`、`rv-3` 均在当前 `HEAD 222bc51bcda235d07aae4567c6da92752da2897e` 加最终工作树改动后重跑并通过；证据输出保存在本目录。
- `just test`、`just test real`、`just ci`、响应上限定向测试、Docker 构建与隔离 Full stack smoke 均通过；远端 GitHub 检查需由 runner 在提交/推送后执行。
- `kc evidence check --evidence tasks/evidence/P1-FEAT-20261109-104805-mcp-gateway-service`：3/3 PASSED。
- 证据身份绑定：分支 `issue-4`，HEAD `222bc51bcda235d07aae4567c6da92752da2897e`，基线 `7be157439c698e75e0e821fe30693857338aa530`，工作树 patch SHA-256 `e8628d6343f4948d9d6d6a7cf857ab759c204b33a048c73a9545800f2185747e`。

## 2. 最终代码状态

| 字段 | 值 |
|---|---|
| 分支 | `issue-4` |
| HEAD | `222bc51bcda235d07aae4567c6da92752da2897e` |
| 基线 | `7be157439c698e75e0e821fe30693857338aa530`（`merge-base HEAD main`）|
| 工作树 patch SHA-256 | `e8628d6343f4948d9d6d6a7cf857ab759c204b33a048c73a9545800f2185747e` |
| patch 指纹范围 | `git diff --binary HEAD` 加排序后的 2 个未跟踪源码文件路径与内容 |
| 工作树源码指纹 SHA-256 | `c9047915fbb6373a4d3d2d44450ef5ce982ca8dbb9bf8ff9e2e55153d33b2da9` |
| 指纹文件数 | 587（tracked + untracked；排除 `tasks/evidence/**`、本 PRD 与 `.agent-runner/**`）|
| Rust / Cargo | `rustc 1.99.0` / `cargo 1.99.0` |
| Docker | `29.4.2` |
| 记录时间（UTC） | 2026-11-09T09:23:50Z |

当前 RV 文件 SHA-256：

| 证据文件 | SHA-256 |
|---|---|
| `rv-1-authorization.txt` | `1a1a7f21c0695b43be5df5d27c9f4974b324b76cc2c47ccbe0db52f450594523` |
| `rv-2-remote-connection.txt` | `486ba0dac763a8b8a70c3bf7f9d68c19eed69fc44523354ad6c45b4b520fbf50` |
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
| `cargo check -p tpl-mcp-infrastructure` | exit 0（最终工作树）|
| `cargo test -p tpl-mcp-infrastructure rejects_oversized_json_before_deserializing_the_response` | exit 0；无 `Content-Length` 的分块 JSON 在反序列化前超过 1 KiB 限值即被拒绝 |
| `cargo test -p tpl-mcp --test remote_response_limit` | exit 0；跨页累计响应上限回归通过 |
| `just test` | exit 0（完整工作区测试）|
| `just test real` | exit 0（真实 PostgreSQL 与服务进程测试）|
| `just ci` | exit 0（fmt、workspace clippy `-D warnings`、测试与 rustdoc）|
| `BUILDX_CONFIG="$PWD/.agent-runner/buildx" just mcp-docker-build` | exit 0；镜像 manifest `sha256:d4f9efb6d9019ff042a17e23297c05abf223a53ddc47bde4b8fca703c80d7a1d` |
| 隔离 Full stack（`scripts/ci-nacos-stack.py`，含 `just stack-smoke`） | exit 0；入口、认证隔离、限流、备份与恢复全部通过。`full-stack-smoke.txt` 保存运行结果 |
| 备份 SIGPIPE 负控 | 原管道 paced 输出状态为 `141 0`，完整消费管道为 `0 0`；上游失败码 `42` 仍保留，缺表仍以 `1` 拒绝。详见 `backup-sigpipe-regression.txt` |
| `just mcp-up` | 在 checkpoint `b1e62f9` 上启动 `mcp-service` 与 `mcp-postgres` 至 healthy；最终树 e2e 各场景继续通过真实 `/ready` 轮询 |
| `/ready` 与 `/live` | checkpoint 上 `/ready` 返回数据库 `ok`，`/live` 返回 `status=ok`；最终树 e2e 的 `/ready` 轮询通过 |
| `kc evidence check --evidence tasks/evidence/P1-FEAT-20261109-104805-mcp-gateway-service` | 3/3 PASSED |

Full stack 使用隔离 Compose 项目和专用网络；没有对共享开发数据库执行删除/恢复。远端 GitHub 检查未在本地伪报为通过，需 runner 在后续提交/推送后执行。

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

## 8. Reviewer findings

1. **Realistic Validation 证据版本过期 — 已修复。** `rv-1`、`rv-2`、`rv-3` 在当前 `HEAD 222bc51` 与当前工作树改动上重跑；`evidence.json` 和本报告记录非空的 `head`、`base`、`patch`、`branch` 及源码指纹，Kc 检查 3/3 通过。
2. **JSON 响应上限在解析后才生效 — 已修复。** RMCP HTTP client 外包一层流式有界客户端，在 JSON bytes 进入 serde 前按配置限流；保留 RMCP 16 MiB 硬上限作为兜底，并继续限制 SSE 事件与分页累计大小。分块响应回归测试覆盖无 `Content-Length` 情况；配置与指南同步。
3. **放行 `2001:2::/48` — 有争议，维持拒绝策略。** 本服务明确只允许全局可达的目标；IANA 登记表把 `2001:2::/48` 标为 Benchmarking 且 `Globally Reachable=False`，故它属于 `2001::/23` 默认拒绝范围。rv-2 证明注册与消费阶段均拒绝该地址。依据：[IANA IPv6 Special-Purpose Address Registry](https://www.iana.org/assignments/iana-ipv6-special-registry)。
4. **Agent 备份校验 SIGPIPE — 已修复并有本地完整栈证据。** `db-backup.sh` 不再用 `grep -q` 提前退出；真实 TOC 的 paced 管道负控验证了旧行为 141 与修复后 0，并验证生产者错误与缺表检测仍保留。隔离 Full stack smoke（入口、认证隔离、限流、备份与恢复）通过。远端 GitHub gate 必须等 runner 提交/推送后执行。
5. **慢上游串行阻塞 `tools/list` — 已修复。** 连接发现使用 `buffer_unordered(8)`，限制并发并隔离单连接失败；workspace CI 测试通过。
