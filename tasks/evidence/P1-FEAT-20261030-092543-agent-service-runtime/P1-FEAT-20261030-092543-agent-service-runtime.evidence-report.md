## 人审导航 / Human Review Navigation

先看目标原型 [`docs/prototypes/agent-service-runtime.html`](../../../docs/prototypes/agent-service-runtime.html)（本机运行 `open "docs/prototypes/agent-service-runtime.html"`），再对照下面三张真实实现截图。三张图均为 **本机验证截图**（2026-11-06 在 rebase 与 Blob 路径加固后的代码树重跑 rv-1 产生），文件位于 evidence 目录、不进入代码 diff；代码页可能不显示二进制图片，请按下方本机路径打开。

| 呈递物 | 本机路径与打开命令 | 你应看到什么 | 实际画面 |
|---|---|---|---|
| 组织 / Runtime / Agent 管理 | `tasks/evidence/P1-FEAT-20261030-092543-agent-service-runtime/rv-1/organization-runtime-agent.png`；运行 `open "tasks/evidence/P1-FEAT-20261030-092543-agent-service-runtime/rv-1/organization-runtime-agent.png"` | 组织选择器、builtin sandbox 与外部 A2A Runtime、绑定 Runtime 的 Agent 列表、创建 Agent 与授权入口 | ![组织管理员配置 Runtime 与 Agent](rv-1/organization-runtime-agent.png) |
| 用户 Session / Run | `tasks/evidence/P1-FEAT-20261030-092543-agent-service-runtime/rv-1/user-session-run.png`；运行 `open "tasks/evidence/P1-FEAT-20261030-092543-agent-service-runtime/rv-1/user-session-run.png"` | 真实 A2A 助手回复、Run ID、状态 `succeeded`、Agent snapshot checksum、事件序号 | ![用户在公开站点创建 Session 并完成 Run](rv-1/user-session-run.png) |
| tracing Run 详情 | `tasks/evidence/P1-FEAT-20261030-092543-agent-service-runtime/rv-1/tracing-run.png`；运行 `open "tasks/evidence/P1-FEAT-20261030-092543-agent-service-runtime/rv-1/tracing-run.png"` | tracing 列表搜索到同一 Run ID，来源 `agent-service / agent-runtime`，状态成功 | ![tracing 中同一 Run 的脱敏事件](rv-1/tracing-run.png) |

约 10 秒自检：三张图都来自同一次 rv-1 重跑；Session 页与 tracing 列表中的 Run ID 相同，Session 页状态为 `succeeded`，tracing 列表状态为成功。事件投影中的 `input.accepted` 内容为 `[已脱敏]`，不含原始提示词。每次复跑都会生成新的 fresh-state Run ID，因此本报告不固化 ID；以截图与 evidence 文本为准。

### 目标原型与实现截图配对

目标原型三态截图由 `scripts/capture-prototype.mjs` 以 1500×1100 视口渲染，和 rv-1 实现截图同视口、同类状态。原型数据是设计示例，实现数据来自本次 rv-1 真实运行；原型图是设计意图，不是运行证据。

| 状态 | 目标原型（设计意图） | 真实实现（rv-1 运行证据） |
|---|---|---|
| 组织 / Runtime / Agent 管理 | ![原型：组织与 Agent 目录](prototype/agent-service-runtime-admin.png) | ![实现：组织与 Agent 管理](rv-1/organization-runtime-agent.png) |
| 用户 Session / Run | ![原型：Session 工作台](prototype/agent-service-runtime-session.png) | ![实现：用户 Session/Run](rv-1/user-session-run.png) |
| Tracing Run 详情 | ![原型：Tracing 详情](prototype/agent-service-runtime-tracing.png) | ![实现：tracing Run 详情](rv-1/tracing-run.png) |

## 真实验证执行记录

