# -----------------------------------------------------------------------------
# justfile —— 统一命令入口。裸运行 `just` 列出所有任务。
#
# 约定：所有项目都提供 run / test / lint 这几个入口，内部各语言自己实现。
# -----------------------------------------------------------------------------

# 本地测试库的默认连接串，与 docker-compose.testing.yml 保持一致。
local_test_database_url := "postgres://testuser:testpass@localhost:5433/testdb"
local_test_redis_url := "redis://:redis123@localhost:6390/0"

default:
    @just --list

# 预先下载依赖（Rust 没有单独的「安装」步骤，`cargo build` 会自动下载；这个命令只是让后续构建可以离线）
sync:
    cargo fetch --locked

# 启动本地开发栈：账号 API、Agent tracing、独立前端与 UI 宿主；自动安装前端依赖并打印 URL
run target="all":
    "{{justfile_directory()}}/scripts/dev-services.sh" run "{{ target }}"

# 停止 just run 管理的本地开发进程；Docker 整栈用 `just down docker`
down target="all":
    "{{justfile_directory()}}/scripts/dev-services.sh" down "{{ target }}"

# 打开本机文件与 Git 改动查看器；默认进入改动视图
view *args:
    cargo xtask view {{ args }}

# 直接打开改动视图，与 `just view --diff` 等价
diff *args:
    cargo xtask view --diff {{ args }}

# 发布构建（带优化，账号服务产物在 target/release/accounts-service）
build:
    cargo build --workspace --release

# 只检查能否通过编译，写代码时最常用
check:
    cargo check --workspace --all-targets

# 用真实本地 PostgreSQL 刷新 SQLx 编译期查询元数据（先运行 just db-up）
sqlx-prepare:
    #!/usr/bin/env bash
    set -euo pipefail
    export DATABASE_URL="${DATABASE_URL:-{{ local_test_database_url }}}"
    cargo sqlx migrate run --source services/accounts/migrations
    cargo sqlx prepare --workspace -- --all-targets

# 自动格式化
fmt:
    cargo fmt --all

# 格式检查 + clippy（警告视为错误）
lint:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings

# 运行测试：`just test` 本地测试集；`just test real` 只跑需要真实 PostgreSQL/Redis 的；`just test all` 全部
test target="":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{ target }}" in
      "")
        cargo test --workspace
        ;;
      real)
        export TEST_DATABASE_URL="${TEST_DATABASE_URL:-{{ local_test_database_url }}}"
        export TEST_REDIS_URL="${TEST_REDIS_URL:-{{ local_test_redis_url }}}"
        echo "使用测试库：${TEST_DATABASE_URL%%@*}@…（需要先 just db-up）"
        cargo test --workspace -- --ignored
        ;;
      all)
        export TEST_DATABASE_URL="${TEST_DATABASE_URL:-{{ local_test_database_url }}}"
        export TEST_REDIS_URL="${TEST_REDIS_URL:-{{ local_test_redis_url }}}"
        cargo test --workspace -- --include-ignored
        ;;
      *)
        echo "未知参数：{{ target }}（可选：real、all）" >&2
        exit 2
        ;;
    esac

# 生成并检查文档（文档里的错误按代码错误对待）
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

# 提交前的完整检查：lint + 测试 + 文档
ci: lint test doc

# PRD 工具链（Rust 实现，源码在 tools/xtask；流程见 docs/guides/prd-workflow.md）。
# 子命令：status / start / heartbeat / release / check / review，例如：
#   just prd status --detail
#   just prd check <prd> --structure-only
# 注意：PRD 的执行权归 kc（issue lifecycle）；start / heartbeat / release 只在人工直跑时使用。
# PRD 工具链：status / start / heartbeat / release / check / review（用法见上方注释）
prd *args:
    cargo xtask prd {{ args }}

# 文件行数上限检查（硬上限 1000 个非空行，CI 里用）
check-lines:
    cargo xtask hook max-lines --max-lines 1000 --glob '*.rs' services tools

# 安装 git 钩子（提交前检查 + 提交信息检查）。需要先装 pre-commit：brew install pre-commit 或 pipx install pre-commit
hooks:
    pre-commit install --install-hooks

# 对全部文件跑一遍提交前检查（等价于 CI 里的 hooks 任务）
hooks-run:
    pre-commit run --all-files --show-diff-on-failure

# 前端 en/zh 文案 key 是否对齐
check-i18n:
    node scripts/check-i18n-keys.mjs

# 构建可复用账号 UI 包，并生成供宿主安装的版本化 tarball
accounts-ui-build:
    cd packages/accounts-ui && pnpm install --frozen-lockfile && pnpm pack:tarball

# 验证独立 React 宿主通过打包 tarball 消费账号 UI 包
accounts-ui-check: accounts-ui-build
    cd examples/accounts-host && pnpm install --frozen-lockfile && pnpm typecheck && pnpm build

# 启动本地 PostgreSQL 与 Redis（端口 5433 / 6390）
db-up:
    docker compose -f docker-compose.testing.yml up -d --wait

# 停止本地中间件（保留数据）
db-down:
    docker compose -f docker-compose.testing.yml stop

# 停止并删除本地中间件的数据卷（会清空数据）
db-reset:
    docker compose -f docker-compose.testing.yml down -v

# 构建账号服务镜像
docker-build:
    docker build -t tpl-accounts:local .

