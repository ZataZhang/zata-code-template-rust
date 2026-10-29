# 隔离 Nacos + Compose 验证

- 命令：`python3 -u scripts/ci-nacos-stack.py`
- 最终退出码：`0`
- Nacos：本次创建的容器，限制 `1 GiB` 容器内存，JVM `Xms=256 MiB`、`Xmx=512 MiB`、`Xmn=128 MiB`、metaspace 上限 `256 MiB`。
- 认证负控：`认证 HTTP 401 传输边界 stub` 是协议 stub；不表示真实 Nacos 错误密码端到端验证。真实 Nacos 远端读取由独立临时 Nacos 完成；真实用户 Nacos 验证记录在 `real-config-source.log`。
- 日志不包含密码、token、Cookie、连接串或远端配置原文。

初次未限制 JVM 时，Nacos 在 stack 构建期间被 Docker OOM kill（退出码 `137`），导致随后 helper 入口无法连接。降低测试用 Nacos JVM heap 后再次完整运行通过；失败运行的隔离 Compose 项目和数据卷已由 harness 清理。

```text
== 启动一次性 Nacos 配置源
PASS  独立 Nacos Client API 返回远端 Compose Properties
PASS  native accounts-service 从应用 TOML Data ID 读取端口；无 DB 时 /auth/me 返回 503
PASS  compose-config fail-closed：配置不存在
PASS  compose-config fail-closed：属性格式不受支持
PASS  compose-config fail-closed：重复属性键
PASS  compose-config fail-closed：非有效端口类型
PASS  compose-config fail-closed：Nacos 不可达
PASS  compose-config fail-closed：认证 HTTP 401 传输边界 stub
PASS  compose-config fail-closed：ENV_FILE 已停用
PASS  Compose 仅执行静默或服务名配置验证
== 启动隔离 Compose stack
PASS  Nacos 值原样到达 PostgreSQL、Redis 与 accounts 容器；NACOS_* 未传入容器
PASS  完整 stack smoke（入口、认证隔离、限流、备份与恢复）
PASS  运维入口未生成 .env.compose
PASS  仅清理本次创建的 Compose 项目、数据卷与 Nacos 容器
```
