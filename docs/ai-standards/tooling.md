# Tooling Standards

## Environment Example Files

`.env.example` 用「是否启用」来表达默认配置要求，而不是按变量是否敏感决定是否注释：

- 默认流程必填的变量保持未注释；必填密钥只保留空值，不写真实凭据或可用默认值
- 可选功能、外部集成或发布流程才需要的变量保持注释，并在相邻注释里说明启用条件
- 变量是否属于密钥，只决定示例里能否出现真实值（不能）
- 默认 env 未填写时，配置加载仍须正常完成（所有字段都有默认值）

新增给密钥类的配置变量（密钥别名、端点注册信息等）放到 `config.toml`，实际密钥值仍由环境变量注入。

## Preferred Tools

- 构建与依赖：`cargo`（锁文件 `Cargo.lock` 必须提交；CI 与 Docker 用 `--locked`）
- 任务入口：`just`
- 格式化：`cargo fmt`；静态检查：`cargo clippy`
- 文档：`cargo doc`

用 `just` 代替手工记忆零散命令。

## Common Commands

| Command | Purpose |
|---|---|
| `just` | 列出所有任务 |
| `just sync` | 预先下载依赖（`cargo fetch --locked`） |
| `just run` / `just down` | 启停本地 API、公开站点、管理后台和账号 UI 第二宿主；`run` 自动安装前端依赖、打印 URL 并读取 `config.toml` 与 `.env.local` |
| `just run backend` | 只启动账号 API；URL 由 `APP_SERVER__PORT` 决定（默认 8080） |
| `just run accounts-host` | 单独启动第二宿主（需要已有 API），默认 URL `http://127.0.0.1:4174` |
| `just run docker` / `just down docker` | Docker 整栈启动 / 停止，等价于 `just stack-up` / `just stack-down` |
| `just check` | 只检查能否编译（写代码时最常用） |
| `just sqlx-prepare` | 用真实本地 PostgreSQL 更新 `.sqlx/` 编译期查询元数据（先 `just db-up`，需安装 `sqlx-cli`） |
| `just copy <目录>` | 从 Rust 模板复制干净工作树；已有目录用 `--force` 时只覆盖同名文件，不会初始化 Git |
| `just sync-template [--list|--apply]` | 按 `.template-sync` 清单查看或同步上游通用文件；默认交互确认，详见 [模板工作流](../guides/template-workflow.md) |
| `just fmt` | 自动格式化 |
| `just lint` | `cargo fmt --check` + `cargo clippy -- -D warnings` |
| `just test` | 本地测试集（不含需要真实外部服务的测试） |
| `just test real` | 只运行需要真实 PostgreSQL / Redis 的测试（先 `just db-up`；连接串默认指向本地 compose，可用 `TEST_DATABASE_URL`、`TEST_REDIS_URL` 覆盖） |
| `just test all` | 全部测试 |
| `just doc` | 构建文档，警告视为错误 |
| `just ci` | 提交前的完整检查：lint + test + doc |
| `just db-up` / `db-down` / `db-reset` | 本地 PostgreSQL 与 Redis（5433 / 6390）；`db-reset` 会清空数据 |
| `just build` | 发布构建 |
| `just web-install` | 安装前端依赖（`pnpm install --frozen-lockfile`） |
| `just stack-up` / `stack-down` / `stack-reset` / `stack-logs` | 整套服务（`docker-compose.yml`）；`stack-reset` 会清空数据 |
| `just db-backup` / `db-restore <文件>` | PostgreSQL 备份与恢复，见 `docs/guides/backup.md` |
| `just web-admin-install` / `web-admin-dev` / `web-admin-check` / `web-admin-test` | 管理后台：安装依赖 / 开发服务器 / 类型+lint+knip+格式+构建 / 浏览器组件测试 |
| `just web-dev` | 单独启动公开前端开发服务器（需另有 API）；通常直接用 `just run` 启动整套本地开发服务 |
| `just web-check` | 前端：类型检查 + lint + 生产构建 |
| `just web-e2e` | 前端：真实浏览器端到端测试（需要本机有 Chrome 与 Docker） |
| `just web-docker-build [后端地址]` | 构建前端镜像；后端地址是**构建参数**（Next.js 的 rewrites 在构建时固化） |
| `just ci-all` | 后端 + 前端的完整检查：`ci` + `web-check` + `web-admin-check` |
| `just prd <子命令>` | PRD 工具链（Rust，`tools/xtask`）：`status` / `start` / `heartbeat` / `release` / `check` / `review`，详见 `docs/guides/prd-workflow.md` |
| `just check-lines` | 文件行数硬上限检查（1000 个非空行；CI 里用） |
| `just docker-build` | 构建账号服务镜像 `tpl-accounts:local` |

`just test` 是常规验证命令，任务要求运行时无需单独征求用户确认。

## 环境提示

- Homebrew 安装的 rustup 是 keg-only：若 `cargo` 提示 command not found，执行  
  `echo 'export PATH="/opt/homebrew/opt/rustup/bin:$PATH"' >> ~/.zshrc && source ~/.zshrc`
- 在管道里取 `cargo`/`just` 的退出码要小心：`cmd | tail` 的状态是 `tail` 的。验证门禁时直接运行命令并看 `$?`，或用 `set -o pipefail`

## Lints

