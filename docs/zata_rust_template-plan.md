# zata_rust_template 迁移计划

> 记录于 2026-10-15。来源：对话中给出的建议，原文保留，供对照另一个会话的实际进展。

> 执行状态更新于 2026-10-21：原文建议的阶段 4 已按最小范围实现，Rust 模板上游为 `https://github.com/ZataZhang/zata-code-template-rust.git`；SQLx 查询也已启用编译期检查并维护离线缓存。下文原始计划保留，实际命令见 `docs/guides/template-workflow.md`。

建议：新建一个独立仓库，先做一条最小的竖切（vertical slice），再逐步搬模板里的东西。不要一上来复刻整个模板。

## 第 0 步：先定三件事

| 决定 | 我的推荐 | 理由 |
|------|----------|------|
| 仓库 | 新建 `~/code/zata_rust_template`，不改 Python 模板 | Python 模板还在用，且你想先验证哪些部分真能通用 |
| Web 框架 | axum + tokio | 目前最主流，社区资料多，和 tower 生态配合好 |
| 数据库 | sqlx（PostgreSQL） | 异步，SQL 在编译期检查，不用学 ORM 的 DSL，迁移用 `sqlx migrate` |

前端先不做：frontends/admin 用 HTTP 对接 Rust 后端即可，等后端跑通后再复制过来。

## 第 1 步：后端骨架（保留四层思想）

四层依赖方向不变，用 Cargo workspace 把边界变成编译器强制的，这比 Python 里靠脚本检查更强：

```
zata_rust_template/
├── Cargo.toml                 # workspace
├── crates/
│   ├── api/                   # axum 路由、参数校验   → 依赖 core
│   ├── core/                  # 用例、领域规则、trait → 依赖 engines 的 trait
│   ├── engines/               # 可插拔能力            → 依赖 infrastructure
│   ├── infrastructure/        # sqlx、配置、日志、redis
│   └── app/                   # composition root，main.rs，装配各层
```

如果某个 crate 的 Cargo.toml 没写依赖，它就根本 `use` 不了，所以 Python 版的 `check_architecture.py` 在这里大部分可以省掉。

## 第 2 步：只做一条竖切

目标是跑通就行，不要铺开：

1. `/health`、`/ready`、`/live` 探针
2. 配置加载（config.toml 加环境变量，用 figment 或 config crate）
3. 日志（tracing）
4. 用户注册和登录，HTTP-only Cookie 会话，会话存 Redis
5. 一个 sqlx 迁移和一条真实查询
6. 一个集成测试，通过真实 HTTP 入口打到应用

这样会同时学到 Rust 里最常用的东西：async、trait 对象、错误类型（thiserror + anyhow）、共享状态（`Arc<AppState>`）。

## 第 3 步：搬通用的内核

等竖切能跑，再按下面顺序从 Python 模板搬：

1. AGENTS.md / CLAUDE.md 和 docs/ai-standards/：把 Python 规则改成 Rust 版（cargo fmt、clippy、错误处理、`///` 文档、unsafe 规则）
2. justfile 和 justfile.shared：统一入口 `just run / test / lint / sync`，内部换成 cargo 命令
3. Skills：统一使用 `~/code/zata_code_template/skills/` 作为唯一维护源；派生仓库不保存本地副本
4. pre-commit：换成 `cargo fmt --check`、`cargo clippy -- -D warnings`
5. Docker：多阶段构建，最终镜像只放一个二进制
6. 通用的守卫测试和 hooks（文件行数限制、PRD 检查等）

## 第 4 步：模板化（2026-10-21 已实现）

虽然目前还没有第二个真实派生项目，本次按用户要求先做了窄范围的 Rust 工作流同步：

1. `just copy <目录>` 复制干净工作树，排除 Git/构建产物/密钥/本仓库 tasks；目标非空时要求 `--force`，只覆盖同名文件，不初始化 Git、不创建提交。
2. `just sync-template` 读取目标项目 `.template-sync`，默认逐文件展示 diff 并确认；`--list` 只读列差异，`--apply` 才批量覆盖。上游删除的文件保留在本地，业务代码和部署配置不在默认同步清单里。
3. 当前通用清单先覆盖 Agent 入口、AI 规范、PRD/想法工作流、skills 与 Rust `xtask`。至少在第二个派生项目使用后，再调整同步边界；不把 Python 版 TUI、沙箱和执行器一并移植。

## 风险提醒

- 还在学 Rust，同时做 async + sqlx + 工作区，坡度很陡。如果卡住，建议先把 rust-study 补一课 async/tokio，再开始第 2 步。
- 编译时间：SQLx 宏在本地联网数据库生成 `.sqlx/` 元数据；普通编译、CI 和 Docker 用离线缓存，无需连数据库。
- Python 模板里的沙箱（E2B、Docker 适配器）不建议第一轮就搬。