# 单独启动账号服务与它依赖的 PostgreSQL、Redis；部署配置从 Nacos 读取，不启动站点或管理后台
accounts-up:
    ./scripts/compose-config.sh compose accounts up -d --build --wait accounts

# 停止账号服务（不停止或删除数据库与 Redis）
accounts-down:
    ./scripts/compose-config.sh compose accounts stop accounts

# ---------------------------------------------------------------------------
# 前台站点（frontends/public，Next.js）
# ---------------------------------------------------------------------------

# 安装前端依赖（按 pnpm-lock.yaml 精确安装）
web-install: accounts-ui-build
    cd frontends/public && pnpm install --frozen-lockfile

# 单独启动公开前端；/api/* 代理到后端。通常直接用 `just run` 启动整套本地服务
web-dev:
    cd frontends/public && pnpm dev

# 前端检查：类型 + lint + 生产构建
web-check: check-i18n accounts-ui-check
    cd frontends/public && pnpm typecheck && pnpm lint && pnpm build

# 真实浏览器端到端测试：启动数据库、后端、前端，用 Chrome 走完注册/登录/退出/限流（需要本机有 Chrome）
web-e2e: accounts-ui-check
    ./scripts/web-e2e.sh

# 构建前端镜像。后端地址必须在构建时给（Next 的 rewrites 在构建时固化）
web-docker-build backend_url="http://accounts:8080":
    docker build -f frontends/public/Dockerfile --build-arg BACKEND_URL={{ backend_url }} -t tpl-web:local .

# ---------------------------------------------------------------------------
# 管理后台（frontends/admin，Vite + React）
# ---------------------------------------------------------------------------

# 安装管理后台依赖（按 pnpm-lock.yaml 精确安装）
web-admin-install:
    cd frontends/admin && pnpm install --frozen-lockfile

# 单独启动管理后台开发服务器（默认 5173）；/api/* 代理到后端。通常直接用 `just run` 启动整套本地服务
web-admin-dev:
    cd frontends/admin && pnpm dev

# 管理后台检查：类型 + lint + 未使用代码 + 格式 + 生产构建（单元/组件测试见 web-admin-test）
web-admin-check: check-i18n
    cd frontends/admin && pnpm typecheck && pnpm lint && pnpm knip && pnpm format:check && pnpm build

# 管理后台的浏览器组件测试（vitest + Playwright Chromium；首次需要 `cd frontends/admin && pnpm test:browser:install`）
web-admin-test:
    cd frontends/admin && pnpm test

# 构建管理后台镜像。账号服务地址在运行时用环境变量 BACKEND_UPSTREAM 指定（默认 accounts:8080）
web-admin-docker-build:
    docker build -t tpl-admin:local frontends/admin

# ---------------------------------------------------------------------------
# Agent tracing 管理界面（frontends/tracing，独立 Vite + React 服务）
# ---------------------------------------------------------------------------

web-tracing-install:
    cd frontends/tracing && pnpm install --frozen-lockfile

web-tracing-dev:
    cd frontends/tracing && pnpm dev

# tracing UI 类型检查与生产构建
web-tracing-check:
    cd frontends/tracing && pnpm typecheck && pnpm build

web-tracing-docker-build:
    docker build -f frontends/tracing/Dockerfile -t tpl-tracing-ui:local frontends/tracing

# 构建 Agent tracing 后端独立镜像
tracing-docker-build:
    docker build -f services/agent-tracing/Dockerfile -t tpl-agent-tracing:local .

# ---------------------------------------------------------------------------
# 整套服务（账号 + Agent tracing、各自 PostgreSQL、Redis 与三个独立前端入口）
# ---------------------------------------------------------------------------

# 从 Nacos 加载部署配置并构建、启动整套服务；不会生成本地部署环境文件
stack-up:
    ./scripts/compose-config.sh compose full up -d --build --wait

# 停止整套服务（保留数据）
stack-down:
    ./scripts/compose-config.sh compose full down

# 停止并删除数据卷：会清空数据库与会话
stack-reset:
    ./scripts/compose-config.sh compose full down -v

# 看整套服务的日志（可加服务名，如 `just stack-logs accounts`）
stack-logs *services:
    ./scripts/compose-config.sh compose full logs -f --tail=100 {{ services }}

# 单独构建并启动 tracing 服务、专属数据库和 tracing UI；accounts 仍作为管理员认证依赖启动
tracing-up:
    ./scripts/compose-config.sh compose full up -d --build --wait tracing-ui

# 停止 tracing 服务和 UI，但保留 tracing 数据库及数据卷
tracing-down:
    ./scripts/compose-config.sh compose full stop tracing-ui agent-tracing

# 对着正在运行的整套服务做冒烟测试：入口、登录与停用、伪造 IP 限流、备份恢复（先 `just stack-up`）
stack-smoke:
    ./scripts/stack-smoke.sh

# 备份整套服务里的数据库到 backups/（RETENTION_DAYS 控制保留天数，默认 30）
db-backup:
    ./scripts/db-backup.sh

# 从备份恢复数据库（会覆盖现有数据）：just db-restore backups/app-….dump
db-restore file:
    ./scripts/db-restore.sh {{ file }}

# 提交前的完整检查：后端 + 前端
ci-all: ci web-check web-admin-check web-tracing-check

clean:
    cargo clean

# 同步 .template-sync 中列出的通用文件；默认逐文件看 diff 并确认。
sync-template *args:
    cargo xtask template sync {{ args }}

# Template maintainer command: copy this repository to a new project.
copy *args:
    cargo xtask template copy {{ args }}
