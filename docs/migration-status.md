# 迁移状态：Python 模板 → Rust

本仓库是 `zata_code_template`（Python / FastAPI）的 Rust 版。迁移**分阶段**进行，先做一条最小竖切跑通，再逐步搬。本页记录已经迁移的、刻意推迟的，以及推迟的原因，便于接着做。

> 状态更新日期：2026-10-29。阶段 1（骨架）、2（认证竖切，含限流）、3（前台站点）、4（PRD 工具链，Rust 重写）、5（管理员账号与管理后台）、6（部署、配置与备份）已完成；账号与认证现已作为独立 `accounts-service` 装配和部署，并新增可被不同前端宿主安装的账号 UI 包。

## 已迁移

### 阶段 1：骨架

| 内容 | 位置 | 说明 |
|---|---|---|
| 分层架构（四层 + 装配层） | `services/accounts/crates/*`、`services/accounts/` | `services/accounts/crates/*` 是账号服务私有层，装配根与二进制位于 `services/accounts/`；守卫测试检查其依赖方向 |
| 健康探针 `/live` `/health` `/ready` | `services/accounts/crates/api`、`services/accounts/crates/core` | `/ready` 通过端口探测 PostgreSQL，503 时不泄露原因 |
| 配置加载 | `services/accounts/crates/infrastructure/src/config.rs`、`config.toml` `[nacos]` | 默认值 < `config.toml` < 可选 Nacos TOML < 环境变量；Nacos 定位元数据保存在项目配置中，本地 `.env.local` 只装地址与认证凭据 |
| 日志 | `services/accounts/crates/infrastructure/src/logging.rs` | `tracing`，文本/JSON 两种格式 |
| 数据库连接池 | `services/accounts/crates/infrastructure/src/persistence.rs` | sqlx，惰性重连 |
| 优雅退出 | `services/accounts/src/shutdown.rs` | Ctrl+C / SIGTERM |
| 统一命令入口 | `justfile` | 精简重写（约 90 行），不是照搬 1500 行的 `justfile.shared` |
| 本地中间件 | `docker-compose.testing.yml` | PostgreSQL + Redis，端口 5433 / 6390 |
| 镜像 | `Dockerfile` | 多阶段构建，非 root |
| AI 规范 | `AGENTS.md`、`CLAUDE.md`、`docs/ai-standards/` | 按 Rust 重写，保留与语言无关的原则 |
| Skills | `skills/idea-inbox`、`skills/code-reviewer` | idea-inbox 原样迁移；code-reviewer 改写为 Rust 版 |
| 任务目录骨架 | `tasks/{inbox,pending,archive}` | 流程约定见 `docs/guides/idea-inbox.md` |

### 阶段 2：认证竖切

| 内容 | 位置 | 说明 |
|---|---|---|
| 用户注册、登录、退出、`/auth/me` | `services/accounts/crates/core/src/auth.rs`、`services/accounts/crates/api/src/auth.rs` | 邮箱 + 密码；设计与安全取舍见 `docs/architecture/system-design.md`「认证」 |
| 密码哈希 | `services/accounts/crates/infrastructure/src/password.rs` | argon2id，在 `spawn_blocking` 里计算 |
| HTTP-only Cookie 会话 + Redis 会话存储 | `services/accounts/crates/infrastructure/src/redis_session.rs` | 令牌 256 位随机；Redis 里只存令牌的 SHA-256 |
| 登录与注册限流 | `services/accounts/crates/core/src/auth/throttle.rs`、`services/accounts/crates/infrastructure/src/redis_attempts.rs` | 按 IP、按账号、注册按 IP；不泄露账号是否存在；默认只信任 TCP 对端地址；Redis 固定窗口；详见 `docs/architecture/system-design.md`「认证」 |
| 数据库迁移 | `services/accounts/migrations/`、`persistence::run_migrations` | `sqlx migrate`，启动时自动执行；表和列带 `COMMENT`；约定见 `docs/ai-standards/migrations.md` |
| 用户仓储 | `services/accounts/crates/infrastructure/src/persistence/user_repository.rs` | SQLx 宏编译期校验 SQL、列名与绑定类型；值使用参数绑定；唯一约束映射为 `EmailTaken` |
| 依赖方向守卫测试 | `services/accounts/tests/architecture.rs` | 读 `cargo metadata` 对照允许名单；自带「故意违规必须报错」的负控 |
| 端到端测试 | `services/accounts/tests/http_end_to_end.rs` | 真实装配 + 真实 socket；认证全流程用真实 PostgreSQL 与 Redis（`just test real`） |
| `just sync` | `justfile` | `cargo fetch --locked`（Rust 没有单独的安装步骤） |