- 工作区 lint 配置在根 `Cargo.toml` 的 `[workspace.lints]`，各 crate 用 `[lints] workspace = true` 继承：`unsafe_code = "forbid"`、`missing_docs = "warn"`、`clippy::all`、`dbg_macro`、`todo`
- `clippy.toml` 允许测试里使用 `unwrap` / `expect`
- 抑制 lint 必须写 `reason`（见 [Comments And Docs](comments-docs.md)）
- 不要整体关闭某类 lint 来「让它通过」；先修代码

## Dependencies

- 版本统一声明在根 `Cargo.toml` 的 `[workspace.dependencies]`，各 crate 用 `xxx.workspace = true` 引用
- 新增依赖前先问：标准库或现有依赖能否解决？这个依赖的维护状态、体积和传递依赖如何？
- 控制 feature：`default-features = false` 并只开需要的（如 `sqlx`），减少编译时间和攻击面
- 层间依赖方向见 [Architecture](architecture.md)

## Docker

- `Dockerfile` 为多阶段构建：构建阶段编译，运行阶段只放二进制与 `config.toml`，使用非 root 用户
- `.dockerignore` 必须排除 `target/`、`.git`、`.env*`
- `docker-compose.testing.yml` 仅用于本机，密码明文写入，不要用于生产

## Git 钩子与 CI

**钩子**（pre-commit 框架，配置在 `.pre-commit-config.yaml`）：`just hooks` 安装一次。提交前只放「快、本机能确定结果」的检查，完整测试在 CI 里跑——钩子太慢会让人习惯性 `--no-verify`，反而什么都拦不住。

| 钩子 | 何时触发 | 拦什么 |
|---|---|---|
| 文件卫生（行尾空格、文件末尾换行、yaml/toml/json 语法、合并冲突标记、大文件） | 每次 | 低级错误；`migrations/` 与锁文件被排除（改写迁移会让 sqlx 的校验和失败） |
| `cargo fmt --check` | 改了 `.rs` | 格式 |
| `cargo clippy -- -D warnings` | 改了 `.rs`、`Cargo.toml`、`Cargo.lock` | 警告 |
| 层间依赖方向守卫测试 | 改了任何 `Cargo.toml` | 违规依赖（clippy 拦不住这个） |
| 前端类型检查 + lint | 改了 `frontend-public/` | 前端错误 |
| i18n key 对齐 | 改了 `messages/*.json` | en/zh 缺 key |
| PRD 归档门禁 | 改了 `tasks/` 根目录或 `tasks/archive/` 下的 PRD | **新归档**的 PRD 必须过 `--archive-ready`（pending 里的不检查；改动已归档 PRD 的验收记录也不重新裁决） |
| PRD 锁冲突 | 改了 `tasks/pending/`、`tasks/evidence/` | 只警告：该 PRD 正被其他会话执行 |
| 文件行数 | 改了 `.rs` / `.ts` / `.tsx` | 只警告超过 1000 个非空行（CI 里是硬上限） |
| 守卫测试修改保护 | 每次 | 改动**文件头标注了「守卫测试（guard test）」**的测试文件，必须 `GUARD_UPDATE_ACK=1`；AI 代理默认不应设置它，需要先问人 |
| 提交信息格式 | `commit-msg` 阶段 | 见上 |

`just hooks-run` 对全部文件跑一遍。**不要用 `--no-verify` 绕过**；钩子失败就修代码。

**CI**（`.github/workflows/ci.yml`，每个 PR 和 `main` 的 push）：

| 任务 | 内容 |
|---|---|
| `hooks` | 通用文件卫生（Rust 与前端的检查由下面的任务负责） |
| `rust` | `just lint`、`just doc`、`just check-lines`、`cargo xtask prd check --archived`、启动真实 PostgreSQL/Redis 后 `just test all` |
| `web` | i18n 对齐、类型检查、lint、生产构建 |
| `web-e2e` | `just web-e2e`：真实 Chrome + 真实后端和数据库（`rust`、`web` 通过后才跑） |
| `docker` | 后端、前端两个镜像能构建出来（不推送） |

注意：CI 配置已用 `actionlint` 检查语法，其中每个命令都在本机跑通过，但**尚未在真实的 GitHub runner 上运行过**；首次运行时留意 Ubuntu 环境差异（`web-e2e` 依赖 runner 上自带的 Chrome、`lsof`、`python3`）。

## Git 约定

- 除非用户明确要求，不要自动执行 `git add`、`git commit`、`git push` 等 Git 变更操作
- 不要用 `git stash` 做门禁对照（见 [Testing](testing.md)）

## Commit Messages

提交信息统一使用**英文**，并遵循 Conventional Commits：

- 格式：`type(scope): subject`，常见 `type` 为 `feat` / `fix` / `refactor` / `docs` / `test` / `chore`；`scope` 可省略
- 标题行不超过 72 字符；正文说明 **why** 而非 **what**
- 可以用正文列出关键改动点，但不要在标题里堆砌多条变更
- 一次提交一个意图：不要顺手把无关改动混进同一提交

由 `commit-msg` 钩子强制校验（`scripts/check-commit-msg.sh`）：标题必须符合 `type(scope): subject`、不超过 72 个字符、不含非 ASCII 字符；合并提交、`revert`、`fixup!`/`squash!` 不检查。正文不检查，但应当用英文并解释 why。