执行日期：2026-11-06。RV-1～RV-6 与 `kc evidence check` 在 `18f0d35d7371229d616ad49c80f9fbd0fef94cba` 上执行；该提交包含 rebase 后的 Agent 功能与 Blob 根目录/符号链接边界加固。随后 `f21be74`、`48f1340` 修复 Docker / pnpm 构建集成配置，`3001d8a` 仅将既有代码等价拆分到子模块，`8fdd12e` 刷新本地 UI tarball 的锁文件校验和，`97bded0` 补齐 tracing 镜像的 workspace build context；这些提交不改变功能行为。`just ci` 与 `just check-lines` 在 `3001d8a` 上通过；重建 UI tarball 后，在全新临时目录按 frozen lock 安装独立 host 并通过类型检查和构建。Issue 的 Realistic Validation 清单包含 rv-1～rv-3 三项，`evidence.json` 的 item 1～3 与之一一对应；rv-4～rv-6 是 PRD §7.6 追加的补充 oracle（边界、业务微服务样例、独立部署 Runtime），在该行为基线提交上执行并把输出保存在本目录，但不作为 Issue 清单项写入 manifest（Issue 清单只有 3 项）。所有场景都通过真实入口运行：真实浏览器（Playwright Chromium，经 `CHROME_PATH` 指定）→ public Next.js 站点 Route Handler → Agent HTTP API → accounts `/auth/me` 或 `/admin/auth/me` → Organization/ACL → Sandbox 或真实 A2A adapter → Agent PostgreSQL + outbox → tracing producer HTTP API → tracing PostgreSQL/query API。除 rv-6 使用独立部署的官方 `a2a-sdk==1.3.0` 参考 Runtime 外，其余场景的外部 A2A endpoint 由本地协议替身模拟（PRD 允许）；所有组件均未使用内存替身，也未直连数据库伪造结果。

| 检查点 | 命令 | 结果 | 关键输出 |
|---|---|---|---|
| **Issue RV-1**（manifest item 1）组织/Runtime/Agent/Session/Run 全链路 | `bash tasks/evidence/P1-FEAT-20261030-092543-agent-service-runtime/scripts/rv-1-organization-runtime-session-lifecycle.sh` | 60/60 项通过 | 幂等键返回同一 Run；取消后 Run 为 `cancelled`；Agent 更新后新 Run 使用新 snapshot、旧 Run snapshot 不变；受控 A2A 错误写成 `Failed` 终态；resources/skill/child Run/Auto 全覆盖；tracing 查询到同一 Run 的完整有序事件 |
| **Issue RV-2**（manifest item 2）身份、ACL 与脱敏 | `bash tasks/evidence/P1-FEAT-20261030-092543-agent-service-runtime/scripts/rv-2-authorization-and-redaction.sh` | 32/32 项通过 | 缺失/无效服务凭据 401 `service_unauthorized`；伪造用户 401；跨组织 403 `organization_membership_required`；部门未授权/个人 deny 403 `agent_access_denied`；owner 隔离按不存在处理；标记秘密不出现在事件/tracing/日志 |
| **Issue RV-3**（manifest item 3）tracing 停机恢复 | `bash tasks/evidence/P1-FEAT-20261030-092543-agent-service-runtime/scripts/rv-3-tracing-outage-recovery.sh` | 20/20 项通过 | 真实停止 tracing 进程期间 Run 仍成功；恢复后 outbox 全部投递、游标推进；tracing 事件完整、有序、唯一，重投无重复 |
| 补充 rv-4 请求体/集合/上传/超时/并发边界（PRD §7.6） | `bash tasks/evidence/P1-FEAT-20261030-092543-agent-service-runtime/scripts/rv-4-boundary-limits.sh` | 21/21 项通过 | 300 KiB JSON → 413 `payload_too_large`；100,001 字符正文 → 400；21 个 resource/skill 引用 → 400；10 MiB+1 上传 → 413，限额内上传 201；A2A 无响应在 ≥55s 后按 60s 超时写成 `failed`；5 个 SLOW Run 恰好 4 running/1 queued，取消后全部终态 |
| 补充 rv-5 独立业务微服务示例（PRD §7.6） | `bash tasks/evidence/P1-FEAT-20261030-092543-agent-service-runtime/scripts/rv-5-business-service-sample.sh` | 22/22 项通过 | 示例经 Agent HTTP 契约创建 Run 201/`created=true`、幂等重放 200/`created=false` 同一 Run、查询与取消响应与直连逐字段一致；错误码 404 `run_not_found`、401 `unauthenticated`、400 `invalid_request` 透传；`cargo tree` 不含 `tpl-agent`/`sqlx`/`postgres`/`redis`，无 Agent 数据库配置 |
| 补充 rv-6 独立部署官方 A2A SDK Runtime（PRD §7.6） | `bash tasks/evidence/P1-FEAT-20261030-092543-agent-service-runtime/scripts/rv-6-real-runtime.sh` | 66/66 项通过 | 以 `uv` + Python 3.12 启动 `a2a-sdk==1.3.0` 官方参考实现（Agent Card `Real A2A SDK Echo`、JSONRPC/1.0），登记为外部 Runtime 并探测 Ready；真实 Run（fresh-state ID 见证据文件）到达 `succeeded`，回复含独立实现产出的 `Real A2A SDK reply`；脚本 sha256 已记录 |
| 交付门禁自检 | `kc evidence check --evidence tasks/evidence/P1-FEAT-20261030-092543-agent-service-runtime` | 3/3 PASSED | item 1 40.3s、item 2 24.3s、item 3 31.1s，均低于 300s 复跑上限；门禁运行于最终代码提交（manifest 只含 Issue 清单的 3 项） |

