# 验证摘要

- `SQLX_OFFLINE=true just ci`：退出码 `0`；完整脱敏 stdout 见 [just-ci.md](just-ci.md)。正常 workspace 测试通过；4 项需真实 PostgreSQL/Redis 的集成测试按常规标记为 ignored。
- `python3 -u scripts/ci-nacos-stack.py`：退出码 `0`；结果和完整脱敏 stdout 见 [isolated-harness.md](isolated-harness.md)。
- `bash -n`：通过，覆盖 `compose-config.sh`、stack smoke、备份与恢复包装器及其实现脚本。
- `python3 -m py_compile scripts/ci-nacos-stack.py`：通过。
- Python helper 参数审计：通过，所有函数均不超过 3 个参数。
- `actionlint .github/workflows/ci.yml .github/workflows/cd.yml`：通过。
- `bash -n scripts/*.sh scripts/lib/*.sh`：通过。
- `git diff --check` 与 active tree 对旧 Nacos selector / 过时 `.env.local` 定位描述的检索：通过。
- 入口检索 `rg -n 'docker compose|docker-compose' justfile scripts docker-compose.yml docker-compose.accounts.yml`：生产 Compose 启停入口由 `just` 调 `scripts/compose-config.sh`；直接调用 Compose 的 helper 只从该入口运行，CI harness 的直接 `down` 固定用于本次随机隔离 project；其余 `just`/浏览器测试直接调用仅使用 `docker-compose.testing.yml`。
- 相关 unit tests：`tpl-infrastructure` 配置测试 17/17 通过；`tpl-accounts --bin compose-config` 测试 4/4 通过（均包含在 `just ci` 内）。

隔离验证使用随机 Nacos namespace/group/Data ID 和随机 Compose project，只清理 harness 创建的容器、网络和数据卷。没有连接或修改用户现存 stack，也没有生成 `.env.compose`。真实 Nacos 的三项引导环境与远端应用配置读取结果另见 `real-config-source.log`。
