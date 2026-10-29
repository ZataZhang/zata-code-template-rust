# Rust 工程模板

面向 Rust Web 服务的可演进工程模板：**axum + tokio + sqlx（PostgreSQL）**。当前账号能力是独立业务服务（`services/accounts/`），服务内部按「接入层 / 核心编排层 / 平台能力层 / 基础设施层 + 装配层」分层；依赖方向由 Cargo 和守卫测试（`services/accounts/tests/architecture.rs`）约束。本仓库当前交付一个业务服务，不是多个业务服务的完整平台。

这是 [`zata_code_template`](../zata_code_template)（Python / FastAPI）的 Rust 版，正在分阶段迁移，进度见 [`docs/migration-status.md`](docs/migration-status.md)。

## 现在有什么

- **健康探针**：`/live`（存活）、`/health`（版本）、`/ready`（PostgreSQL、Redis 是否就绪，不可用时返回 503，恢复后自动变回 200）
- **用户认证**：注册、登录、退出、`/auth/me`；argon2id 密码哈希，HTTP-only Cookie 会话存 Redis（只存令牌的哈希），防用户枚举，写接口只收 JSON
- **管理员账号与管理后台接口**：管理员是独立的账号体系（独立的表、Cookie、会话命名空间、限流计数，更严的默认上限），只能由启动时的环境变量创建；`/admin/auth/*` 登录退出，`/admin/users*` 分页/筛选/搜索用户并启用/停用（停用立即生效）
- **登录/注册限流**：按 IP 和按账号计数（存 Redis），超限返回 429 + `Retry-After`；不泄露账号是否存在，默认不信任可伪造的 `X-Forwarded-For`
- **数据库迁移**：`sqlx migrate`，启动时自动执行，表和列带注释
- **可复用账号服务**：独立二进制 `accounts-service`，拥有账号数据、迁移和 Redis 会话；新应用可独立部署同一实现，也可明确选择共享账号域。当前共享实例没有 tenant/app 隔离，详见[账号服务接入指南](docs/guides/accounts-integration.md)
- **配置**：`config.toml` 默认值 < 可选 Nacos TOML < 环境变量；本地 Nacos 连接凭据放 `.env.local`；密码不会出现在日志里
- **日志**：`tracing`，文本或 JSON
- **优雅退出**：收到 Ctrl+C / SIGTERM 后处理完在途请求再退出
- **工程工作流**：`just` 统一入口、clippy 零警告、`missing_docs` 强制公共 API 写文档、AI 协作规范（`AGENTS.md`、`docs/ai-standards/`）
- **部署**：账号服务多阶段 `Dockerfile`，非 root 运行；`just accounts-up` 可单独启动账号服务及依赖，`just stack-up` 可启动账号服务、前台、管理后台、PostgreSQL 和 Redis；完整站点前面有会覆盖写 `X-Forwarded-For` 的 nginx；自带数据库备份/恢复脚本；打 `v*` tag 自动构建并推送镜像到 GHCR

- **前台站点**（`frontend-public/`，Next.js）：营销页、注册、登录、受保护区域，中英双语；有真实浏览器端到端测试
- **管理后台**（`frontend-admin/`，Vite + React）：管理员登录、用户列表（搜索、状态筛选、分页）、启用/停用用户，中英双语；有真实浏览器端到端测试

不包含：邮箱验证、找回密码、管理员的增删与改密码、运行轨迹等，见[迁移状态](docs/migration-status.md)。

## 架构

```text
services/
└── accounts/        tpl-accounts        composition root；bin: accounts-service
crates/                                      仅供账号服务内部使用
├── api/             tpl-api             HTTP 路由、DTO            → 依赖 core
├── core/            tpl-core            账号用例、规则、端口       → 不依赖其他 crate
├── engines/         tpl-engines         可插拔的平台能力（空）     → 依赖 core
└── infrastructure/  tpl-infrastructure  配置、日志、账号 DB/Redis → 依赖 core
```

其他业务模块应在 `services/<module>/` 建立独立服务，并拥有自己的数据与迁移，不依赖账号私有 crate。详细架构见 [`docs/architecture/system-design.md`](docs/architecture/system-design.md)，跨应用接入见[账号服务接入指南](docs/guides/accounts-integration.md)。

## 快速开始

### 环境

