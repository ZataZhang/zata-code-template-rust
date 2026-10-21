# Rust 工程模板

面向 Rust Web 服务的可演进工程模板：**axum + tokio + sqlx（PostgreSQL）**，按「接入层 / 核心编排层 / 平台能力层 / 基础设施层 + 装配层」拆成 Cargo workspace，层间依赖方向有两道防线：未声明的依赖编译器直接拒绝，已声明的依赖由守卫测试（`crates/app/tests/architecture.rs`）检查。

这是 [`zata_code_template`](../zata_code_template)（Python / FastAPI）的 Rust 版，正在分阶段迁移，进度见 [`docs/migration-status.md`](docs/migration-status.md)。

## 现在有什么

- **健康探针**：`/live`（存活）、`/health`（版本）、`/ready`（PostgreSQL、Redis 是否就绪，不可用时返回 503，恢复后自动变回 200）
- **用户认证**：注册、登录、退出、`/auth/me`；argon2id 密码哈希，HTTP-only Cookie 会话存 Redis（只存令牌的哈希），防用户枚举，写接口只收 JSON
- **管理员账号与管理后台接口**：管理员是独立的账号体系（独立的表、Cookie、会话命名空间、限流计数，更严的默认上限），只能由启动时的环境变量创建；`/admin/auth/*` 登录退出，`/admin/users*` 分页/筛选/搜索用户并启用/停用（停用立即生效）
- **登录/注册限流**：按 IP 和按账号计数（存 Redis），超限返回 429 + `Retry-After`；不泄露账号是否存在，默认不信任可伪造的 `X-Forwarded-For`
- **数据库迁移**：`sqlx migrate`，启动时自动执行，表和列带注释
- **配置**：`config.toml` 默认值 < 环境变量；本地密钥放 `.env.local`；密码不会出现在日志里
- **日志**：`tracing`，文本或 JSON
- **优雅退出**：收到 Ctrl+C / SIGTERM 后处理完在途请求再退出
- **工程工作流**：`just` 统一入口、clippy 零警告、`missing_docs` 强制公共 API 写文档、AI 协作规范（`AGENTS.md`、`docs/ai-standards/`）
- **部署**：一个二进制；多阶段 `Dockerfile`，非 root 运行；`just stack-up` 一条命令拉起整套服务（后端 + 前台 + 管理后台 + PostgreSQL + Redis，前台前面有会覆盖写 `X-Forwarded-For` 的 nginx）；自带数据库备份/恢复脚本；打 `v*` tag 自动构建并推送镜像到 GHCR

- **前台站点**（`frontend-public/`，Next.js）：营销页、注册、登录、受保护区域，中英双语；有真实浏览器端到端测试
- **管理后台**（`frontend-admin/`，Vite + React）：管理员登录、用户列表（搜索、状态筛选、分页）、启用/停用用户，中英双语；有真实浏览器端到端测试

不包含：邮箱验证、找回密码、管理员的增删与改密码、运行轨迹等，见[迁移状态](docs/migration-status.md)。

## 架构

```text
crates/
├── api/             tpl-api             HTTP 路由、DTO            → 依赖 core
├── core/            tpl-core            用例、领域规则、端口(trait) → 不依赖任何内部 crate
├── engines/         tpl-engines         可插拔的平台能力（空）     → 依赖 core
├── infrastructure/  tpl-infrastructure  配置、日志、数据库         → 依赖 core
└── app/             tpl-app             composition root（bin）    → 依赖全部
```

`tpl-` 是占位前缀，项目定名后按 [`docs/migration-status.md`](docs/migration-status.md) 里的步骤重命名。详细说明见 [`docs/architecture/system-design.md`](docs/architecture/system-design.md)。

## 快速开始

### 环境

- Rust（`cargo --version` 能看到版本即可）。Homebrew 安装的 rustup 是 keg-only，若提示 `command not found`：  
  `echo 'export PATH="/opt/homebrew/opt/rustup/bin:$PATH"' >> ~/.zshrc && source ~/.zshrc`
