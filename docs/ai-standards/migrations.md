# Database Migrations

账号服务用 [`sqlx`](https://github.com/launchbadge/sqlx) 管理 PostgreSQL 迁移。迁移文件是**纯 SQL**，由该服务独占管理，放在 `services/accounts/migrations/`。将来新增业务服务时，各自维护自己的迁移目录和数据库，不得把 SQL 写进账号迁移中。

## 基本约定

- 文件名：`<14 位时间戳>_<简短描述>.sql`，例如 `20261016090000_create_users.sql`；时间戳决定执行顺序
- 新增账号迁移用 `sqlx migrate add --source services/accounts/migrations <描述>`（需要安装 `sqlx-cli`：`cargo install sqlx-cli --no-default-features --features postgres`），不要手工编造时间戳
- **已经应用到任何环境的迁移文件不要再修改**。sqlx 会记录每个迁移的校验和，改动后启动会报错；需要变更就新增一个迁移
- 一个迁移只做一件事；破坏性变更（删列、改类型）要先确认数据如何处理，并在文件头注释里写清楚
- 迁移里不要放业务数据的种子，种子数据由应用代码或单独脚本负责

## 表与列必须有注释

每张新表和它的每一列都要用 `COMMENT ON` 写说明，作为运维排障时的字段说明来源（对应 Python 版的「模型必须带 comment」规则）：

```sql
COMMENT ON TABLE users IS '用户账号：邮箱加密码登录';
COMMENT ON COLUMN users.email IS '登录邮箱，已规范化为小写，全局唯一';
```

约束要有名字（`CONSTRAINT users_email_unique UNIQUE (email)`），报错信息和排障时才看得懂。

## 何时执行

- **账号服务启动时自动执行**（`services/accounts/src/composition.rs` 调用 `persistence::run_migrations`），已应用的会被跳过，所以可以重复启动
- 配置了 `DATABASE_URL` 就要求启动时数据库可达；不可达则启动失败并以非 0 退出（快速失败，交给编排平台重启）
- 迁移 SQL 在**编译期**嵌入账号服务二进制（`sqlx::migrate!`），部署时不需要带迁移目录；但**构建镜像时 `services/accounts/migrations/` 必须在构建上下文里**（见 `Dockerfile`）
- `crates/infrastructure/build.rs` 监视 `services/accounts/migrations/`，改了 SQL 会触发重新编译

## 查询的写法

- 生产查询使用 `sqlx::query!` / `query_as!` / `query_scalar!`，让 SQL 语法、列名、绑定类型和返回字段在编译期对照 PostgreSQL schema 检查
- **永远用 `$n` 参数绑定，不拼接用户输入**；宏要求 SQL 是字面量，不能抽成运行期格式字符串
- 编译使用仓库根目录 `.sqlx/` 的离线元数据，不要求本地、CI 或 Docker 构建时连接数据库。修改 SQL 或迁移后先 `just db-up`，再运行 `just sqlx-prepare` 并提交更新后的 `.sqlx/`
- `just sqlx-prepare` 会先对本地测试数据库执行 `services/accounts/migrations/` 中尚未应用的账号迁移，再运行 `cargo sqlx prepare --workspace -- --all-targets`。首次需安装匹配 SQLx 版本的 CLI：`cargo install sqlx-cli --version 0.9.0 --locked --no-default-features --features postgres`
- CI 和 Docker 构建设置 `SQLX_OFFLINE=true`，确保使用提交的元数据；运行时仍由真实数据库测试（`just test real`）验证 SQL 行为

## 怎么测

迁移和 SQL 必须在**真实 PostgreSQL** 上验证（不能只用 mock 或 SQLite）：

```bash
just db-up
just test real
```

真实数据库测试要自己清理数据，且不依赖库里已有的数据（见 `testing.md`）。涉及迁移的改动，至少验证：全新空库上能完整执行；在已执行过旧迁移的库上能增量执行；随后刷新 `.sqlx/` 并用离线模式编译。