### 阶段 3：前台站点

| 内容 | 位置 | 说明 |
|---|---|---|
| 前台站点 | `frontend-public/` | 从 Python 模板复制，并**适配 Rust 后端的接口契约**：登录字段 `identifier`→`email`、返回结构 `{user:{id,email}}`、去掉 `display_name`、注册后自动登录、密码规则 8 字符/128 字节、错误按 `code` 翻译、429 显示等待时间 |
| 修掉的原有问题 | 同上 | ① 全局 401 拦截器会在登录失败时整页刷新，吞掉错误提示；② `language-switcher` 的 lint 错误（`react-hooks/immutability`）；③ 文案里的 Python 模板描述（Ruff、Alembic、双认证域、双前端、`src/backend/...`）改成对 Rust 项目真实成立的说法 |
| 独立 pnpm 项目 | `frontend-public/pnpm-workspace.yaml`、`frontend-public/Dockerfile` | 不再是 Rust/Cargo workspace 成员；Dockerfile 以 `BACKEND_URL` 构建参数传入账号服务地址 |
| 真实浏览器端到端测试 | `frontend-public/e2e/auth.e2e.mjs`、`scripts/web-e2e.sh`、`just web-e2e` | 20 项检查：注册/登录/退出/Cookie 属性/限流（中英文）/账号互不影响；自动起停数据库、后端、前端并清理 |
| pre-commit 钩子 | `.pre-commit-config.yaml`、`scripts/check-commit-msg.sh`、`scripts/check-i18n-keys.mjs` | 文件卫生、cargo fmt/clippy、依赖方向守卫、前端类型/lint、i18n 对齐、提交信息格式；负控已验证（坏格式、违规依赖、坏提交信息都被拦） |
| CI | `.github/workflows/ci.yml` | 5 个任务：hooks / rust / web / web-e2e / docker；已过 `actionlint`，**尚未在真实 GitHub runner 上运行** |
| 前端 just 命令 | `justfile` | `web-install`、`web-dev`、`web-check`、`web-e2e`、`web-docker-build`、`ci-all` |

### 阶段 5：管理员账号与管理后台

这是第一个完整用 PRD 工作流（`tasks/` → `tasks/archive/`）做的功能，PRD 见 `tasks/archive/` 下的 `admin-accounts-and-console`。

| 内容 | 位置 | 说明 |
|---|---|---|
| 管理员域 | `services/accounts/crates/core/src/admin.rs` | 独立账号、登录与会话、用户管理用例；引导账号幂等且不覆盖密码；列表参数在这里统一规整 |
| 用户停用/启用 | `services/accounts/crates/core/src/auth.rs`、迁移 `20261016120000_add_admin_users.sql` | `users.status`（CHECK 约束）；停用立即生效，登录得到 `403 account_disabled` |
| 会话端口改为中立的 `i64` 主体 | `services/accounts/crates/core/src/auth.rs`、`services/accounts/crates/infrastructure/src/redis_session.rs` | 用户与管理员复用同一个端口与适配器，靠 Redis 键命名空间隔离 |
| 管理后台接口 | `services/accounts/crates/api/src/admin.rs`、`services/accounts/crates/api/src/http.rs` | `/admin/auth/*`、`/admin/users*`；错误映射、客户端 IP、Cookie 辅助从 `auth.rs` 抽到 `http.rs` 共用 |
| 管理员仓储、用户仓储扩展 | `services/accounts/crates/infrastructure/src/persistence/` | 列表用固定 SQL + 参数绑定，关键字转义 `LIKE` 通配符 |
| 配置 | `services/accounts/crates/infrastructure/src/config.rs`、`config.toml` `[admin]` | 引导账号（密码在 `Debug` 里隐藏）、会话时长、Cookie 名、更严的默认限流 |
| 管理后台前端 | `frontend-admin/` | 从 Python 模板复制并**大幅精简**：只保留登录与用户管理（搜索、状态筛选、分页、启用/停用，中英双语）；删掉演示页面（仪表盘假数据、任务、项目、应用、聊天、帮助、设置表单、运行轨迹、What's New）；错误按后端的 `code` 翻译；修掉模板里 `instanceof AxiosError` 永不命中（请求被包装成 `ApiRequestError`）导致会话过期不跳转的问题；独立 pnpm 项目，Dockerfile 用 `BACKEND_UPSTREAM` 在运行时指定后端 |
| 测试 | `services/accounts/crates/core/src/admin/tests.rs`、`services/accounts/crates/api/tests/admin_routes.rs`、`services/accounts/tests/http_end_to_end.rs`、`frontend-admin/e2e/admin.e2e.mjs` | 隔离、限流独立、停用立即生效、列表边界、`RequireJson`；真实 PostgreSQL/Redis；真实 Chrome 33 项检查；负控：让管理员复用普通域的键前缀，真实端到端用例变红 |
| just 命令 | `justfile` | `web-admin-install`、`web-admin-dev`、`web-admin-check`、`web-admin-test`、`web-admin-docker-build`；`web-e2e` 同时跑两个前端 |