- [just](https://github.com/casey/just)
- Docker（本地 PostgreSQL / Redis 与构建镜像时需要）

### 运行

```bash
just            # 列出所有任务
just run        # 启动服务，默认 http://127.0.0.1:8080
curl localhost:8080/live
```

要连数据库和 Redis，并启用认证：

```bash
cp .env.example .env.local     # 然后取消 DATABASE_URL 和 REDIS_URL 两行的注释
just db-up                     # 启动本地 PostgreSQL（5433）与 Redis（6390）
just run                       # 启动时自动执行 migrations/ 下的迁移

curl localhost:8080/ready      # {"status":"ready","checks":{"postgres":"ok","redis":"ok"}}

# 注册、登录（-c 保存 Cookie）、查看当前用户（-b 带上 Cookie）、退出
curl -X POST localhost:8080/auth/register -H 'Content-Type: application/json' \
     -d '{"email":"me@example.com","password":"correct-horse"}'
curl -c jar.txt -X POST localhost:8080/auth/login -H 'Content-Type: application/json' \
     -d '{"email":"me@example.com","password":"correct-horse"}'
curl -b jar.txt localhost:8080/auth/me
curl -b jar.txt -X POST localhost:8080/auth/logout -H 'Content-Type: application/json' -d '{}'
```

没有同时配置 `DATABASE_URL` 和 `REDIS_URL` 时，`/auth/*` 与 `/admin/*` 返回 503，其余功能照常。

管理后台：管理员没有注册入口，启动前设置引导账号（已存在则不改动，绝不覆盖密码）：

```bash
export ADMIN_BOOTSTRAP_USERNAME=admin ADMIN_BOOTSTRAP_PASSWORD='至少8位的初始密码'
just run

curl -c admin.txt -X POST localhost:8080/admin/auth/login -H 'Content-Type: application/json' \
     -d '{"identifier":"admin","password":"至少8位的初始密码"}'
curl -b admin.txt 'localhost:8080/admin/users?keyword=example&status=active'
curl -b admin.txt -X POST localhost:8080/admin/users/1/disable -H 'Content-Type: application/json' -d '{}'
```

没配引导账号时，管理员登录永远返回 401（`invalid_credentials`）。

要看前端界面：

```bash
just web-install      # 首次：安装前端依赖
just web-dev          # 另开终端（此时后端已在 8080 运行），打开 http://localhost:3000

just web-admin-install   # 管理后台：首次安装依赖
just web-admin-dev       # 打开 http://localhost:5173（后端需配置引导管理员）
```

## 常用命令

| 命令 | 用途 |
|---|---|
| `just sync` | 预先下载依赖 |
| `just run` | 启动服务 |
| `just check` | 只检查能否编译（最快） |
| `just lint` | `cargo fmt --check` + `clippy -D warnings` |
| `just test` | 本地测试集 |
| `just test real` | 需要真实 PostgreSQL / Redis 的测试，含认证端到端流程（先 `just db-up`） |
| `just doc` | 构建文档，警告视为错误 |
| `just ci` | 提交前的完整检查：lint + test + doc |
| `just db-up` / `db-down` / `db-reset` | 本地中间件；`db-reset` 会清空数据 |
| `just docker-build` | 构建后端镜像 |
| `just web-check` | 前端：类型 + lint + 生产构建 |
| `just web-e2e` | 前端：真实浏览器端到端测试，公开站点与管理后台各一套（需要 Chrome） |
| `just web-admin-check` / `web-admin-test` | 管理后台：类型 + lint + 未使用代码 + 格式 + 构建 / 浏览器组件测试 |
| `just stack-up` / `stack-down` / `stack-reset` / `stack-logs` | 整套服务（后端 + 前台 + 管理后台 + PostgreSQL + Redis）一条命令跑起来，首次生成随机密钥 |
| `just db-backup` / `db-restore <文件>` | 数据库备份（校验后才落盘）与恢复（会覆盖现有数据） |
| `just ci-all` | 后端 + 前端的完整检查 |
| `just hooks` | 安装 git 钩子（提交前检查 + 提交信息检查，需要先装 pre-commit） |

## 配置

优先级：代码默认值 < `config.toml` < 环境变量。

- 覆盖单个值：`APP_<分区>__<字段>`，双下划线表示层级，如 `APP_SERVER__PORT=9000`
- 连接串：`DATABASE_URL`、`REDIS_URL`（不配置则不检查、不启用对应功能）
- 生产环境（HTTPS）必须设置 `APP_AUTH__COOKIE_SECURE=true`；服务在反向代理之后时，务必先读 `config.toml` 里 `trust_forwarded_for` 的说明
- 日志级别：`RUST_LOG`
- 管理后台：`ADMIN_BOOTSTRAP_USERNAME` / `ADMIN_BOOTSTRAP_PASSWORD`（引导管理员），`[admin]` 分区调会话时长、Cookie 名与登录限流
- 完整示例见 [`.env.example`](.env.example) 与 [`config.toml`](config.toml)

## 文档

- [系统架构](docs/architecture/system-design.md)
- [前台站点](frontend-public/README.md)（接口契约、部署注意事项）
- [AI 协作规范](docs/ai-standards/index.md)（入口：[`AGENTS.md`](AGENTS.md)）
- [数据库迁移约定](docs/ai-standards/migrations.md)
- [迁移状态](docs/migration-status.md)
- [部署](docs/guides/deployment.md)（整套 compose、上生产清单、HTTPS 代理）
- [配置](docs/guides/configuration.md)
- [备份与恢复](docs/guides/backup.md)
- [想法收集流程](docs/guides/idea-inbox.md)
- API 文档：`cargo doc --workspace --no-deps --open`
