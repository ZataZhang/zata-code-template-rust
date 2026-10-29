# 本轮验证计划与基线

来源：`tasks/pending/P1-CONFIG-20261029-160000-nacos-compose-source.md` §7.6。

## 起始状态

- 工作树含上轮已完成的账号服务迁移，以及后来新增的应用 Nacos TOML 加载和本机开发启动脚本；本轮保留这些修改。
- 应用配置读取使用 `NACOS_DATA_ID=rust-study-local.toml`，已通过真实 Nacos 认证/读取/TOML 解析。
- 本轮开始时 Compose、备份恢复仍读本地 `.env.compose`，`stack-env.sh` 会自动生成该文件。该问题是本轮实际修复对象。
- 本地部署配置含 9 项；Nacos Properties 原有 6 个非敏感部署键和两个本机应用连接串。两个应用连接串由应用 TOML 独立管理。
- 迁移前检查本地部署密码与正在运行容器一致。只记录相等标记，任何报告不得记录原值。

## 必需真实验证

- rv-1：在内存读取原配置，向真实本机 Nacos 发布，使用新请求回读并逐项对比全部9项。新CLI通过真实Nacos读取后，移除本地部署文件，再检查正式入口和文件不存在；不得另存备份或临时env。
- rv-2：独立临时Nacos与隔离Compose项目；远端值经统一CLI进入实际容器，认证、限流、备份恢复通过。配置无效/认证错误/不存在/不可达时，用测试边界的调用记录证明Docker未执行。
- rv-3：just ci、shell语法、actionlint、入口搜索与文档同步。前端UI/API不变，不重跑无关页面设计验证。

验证中的 Nacos、数据库、Redis和容器均使用自建项目，不升级/重建用户当前 tpl，也不删除其数据卷。原始日志留在忽略的 evidence 下，且在输出前遮盖临时凭据；秘密配置不能写入日志或临时env。

用户最新要求：项目 locator 集中 config.toml，默认 env 只 URL/用户名/密码3项，应用/Compose读取已在真实用户Nacos验证。CI认证拒绝允许HTTP状态传输边界stub，必须如实标记；不能描述成真实Nacos坏密码验证。