负控执行方式：

- rv-1：场景内由 A2A 协议替身返回受控错误，断言真实 adapter 把 Run 写成 `Failed`、tracing 也是 `failed`；若 adapter 吞错或伪报成功，对应检查打印 `FAIL`，汇总不再是 60/60，命令非零退出。
- rv-2：每个授权面都先发真实未授权请求（缺失/错误服务凭据、伪造 Cookie、仅 `X-User-Id`、跨组织、部门未授权、个人 deny、他人 Session/Run），再断言拒绝码与零副作用；任一请求被错误接受即变红。
- rv-3：真实停止 tracing 进程作为故障注入；若 Run 因 tracing 失败、outbox 丢事件或恢复后重复/乱序，对应检查打印 `FAIL`，汇总不再是 20/20。
- rv-4：修复前该探测已真实变红——JSON 413 返回框架纯文本、public 上传在 256 KiB 处连接重置；修复后 21/21 通过。移除 413 归一化中间件或恢复 public 上传的全局上限会重现失败。
- rv-5：示例服务对不存在的 Run、缺少用户 Cookie、缺少 `Idempotency-Key` 三种请求必须透传 Agent 的真实错误码；若示例伪造成功或改写错误码，对应检查打印 `FAIL`，汇总不再是 22/22。
- rv-6：真实 Runtime 未配置或不可达时包装脚本启动即失败；若 adapter 与该独立实现不能互操作，`真实 Runtime 完成 Run 并返回独立实现回复` 打印 `FAIL`。修复前该场景已真实变红：adapter 把携带 `taskId` 的直接 Message 误当 Task 轮询，`GetTask` 返回 `-32001 Task not found`；修复后 66/66 通过。

## 配置与日志脱敏审阅（R3）

- 配置键：`docs/guides/agent-service.md` §8 明确连接串、`AGENT_SERVICE_TOKENS_JSON`、`AGENT_TRACING_PRODUCER_TOKEN`、`AGENT_CREDENTIAL_<ALIAS>` 只从环境或部署平台注入；`config.toml` 与 `.env.example` 不保存真实秘密。
- 代码审阅：`services/agent/crates/infrastructure/src/config.rs` 对 `DatabaseSettings`、`IdentitySettings`、`TracingSettings` 手写 `Debug`，连接串、服务 token 清单和 producer token 只显示“已配置，已隐藏”；Nacos 配置错误测试断言错误文本不回显粘贴的秘密。
- Blob 存储审阅：`FilesystemBlobStore` 拒绝跨平台路径穿越，逐级解析配置根目录内的父目录；读写路径均拒绝越出根目录的符号链接，单元测试覆盖目录链接写入和文件链接读取/删除。
- 运行断言：rv-2 的“Agent 日志不包含标记秘密、完整提示词或服务凭据”“标记秘密保存在 owner 可见的消息中，但不出现在 Run 事件投影”“tracing 查询到同一 Run 且响应不含标记秘密”均通过。
- 限制：日志样本来自本机真实运行，不是生产环境日志审计；生产日志轮转、采集侧脱敏仍需部署后复核。

