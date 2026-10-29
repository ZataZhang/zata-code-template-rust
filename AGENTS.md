# AI Agent Entry Guide

本文件是仓库的**跨工具 AI 入口适配层**，不是规范正文。
统一规范源在 `docs/ai-standards/`，详细架构权威文档在 `docs/architecture/system-design.md`。

## Read Order

任何任务都先读：

1. `docs/ai-standards/index.md` — 规范地图与权威关系

然后根据任务类型按需补读下表对应的页面，**不要无差别地一次性读完所有标准页**。

## When To Read Which Standard

| 标准页 | 何时必须读 | 何时可以跳过 |
|---|---|---|
| `docs/ai-standards/index.md` | 任何任务开始时；需求、方案与任务规划讨论时核对其中的 ROI 原则 | 永远不跳过 |
| `docs/ai-standards/architecture.md` | 新增/修改 `services/*/crates/` 下的模块、增删或修改任何 `Cargo.toml` 的 `[dependencies]`、新增端口或实现、新增对外 HTTP 入口 | 只改文档、测试、构建脚本时 |
| `docs/architecture/system-design.md` | 开始**任何**新的后端功能、新增层间契约、改动 composition root、新增独立服务边界 | 小范围 bugfix 或参数调整，不动层间契约时 |
| `docs/ai-standards/code-reuse.md` | **任何**新增或修改代码前；考虑提取 helper、判断是否复制粘贴、函数参数 ≥4 个、文件接近 500 行时；交付前必须完成其中的 **AI 编码自检清单** | 纯文档排版或纯配置改动且无代码逻辑变更 |
| `docs/ai-standards/naming.md` | 新增变量、函数、类型、模块、测试；命名风格不确定时 | 仅改文档时 |
| `docs/ai-standards/comments-docs.md` | 新增/修改公共 API；写 `///` / `//!`；使用 `#[allow]`；写 `unsafe` | 仅改私有实现细节且不涉及公共文档时 |
| `docs/ai-standards/documentation.md` | 改动公共函数签名、配置项、业务流程；新增长期文档页 | 纯内部重构且无对外行为或文档变化 |
| `docs/ai-standards/testing.md` | **任何代码变更开始前**；制定验证策略、收集证据或声称完成前必须重新核对；遇到测试失败时 | 仅改注释、纯文档排版或无可执行行为变化的纯配置 |
| `docs/ai-standards/migrations.md` | 新增、修改任何服务的 `migrations/`；写或改任何 SQL 查询；改动数据库表结构 | 不涉及数据库时 |
| `frontend-public/README.md`、`frontend-public/AGENTS.md` | 改动 `frontend-public/` 下任何内容；改动后端接口契约（字段、返回结构、错误码）；写前端代码前（Next.js 版本有破坏性变更，先读 `node_modules/next/dist/docs/`） | 只改后端内部实现、不影响接口契约时 |
| `docs/guides/deployment.md`、`configuration.md`、`backup.md` | 改 `docker-compose.yml`、`deploy/`、Dockerfile、配置项、备份脚本、反向代理或限流相关的 IP 来源时 | 其他任务 |
| `skills/prd/SKILL.md`、`docs/guides/prd-workflow.md` | 新增或执行 PRD（`tasks/pending/`）；改动 `tasks/` 下任何 PRD；提到「写 PRD」「按 PRD 实现」「归档」时 | 不涉及 PRD 时 |
| `docs/ai-standards/tooling.md` | 选择运行命令；改 `justfile` / Docker / compose 配置；增删依赖；撰写提交信息 | 已熟悉常用 `just` 命令、且本次不动工具链配置时 |

## Critical Summary