### 阶段 6：部署、配置与备份

| 内容 | 位置 | 说明 |
|---|---|---|
| 整套服务的 compose | `docker-compose.yml`、`docker-compose.accounts.yml`、`just stack-*`、`services/accounts/src/bin/compose_config.rs` | PostgreSQL + Redis + 账号服务 + 前台 + 管理后台；部署配置从独立 Nacos Properties Data ID 读取并只注入 Compose 子进程，不生成本地部署环境文件；accounts 用 `/ready` 做健康检查 |
| Nacos Compose 配置入口 | `services/accounts/crates/infrastructure/src/config/{nacos,compose}.rs`、`scripts/compose-config.sh` | 复用应用 Nacos HTTP 读取器，强制独立 Compose Data ID；严格解析白名单字面量属性并在 Docker 启动前校验；Nacos 错误时不回退本地配置 |
| 前台入口代理 `edge` | `deploy/edge.nginx.conf` | **验证中发现 Next.js 的 rewrite 代理不会追加 `X-Forwarded-For`**，直接暴露它再开 `trust_forwarded_for` 会让伪造 IP 绕过限流（复现：21 次失败全部通过）。因此前台经 nginx 访问，nginx 覆盖写该头并把 `/api/` 直接转给账号服务；`web` 与 `accounts` 不发布端口。管理后台的 nginx 同样补了 `TRUSTED_PROXY_CIDR`（前面还有 HTTPS 代理时还原真实客户端 IP）。已更正 system-design、前台 README、config.toml 里的错误说法 |
| 修掉的 Dockerfile 问题 | `Dockerfile` | 加入 `tools/xtask` 之后后端镜像一直构建失败（工作区成员 `tools/*` 没拷进构建上下文），CI 的 docker 任务本会红；compose 首次构建时才暴露 |
| 备份与恢复 | `scripts/db-backup.sh`、`scripts/db-restore.sh`、`scripts/lib/` | `pg_dump` 自定义格式；先写临时文件、校验后才落盘；恢复前先验证文件可读（坏文件不动现有数据）；备份与恢复均先从 Nacos 装配 Compose 环境 |
| 冒烟测试 | `scripts/stack-smoke.sh`、`scripts/lib/stack-smoke.sh`、`just stack-smoke` | 对着运行中的整套服务：入口、登录与停用、伪造 IP 限流、备份→删数据→恢复；外部入口加载 Nacos 配置后再运行 smoke |
| 发布流水线 | `.github/workflows/cd.yml` | 打 `v*` tag：先确认该提交的 CI 成功，再构建三个镜像推到 GHCR 并创建 GitHub Release；**发布方式（GHCR 镜像）是我定的，没有照搬 Python 版的 zip 包**；尚未在真实 runner 上运行 |
| 文档 | `docs/guides/deployment.md`、`configuration.md`、`backup.md` | 部署与上生产清单、HTTPS 代理接入、全部配置项、备份调度与演练 |

### 模板化与 SQLx 编译期校验（2026-10-21）

| 内容 | 位置 | 说明 |
|---|---|---|
| 安全复制 | `just copy`、`tools/xtask/src/template.rs` | 排除 `.git`、构建产物、密钥与当前仓库 tasks；`--force` 只覆盖同名文件，不删除其他文件，也不自动初始化 Git 或提交 |
| 上游同步 | `just sync-template`、`.template-sync` | 限定通用 AI 规范、工作流、skills 和 `xtask`；默认交互逐文件确认，`--list` 只读，`--apply` 批量覆盖；上游删除项保留 |
| SQL 编译期检查 | `.sqlx/`、仓储 SQL 宏、`just sqlx-prepare` | CI 与 Docker 使用离线 schema；SQL 或迁移改动后由本地真实 PostgreSQL 刷新缓存 |