## 验收清单状态（与 PRD §9 同步）

- 已勾选：rv-1/rv-2/rv-3/rv-4/rv-5/rv-6 覆盖项、配置/日志脱敏审阅、前端构建与真实浏览器入口、Cargo 依赖守卫、文档、最终证据 provenance、§9.1 呈递。
- 原先未完成的两项执行侧验证已补齐：rv-6 使用独立部署的官方 `a2a-sdk==1.3.0` 参考 Runtime 产生真实 Run；rv-5 新增独立业务微服务示例并验证只经 Agent HTTP 契约调用。两者仍有明确限制，见“本机环境与已知限制”。
- 生产 `docker-compose.yml` 整栈未在本机执行；已在“本机环境与已知限制”标注 not run。
- 所有 `Human-Confirmed` 项保持未勾选，等待用户审阅；PRD 横幅为 `🧍 验收状态：待人工验收`，Final Reconciliation 不再记录 validation blocker。

## 本机环境与已知限制

- 运行环境：macOS arm64；`docker-compose.testing.yml` 提供独立 PostgreSQL（5433/5434/5435）与 Redis（6390），不触碰生产库。
- 证据场景分别重置测试库并在末尾清理测试账号；每轮生成新的组织、成员、Agent、Session、幂等键和 Run ID（fresh state）。
- 发现并修复了 `scripts/agent-e2e.sh` 在 macOS `/bin/bash` 3.2 下把 `$VAR` 后的全角标点并入变量名的问题（改成 `${VAR}`）；kc 的 `bash -lc` 会解析到 3.2，修复前 `kc evidence check` 0/3。
- **完成（有限制）**：rv-6 已用独立部署的官方 `a2a-sdk==1.3.0` 参考 Runtime（不是仓库内协议替身）完成真实 Run，Agent Card、协议版本、来源与脚本 sha256 均记录在证据文件中。该 Runtime 是本机部署的开源参考实现，不是第三方厂商生产端点；因无生产 Runtime 凭据，本项不能宣称对某供应商生产环境的可用性。
- **not run**：`docker-compose.yml` 整栈生产形态与 CI/CD 发布流程未在本机执行；前端构建分别由 `just web-check`、`just web-admin-check`、`just web-tracing-check` 覆盖。
- **完成（有限制）**：rv-4 已通过真实 Agent HTTP 入口观察 256 KiB JSON、100,000 字符正文、20 个集合引用、10 MiB 上传、A2A 60s 超时与并发 4（21/21）。探测发现并修复了 public 上传路由沿用 256 KiB 全局上限、框架 413 未返回 JSON 错误码两处缺陷；外部 A2A 仍为本地协议替身（HANG/SLOW），60s 观察不早于 55s，并发状态来自单实例数据库查询，不代表多实例分布式调度。
- **完成（有限制）**：rv-5 已新增 `examples/agent-business-service` 并验证创建/查询/取消三个契约端点、幂等重放与错误码透传（22/22）；示例不依赖 Agent 私有 crate 或数据库。示例不覆盖上传、SSE 与生产网络策略。
- 截图、原型截图与脚本位于本机 evidence 目录、不进入代码 diff；代码页可能不显示二进制图片，请按本报告中的本机路径打开。原型截图为设计意图，不是运行证据。

## 其他验证