- Rust（`cargo --version` 能看到版本即可）。Homebrew 安装的 rustup 是 keg-only，若提示 `command not found`：  
  `echo 'export PATH="/opt/homebrew/opt/rustup/bin:$PATH"' >> ~/.zshrc && source ~/.zshrc`
- [just](https://github.com/casey/just)
- Docker（本地 PostgreSQL / Redis 与构建镜像时需要）

### 运行

```bash
just            # 列出所有任务
just run        # 安装前端依赖并启动 API、公开站点、管理后台和账号 UI 宿主；打印全部 URL
# 浏览器访问 http://127.0.0.1:3000、http://127.0.0.1:5173 或 http://127.0.0.1:4174
# API 健康检查：http://127.0.0.1:8080/ready
just down       # 停止 just run 启动的进程
```

`8080` 是 API 端口，根路径 `/` 没有网页，会返回 404。只启动 API 可用 `just run backend`；Docker 整栈使用 `just run docker` / `just down docker`。

只运行账号服务容器及 PostgreSQL/Redis，不启动任何前端：

```bash
just accounts-up
curl http://127.0.0.1:8082/ready
just accounts-down
```

要连数据库和 Redis，并启用认证：

```bash
# 本机共享测试服务配置见 ../zata-ops/local/docker-compose.testing.yml
DATABASE_URL='postgres://testuser:testpass@localhost:5433/testdb' REDIS_URL='redis://:redis123@localhost:6390/0' just run
# 启动时自动执行 services/accounts/migrations/ 下的迁移，并启动前端

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
# 首次创建管理员时，设置好变量后再运行 just run；若服务已运行，先 just down
just run

curl -c admin.txt -X POST localhost:8080/admin/auth/login -H 'Content-Type: application/json' \
     -d '{"identifier":"admin","password":"至少8位的初始密码"}'
curl -b admin.txt 'localhost:8080/admin/users?keyword=example&status=active'
curl -b admin.txt -X POST localhost:8080/admin/users/1/disable -H 'Content-Type: application/json' -d '{}'
```

没配引导账号时，管理员登录永远返回 401（`invalid_credentials`）。

要看前端界面：

```bash
just run                 # 自动安装前端依赖并启动所有开发服务
just run frontend-public # 只启动公开站点（需另有 API 在运行）
just run frontend        # 只启动管理后台（需另有 API 在运行）
just run accounts-host   # 只启动第二宿主（需先有 API），打开 http://127.0.0.1:4174
just down
```

## 常用命令

| 命令 | 用途 |
|---|---|
| `just sync` | 预先下载依赖 |
| `just run` / `just down` | 启动 / 停止本地 API、公开站点、管理后台和第二宿主；启动时自动安装前端依赖并打印 URL |
| `just run backend` | 只启动账号 API |
| `just run docker` / `just down docker` | 启动 / 停止 Docker 整栈（等价于 `just stack-up` / `just stack-down`） |
| `just accounts-up` / `accounts-down` | 账号服务独立模式（API 仅绑定 `127.0.0.1:8082`）；默认与完整站点共用 `tpl` Compose 项目，切换前阅读部署指南 |
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
| `just stack-up` / `stack-down` / `stack-reset` / `stack-logs` | 先从 Nacos 读取部署配置，再管理整套服务（账号 + 前台 + 管理后台 + PostgreSQL + Redis） |
| `just db-backup` / `db-restore <文件>` | 数据库备份（校验后才落盘）与恢复（会覆盖现有数据） |
| `just ci-all` | 后端 + 前端的完整检查 |
| `just hooks` | 安装 git 钩子（提交前检查 + 提交信息检查，需要先装 pre-commit） |

## 配置

优先级：代码默认值 < `config.toml` < 可选 Nacos TOML < 环境变量。

- 覆盖单个值：`APP_<分区>__<字段>`，双下划线表示层级，如 `APP_SERVER__PORT=9000`
- 连接串：`DATABASE_URL`、`REDIS_URL`（不配置则不检查、不启用对应功能）
- Nacos namespace/group 与应用/Compose Data ID 位于 `config.toml` 的 `[nacos]`；`.env.local` 只保存 Nacos 地址和认证凭据。设置 `NACOS_SERVER_ADDR` 后，服务启动时读取应用 TOML
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