### 账号服务独立部署与前端模块复用（2026-10-29）

| 内容 | 位置 | 说明 |
|---|---|---|
| 账号业务服务 | `services/accounts/`、`services/accounts/migrations/` | auth、admin、users 仍由同一个账号服务和数据所有者管理；保留对外 `/auth/*`、`/admin/*` 契约，其他服务不依赖账号内部 crate，也不直连账号数据库或 Redis |
| 服务独立运行 | `Dockerfile`、`docker-compose.accounts.yml`、`just accounts-up` | 独立模式 API 只绑定本机回环地址且关闭转发头信任；完整站点由 nginx 同源代理访问。独立模式与完整站点默认使用相同 Compose 项目，不能同时切换运行；隔离并存需使用不同项目名、环境文件和 API 端口 |
| 可复用账号 UI | `packages/accounts-ui/`、`examples/accounts-host/` | 普通用户注册/登录组件、可配置 API base 和 HTTP 客户端打包为 npm tarball；第二宿主实际安装该包并用浏览器验证注册、会话、退出 |
| 接入边界 | `docs/guides/accounts-integration.md` | 可以为不同应用独立部署各自账号实例，或明确选择共享账号域；当前没有 tenant/app_id 隔离，不能把共享实例描述为跨应用数据隔离 |

### 阶段 4：PRD 工具链（用 Rust 重写）

| 内容 | 位置 | 说明 |
|---|---|---|
| PRD skill | `skills/prd/`（SKILL.md、模板、references） | 从 Python 模板复制，改写了对 Python 脚本的引用 |
| 契约解析（唯一实现） | `tools/xtask/src/prd/contract.rs` | 验收清单、`Human-Confirmed` 分组、三态横幅、Change Log；`CONTRACT_VERSION` 与 SKILL.md 的标记由测试钉死 |
| 门禁 | `tools/xtask/src/prd/check/` | 必需章节、Part A 元数据泄漏、解读回显、FR 编号、oracle 模式（分级）、交付依赖、最终对账、横幅一致性；三档严格度 |
| 执行锁 | `tools/xtask/src/prd/lock.rs` | 文件锁串行化；心跳加 worktree 活性判过期；接管留档；归属移交 |
| 状态看板 | `tools/xtask/src/prd/status.rs` | 进度、横幅、依赖、锁、证据；`PENDING` / `AWAITING HUMAN` / `ARCHIVE` 分区 |
| `prd review`、提交钩子 | `review.rs`、`hooks.rs`、`commands.rs` | 审查入口；PRD 归档门禁、锁冲突警告、行数上限、守卫测试修改保护 |
| 测试 | 单元 78 个 + 端到端 10 个 | 端到端用真实 git worktree 和真实进程，含「6 个进程同时领锁恰好 1 个成功」；并发测试连跑数百次零失败 |
| 工作流指南 | `docs/guides/prd-workflow.md` | 生命周期、命令、门禁、锁、与 Python 版的差异 |

**没有移植的**：看板的 FILES 列与分支副本优先读取；`just ai implement`（AI 执行器与独立 verifier 的编排）和证据密钥扫描；人审清单的交互 HTML 生成与结构质检。原因见 `docs/guides/prd-workflow.md`。

**重写过程中发现并修掉的 Python 版缺陷**：① 锁的接管算法存在「两个会话同时持锁」的窗口（并发测试实测复现过，已改为文件锁串行化）；② 模板里的豁免提示句会让真实的 oracle 块完全不被校验；③ 同一份契约解析在 Python 里被抄了三份，靠守卫测试防漂移。

## 刻意推迟

