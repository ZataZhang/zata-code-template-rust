# 部署配置迁移记录

- Namespace：`rust-study-dev`；Group：`ACCOUNTS`；Data ID：`rust-study-compose.properties`。
- 已从本地既有部署文件在内存读取并发布 9 项参数，未生成或轮换任何密码。
- 发布后使用新的 Client API 请求回读，9 项配置值与原值逐项一致；按配置键值比较，忽略末尾换行格式差异。
- 发布前核对原有远端部署参数与本地相同；在运行容器的数据库、Redis、管理员引导密码与本地值相同。只记录相等标记，不记录具体密钥。
- 旧 Properties 中的本机 DATABASE_URL/REDIS_URL 不再作为部署键；应用配置继续由 `rust-study-local.toml` 独立管理。
- 按用户最新要求，`.env.local` 最终只保留 Nacos URL、用户名和密码；四个旧环境选择器已移除。命名空间、Group 和两份 Data ID 集中到 `config.toml` 的非敏感 `[nacos]`，沿用既有 rust-study 标识。
- 新统一入口通过真实 Nacos 执行 `compose full config --quiet` 成功后，再次新请求回读全部 9 项与本地一致，随后删除本地 `.env.compose`；未另存备份或临时 env。

- 仅3项引导环境下，新正式CLI `compose full config --quiet` 与 `config --services` 均通过；再次确认本地部署文件未生成。应用定位也经真实Nacos新请求读取，TOML中的数据库/Redis字段齐全。
