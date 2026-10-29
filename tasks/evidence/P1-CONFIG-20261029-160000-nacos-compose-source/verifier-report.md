# 独立审查报告

## 审查范围与验收标准

依据 `tasks/pending/P1-CONFIG-20261029-160000-nacos-compose-source.md` 及其 §7.6/§9 验收项，审查最终源码快照与交付证据。预期行为是：原部署值原样留在 Nacos；所有正式 Compose、备份、恢复和 smoke 入口先读 Nacos，失败时不调用 Docker；应用 TOML 与 Compose Properties 分别从项目 `[nacos]` 中的 Data ID 读取；不生成本地部署 env 文件；CI 使用隔离配置且不依赖用户 Nacos 凭据。

本次没有审查或修改上一轮账号模块实现，也没有运行会操作现存部署的命令。

## 发现

无严重、高、中或低优先级的未解决实现、需求或文档缺陷。

重点检查结果：

- `services/accounts/src/bin/compose_config.rs` 先加载并校验 Nacos 配置，再构造 Docker 子进程；子进程环境通过 `env_clear` 限定，Compose 固定使用 `--env-file /dev/null`。公开 Compose 命令有白名单，裸 `config`、环境导出、`exec`、`run` 和旧 `ENV_FILE` 均被拒绝。日志只打印项目名和端口。
- `crates/infrastructure/src/config/nacos.rs` 从 `config.toml` 的 `[nacos]` 读取 namespace/group 与两个独立 Data ID；Nacos 地址和认证凭据从环境读取。HTTP 客户端不跟随重定向、设置请求超时并限制响应大小；错误不包含配置正文或凭据。
- `crates/infrastructure/src/config/compose.rs` 限制可注入键，拒绝重复/未知键、控制字符与无效类型；值按字面量保留。隔离 harness 以含空格、引号和 `${...}` 的管理员密码实测值无变形地到达容器。
- `justfile` 的正式部署和维护入口统一经 `scripts/compose-config.sh`；备份、恢复和 smoke 的内部 helper 接收同一个受限环境。测试中管理员登录请求体从 stdin 传递，Redis CLI 认证走环境变量，未把这些秘密放进命令参数。
- `.env.compose`、`.env.compose.example` 与 `scripts/stack-env.sh` 均不存在；CI 对本地文件不存在和命令执行后未重建均有断言。`ENV_FILE` 负控在 Docker 调用前失败。
- CI harness 明确将 Nacos 用户名、密码和 token 置空，使用临时 locator TOML、独立 Nacos 与随机 Compose project，不继承用户私服凭据；原生应用配置测试清理继承的 DB/Redis/Admin 环境，避免误连本机业务数据库。
- 部署示例、`config.toml`、`.env.example`、README 与配置/部署/备份指南一致：`.env.local` 仅配置 Nacos URL、用户名和密码，locator 在 `[nacos]`，部署参数不写入本地文件。已检查迁移指南中保留的旧版 `.env.compose` 命令只用于旧版本停机迁移。

## 验证证据

### rv-1：真实值迁移与正式入口

- `tasks/evidence/P1-CONFIG-20261029-160000-nacos-compose-source/migration-report.md` 记录从原部署文件内存发布到真实 Nacos，并用新请求逐项比对 9 项配置；密码未轮换。迁移前值与运行容器环境相等，记录未包含原值。
- `real-config-source.log` 记录引导环境只有 `NACOS_SERVER_ADDR`、`NACOS_USERNAME`、`NACOS_PASSWORD`，locator 来源为 `config.toml [nacos]`；`config --quiet`、`config --services` 和应用 TOML 读取均成功；本地 `.env.compose` 不存在。
- 迁移记录说明仅忽略属性文件末尾换行格式差异，键值逐项一致；删除本地文件前已完成发布、回读与新 CLI 读取。

### rv-2：隔离 Nacos、Compose 与失败关闭

- `isolated-harness.md` 记录退出码 `0`：独立 Nacos → 正式 CLI → PostgreSQL、Redis、accounts 容器；容器环境值与 Nacos 值相等，Nacos 引导参数未传入业务容器。
- 真实 HTTP smoke 覆盖前台与管理入口、普通用户和管理员登录隔离、限流、备份、删数据、恢复及恢复后健康状态。
- 不存在 Data ID、未知键、重复键、非法端口、Nacos 不可达、旧 `ENV_FILE` 等失败负控均用 Docker 调用记录确认没有运行 Docker。公开 CLI 的敏感输出子命令另有拒绝测试。
- 认证拒绝负控是本机 HTTP `401` 传输边界 stub，不是“真实 Nacos 错误密码”验证；该 mock 边界已在 PRD 与 harness 证据中明确。真实 Nacos 的成功认证/读取由 rv-1 验证。

### rv-3：静态检查与工作树来源

- `SQLX_OFFLINE=true just ci` 通过，包含 `just lint`、workspace 测试与 rustdoc；标准测试中 4 个需要 PostgreSQL/Redis 的测试仍按项目约定标记为 ignored，真实栈覆盖由 rv-2 提供。
- `actionlint`、shell 语法、CI Python 编译、旧入口/旧选择器搜索、文档同步检查均通过，详见 `verification-summary.md`。
- 我只读复核 `source-manifest.json`：359 个条目均存在且 SHA256 全部匹配；来源说明记录聚合 SHA256 `6f66977091a422c906f45b9f103da16a233957a12cc783ad4a47b62963e5e07e`。
- `uv run mkdocs build` 未运行；它不是本 PRD 明确门禁，且未在本轮证据中要求。该项仅作为常规验证状态记录，不构成缺陷。

## 结论

需求实现、代码安全边界、交付验证与文档同步均通过审查。认证拒绝仅验证 HTTP 401 传输边界；报告未将其说成真实 Nacos 错密码测试。Human-Confirmed 仍由用户验收，本报告不代替人工勾选。

## 评审总结

🔴 严重：0 ✅

🟠 高：0 ✅

🟡 中：0 ✅

🔵 低：0 ✅

需求状态：✅ 通过

代码审查：✅ 已完成（代码发现：0）

验证状态：✅ 必需门禁通过；`uv run mkdocs build` 未运行，非本次必需门禁

文档状态：✅ 通过

结论：✅ 通过 — PRD 要求及真实迁移/隔离验证均完成；人工验收仍待用户执行。

```json
{"verdict":"approved","findings":0,"required_gates":"passed"}
```
