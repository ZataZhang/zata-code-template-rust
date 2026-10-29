# Nacos 部署配置交付证据

## 人审导航 / Human Review Navigation

部署参数已迁入远端，本地密码文件已移除。打开 `just prd review P1-CONFIG-20261029-160000-nacos-compose-source.md`，查看原值回读一致、文件不存在及正式命令读取结果。离线页面为 [人工审查清单](human-review-checklist.html)，详细事实见 [迁移记录](migration-report.md)。本次没有创建 PR，远端 GitHub Actions 未运行。

## rv-1 · 真实环境迁移

原有 9 项部署值在内存迁移至真实 Nacos，以新 Client API 请求逐项比较一致，没有轮换密码。实际 `.env.local` 仅保留地址、用户名、密码；项目定位由 `config.toml` 提供。正式 CLI `compose full config --quiet` 和 `config --services` 成功，`.env.compose` 不存在且未再生成。最新 CLI 重查 exit 0。未重启用户容器或连接其数据库进行写入。

- [迁移记录](migration-report.md)
- [真实配置源验证](real-config-source.log)

## rv-2 · 隔离真实入口

[隔离验证记录](isolated-harness.md)：独立真实 Nacos → 配置 CLI → Docker 环境 → 实际 PostgreSQL/Redis/accounts 容器；容器有效值逐项相等，Nacos 凭据未进入业务容器。真实 HTTP 入口的注册登录、管理员隔离、限流和备份恢复通过，未生成本地部署文件。应用 TOML 另经原生服务端口和健康请求验证。

失败负控包括不存在配置、未知格式、重复键、非法端口、不可达和旧 ENV_FILE；Docker 调用记录均为空。认证拒绝仅用 HTTP401 传输 stub，未声称完成真实 Nacos 错密码测试；真实用户 Nacos 成功认证见 rv-1。

首次测试 Nacos 被 OOM kill；降低隔离测试 JVM heap 后完整复验通过。隔离资源只按本次唯一项目清理，未清理用户资源。

## rv-3 · 构建与静态检查

`SQLX_OFFLINE=true just ci` 退出 0，覆盖 lint、workspace 常规测试和文档；其中 4 项需独立服务的 Rust 集成测试按常规定义 ignored，不计为本轮已执行。相关配置单测 17 项、CLI 单测 4 项通过。真实服务层由 rv-2 的隔离栈另行验证。见 [验证摘要](verification-summary.md)及[完整构建记录](just-ci.md)。工作流只完成本机检查，不声称远端 CI 已执行。[独立审查](verifier-report.md)结论 approved/PASS，0 项未解决 finding；359 项清单全部匹配。

## 来源与人工呈递

[源码清单](source-manifest.json)与[来源说明](source-provenance.md)绑定未提交工作树，涵盖此前账号模块交付和本次改动。证据不包含密钥原值或连接串。[浏览器验证](review-ui.log)确认本地清单在 Chrome 可见、无 pageerror、可生成回复；该自动检查不代表人工验收。

## 限制

Nacos 是部署与维护命令的前置依赖；每次命令读取，修改配置后需重新启动相关服务才能生效。运行进程、容器环境及正常数据库备份仍会持有必要信息；本次消除的是本地部署配置文件。未实现热更新或密钥轮换。