| 内容 | 原因 | 建议时机 |
|---|---|---|
| 验证码 / IP 信誉等更重的防护 | 现在的限流允许别人故意输错来暂时锁住某个账号（窗口后自动恢复）；若不可接受才需要 | 出现真实滥用时 |
| 邮箱验证、找回密码 | 需要邮件发送基础设施 | 有真实用户注册需求时 |
| 管理员的增删、改密码、角色与细粒度权限、操作审计、二次验证 | 阶段 5 只做了「环境变量引导的单一管理员体系」；密码没有轮换机制 | 有真实运营需求时 |
| 「登出所有设备」、会话列表 | 需要按用户索引会话（Redis 里目前只有 令牌哈希→用户 ID 的映射） | 有需求时 |
| 请求 ID / 关联日志中间件 | Python 版有 request context；目前只有 `TraceLayer` | 排障遇到困难时 |
| 管理后台的运行轨迹页面及其后端 | 依赖业务有了才有意义 | 有真实流量需求时 |
| `/metrics`（Prometheus）、运行轨迹、OTLP 导出 | 依赖业务有了才有意义 | 有真实流量需求时 |
| 沙箱执行后端（filesystem / Docker / E2B） | 面向特定的 Agent 场景，不是通用底座 | 有具体需求时再做 |
| `just ai implement`（AI 执行器 + 独立 verifier 的编排）、证据密钥扫描、人审清单交互 HTML | PRD 工具链里**没有**移植的部分，见阶段 4 | 有具体需求时 |
| Python 版 `justfile.shared` 里的 `worktree`、`e2e`、`release`、`ai *` | 体量大，且很多绑定 uv/pytest/Alembic；Rust 已有轻量的 copy/sync 入口，但未照搬这些配方 | 按需逐个搬 |
| Python 版发布 zip 包（模板自身的分发方式） | 这里的 `cd.yml` 发布的是镜像；模板自己的 zip 分发另议 | 决定模板分发方式后 |
| 可观测性栈（Prometheus / Grafana / Loki / Vector 的 compose 与配置） | 后端还没有 `/metrics`，配了也没数据可看 | 有 `/metrics` 之后 |
| MkDocs 站点 | 改用 `cargo doc` + Markdown；是否需要站点另议 | 按需 |
| 其他 skills：`decision-board`、`feature-docs`、`git-diff-report`、`interactive-ui-prototype`、`manage-roadmap`、`rebase-merge-branches`、`api-help-docs` | 尚未逐个评估是否与语言无关、是否依赖 Python 脚本 | 按需评估 |
| `zata-writer`、`zatatree-blog` | 个人写作类 skill，大概率不属于工程模板 | 不迁移，除非另有决定 |

## 已知偏差与需要你知道的

- **依赖方向与 Python 版不同**：见 `docs/architecture/system-design.md`「与 Python 版模板的差异」。
- **crate 名前缀 `tpl-` 是占位符**，项目定名后需要重命名（见下）。
- **SQLx 使用编译期检查**：生产仓储使用 `query!` / `query_as!` / `query_scalar!`，`.sqlx/` 提供离线元数据；更新流程见 `docs/ai-standards/migrations.md`。
- **配置了数据库就要求启动时可达**（要执行迁移），不可达则启动失败；运行中的短暂中断才是惰性重连。
- **默认端口改为 8080**（Python 版用 8000），避免本机同时跑两个项目时冲突。
- **compose 端口 5433 / 6390**：避开本机常见的 5432 / 6379（本机已有别的项目占用）。

## 建议的下一步

1. ✅ 已推送到公开 GitHub 仓库，首次 CI 的 7 个 job 全部通过（[run 39584333354](https://github.com/ZataZhang/zata-code-template-rust/actions/runs/39584333354)）。
2. 管理后台目前只有用户管理；有新的运营需求时，再用 PRD 工作流往里加页面（别把演示页面加回来）。
3. 上生产前的清单：`APP_AUTH__COOKIE_SECURE=true`；前台站点必须经 nginx 等会覆盖写 `X-Forwarded-For` 的代理访问（Next.js 自己不会写，见 `docs/guides/deployment.md`）；`just stack-up` 的 compose 已按此配置。

## 给项目重命名

文件夹名可以随时改，对 Cargo 与 Docker Compose 没有影响（compose 项目名固定为 `tpl-testing`）。要把占位的 `tpl` 前缀换成真实项目名（以 `acme` 为例）：

```bash
# 1. 一条命令替换所有文本文件里的 tpl- / tpl_（跳过 target、node_modules、.next、.git 和两个锁文件，二进制文件自动忽略）
grep -rIl -E 'tpl[-_]' --exclude-dir=target --exclude-dir=.git --exclude-dir=node_modules --exclude-dir=.next \
  --exclude=Cargo.lock --exclude=pnpm-lock.yaml . \
  | xargs sed -i '' -E 's/tpl([-_])/acme\1/g'
# 2. 改名会改变 use 语句的字母顺序，需要重新格式化，否则 just lint 会失败
just fmt
# 3. 验证（Cargo.lock 会在编译时自动更新）
just ci
```

改完用 `git diff` 看一遍，确认没有误伤（比如 `tpl-testing` 这个 compose 项目名也会被换掉，这是预期的）。