- 每次需求、方案与任务规划讨论默认评估 ROI，优先推荐最小有效方案；不为未提出的未来需求增加抽象。详见 `docs/ai-standards/index.md` 的「讨论与规划的 ROI 原则」。
- 项目以 AI Agent 理解并执行指令为核心；不为追求 100% 规范遵守率而逐条增加程序校验或硬门禁。能由编译器或现有守卫测试强制的约束（`unsafe`、`missing_docs`、依赖方向）除外。详见 `docs/ai-standards/index.md` 的「Agent 指令与程序边界」。
- 依赖方向（Cargo 拦截未声明的依赖，`services/accounts/tests/architecture.rs` 检查已声明的依赖，违规则 `just test` 失败）：账号私有层 `tpl-api`、`tpl-engines`、`tpl-infrastructure` 只依赖 `tpl-core`；`tpl-core` 不依赖内部 crate；只有 `tpl-accounts` 依赖这些账号层。后续业务服务不得依赖账号领域、仓储或迁移；确有跨服务复用需求时，先提取无业务归属的独立契约或平台 crate。**改任何 `Cargo.toml` 的依赖前先核对这条。**
- 账号业务规则写在账号私有的 `tpl-core`；`tpl-api` 只做参数校验、DTO 转换和调用用例；跨层协作经由 core 端口（trait）。其他服务不要把账户私有层当通用库。
- 使用 `cargo` 和 `just`。`just test` 是常规验证命令，任务要求运行时无需单独征求用户确认；提交前运行 `just ci`。
- 内部注释、`///` 文档注释统一使用中文（专有名词除外）；公共项必须有文档（`missing_docs` 会警告，`just lint` 里警告即失败）。
- 生产路径不用 `unwrap()` / `expect()`，用 `?` 返回有语义的错误；库层用 `thiserror`，`anyhow` 只用在 `main` 等应用入口。
- 变量命名必须具有来源、类型或状态语义，避免 `data`、`item`、`res`；用类型（enum、新类型）表达约束。
- 新增或修改代码前先搜索现有实现；禁止复制粘贴后微调，参数超过 4 个时收敛到结构体。
- 前端（`frontend-public/`）是后端之外的独立 Next.js 项目，只通过 `/api/*` HTTP 与后端通信。改后端接口契约时必须同步改 `frontend-public/lib/api/` 并重跑 `just web-e2e`（前端类型是手写的，编译器发现不了契约漂移）；前端按错误**码**取翻译，不要依赖后端文案。
- 管理后台（`frontend-admin/`）同理：改 `/admin/*` 契约时同步改 `frontend-admin/src/api/` 并重跑 `just web-e2e`；管理员域与普通用户域必须保持隔离（独立表、Cookie、Redis 键命名空间、限流前缀），不要为了复用把它们合并。
- 限流依赖 TCP 对端地址：账号服务只能经 `tpl_accounts::server::serve` 启动；不要为了「方便」信任 `X-Forwarded-For`（默认关闭，开启条件见 `config.toml`）；完整站点必须由会覆盖写该头的 nginx 代理，独立开发模式只绑定回环地址并关闭该信任（见 `docs/guides/deployment.md`）。
- 含密码的类型手写 `Debug`；日志与错误响应不得泄漏密钥、内部原因或连接串。
- 外部输入必须有上限与超时（请求体大小、集合长度、外部调用超时）。
- 除非用户明确要求，否则不要自动执行 `git add`、`git commit`、`git push` 等 Git 变更操作。
- 提交信息统一用英文并遵循 Conventional Commits（`type(scope): subject`，标题 ≤ 72 字符，正文解释 why），详见 `docs/ai-standards/tooling.md`。
- 单代码文件非空行不超过 1000 行（目标 500 行以内）。
- 执行 `tasks/pending/` 下任何 PRD 前先 `just prd start <prd-file>` 领取执行锁（知道工具名就 `--tool` 自报）；若报告他人新鲜锁，**立即停止并向用户报告持锁者**，不要继续实现，也不要自行 `release --force`。长任务定期 `just prd heartbeat <prd-file>`。详见 `docs/guides/prd-workflow.md`。
- PRD 对应任务全部完成后：先 `just prd check --archive-ready <prd-file>` 通过，再把 PRD 从 `tasks/pending/` 移到 `tasks/archive/`。**归档只代表执行侧交付完成，不等人工验收**：`Human-Confirmed` 空框带着 `🧍 待人工验收` 横幅归档，只有人能勾选它们、把横幅改成 `✅ 已验收`；执行方不得代勾，也不得把它们改写成 `[~]`。
- 随手想法先落 `tasks/inbox/`：只在**想法边界**捕获（用户明确要求，或话题收敛到可开 PRD 的状态），条目 = 用户原话逐字**引用** + 标注为 `AI 派生` 的背景块，**只追加**到 `tasks/inbox/ideas.md`（禁止改写已有条目）；AI 维护 `tasks/inbox/summary.md` 做总结。详见 `docs/guides/idea-inbox.md`。
- 禁止为了可测性修改生产代码：不得为了让测试或负控能变红而往生产代码里加故障注入开关、失败模式、test-only 配置项、计数器或观测钩子；无法变红时记 `negative_control: not feasible — <原因>`。
- 守卫测试（如将来新增）失败时，修复触发它的源码或配置，不要修改守卫测试本身让测试通过；仅当约定本身变更时才改守卫测试，并同步更新对应文档。
- 验证措辞要如实：没执行的验证写 `not run`；只验证到某一层就写那一层，不要把「单元测试通过」写成「端到端通过」。缺凭据或缺外部服务时停下来报告，不要用替身顶上去宣称完成了真实入口验证。
- 变更代码时同步更新 `docs/`、`///` 文档注释、`config.toml` 与 `.env.example`。
- 新增给密钥类的配置变量放到 `config.toml`，实际密钥值由 `.env.local` / 环境变量注入；默认 env 未填写时，配置加载仍须正常完成。`.env.example` 里默认必填的变量保持未注释，可选的保持注释，密钥值始终留空。
- 本仓库原生启动服务时默认从 Nacos 应用 TOML 读取数据库和 Redis 配置；`.env.local` 只提供 Nacos 地址与认证信息。启动命令不要主动内联 `DATABASE_URL` / `REDIS_URL` 覆盖 Nacos，除非用户明确要求覆盖或切换到本地测试依赖。详见 `docs/guides/configuration.md`。
- 迁移状态与尚未迁移的内容见 `docs/migration-status.md`。

## Maintenance Rule

- 共享规范优先写入 `docs/ai-standards/`，不要把长篇规则重新复制回本文件
- 新增标准页时，同时在上方 **When To Read Which Standard** 表中加一行触发条件，保持入口可路由