- `just web-check`：Next.js 生产构建通过，路由包含 `/api/agent/[...path]` 与 `/app/agents`。
- `just web-admin-check`：Prettier、`tsc`、Vite 构建通过，包含 organizations / agent-runtimes / agents 页面。
- `just web-tracing-check`：`tsc` 与 Vite 构建通过。
- `just web-admin-test`：10 个测试文件、57 个用例通过。
- `TEST_AGENT_DATABASE_URL=postgres://agent:agentpass@localhost:5435/agent_dev cargo test -p tpl-agent --test http_end_to_end -- --ignored`：1 passed / 0 failed，新增的 300 KiB JSON → 413 `payload_too_large`、300 KiB 上传 → 201、1 MiB+1 上传 → 413 断言均执行并通过（本机测试库，不代替 rv-4 的真实入口证据）。
- `cargo test -p agent-business-service`：9 passed / 0 failed（契约解析 4、上游错误映射 3、路径校验 2）；`cargo clippy -p agent-business-service --all-targets -- -D warnings` 通过。
- `cargo test -p tpl-agent-engines`：25 passed / 0 failed，含新增的 `direct_message_completes_even_when_the_message_carries_a_task_id` 与 `non_terminal_task_requires_polling`；`cargo clippy -p tpl-agent-engines --all-targets -- -D warnings` 通过。
- `just ci`：2026-11-06 在 `3001d8a` 上通过（exit 0）——`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、`cargo doc --workspace --no-deps` 均成功，包含 Blob 路径穿越与符号链接回归测试。
- `just check-lines`：2026-11-06 在 `3001d8a` 上通过；超限的 A2A 与 admin 文件已按职责拆分。
- `pnpm pack:tarball`：在 `packages/accounts-ui/` 连续两次产出相同 SHA-512，且与 Linux CI `npm pack` 输出一致。
- 独立临时目录验证：复制 `examples/accounts-host/`、锁文件与新 tarball 后，使用 pnpm 11.3.0 执行 `pnpm install --frozen-lockfile`、`pnpm typecheck`、`pnpm build` 均通过；`minimumReleaseAgeExclude` 只覆盖未发布的本地 `@tpl/accounts-ui`。

## 代码树与证据文件

- 真实入口证据提交：`18f0d35d7371229d616ad49c80f9fbd0fef94cba`；tree：`6f0d624ffb3262cd8a83dd0cc152bbe16c9afa2e`。后续 `f21be74`、`48f1340`、`3001d8a`、`8fdd12e`、`97bded0` 分别修复 Docker workspace 构建上下文、pnpm 本地 tarball 发布时限检查与锁文件校验和，并将 A2A/admin 实现等价拆分到子模块；`just ci` 和 `just check-lines` 在 `3001d8a` 上通过，全新临时目录中的 accounts-host frozen 安装/类型检查/构建在 `8fdd12e` 上通过。
- PR 基线 `origin/main`：`5851023c`；分支另包含 `01ddeb4`（修正仓库 Runner 对安全 `.env.example` 的误拦截），该修复与 Issue #1 的提交门禁阻塞直接相关。
- 原始实现基线：`d615578a18bb5b2d4ba482d467086cf3dc71cd28`。2026-11-04 的未提交工作树指纹为历史证据，已由上面的最终提交验证取代。
- `just ci` 在最终代码提交上 exit 0；见上节。
- 结构化 manifest：[`evidence.json`](evidence.json)；manifest 只覆盖 Issue Realistic Validation 清单的 3 项：

| Manifest Item | 对应 Issue 条目 | 证据文件 |
|---|---|---|
| 1 | rv-1 | `rv-1-organization-runtime-session-lifecycle.txt`、`rv-1-organization-runtime-agent.png`、`rv-1-user-session-run.png`、`rv-1-tracing-run.png` |
| 2 | rv-2 | `rv-2-authorization-and-redaction.txt` |
| 3 | rv-3 | `rv-3-tracing-outage-recovery.txt` |

补充 oracle（PRD §7.6，不在 Issue 清单与 manifest 内，输出同样保存在本目录）：`rv-4-boundary-limits.txt`、`rv-5-business-service-sample.txt`、`rv-6-real-runtime.txt`。

复跑脚本位于 [`scripts/`](scripts/)（只随证据分支上传，不进入代码 diff）。
