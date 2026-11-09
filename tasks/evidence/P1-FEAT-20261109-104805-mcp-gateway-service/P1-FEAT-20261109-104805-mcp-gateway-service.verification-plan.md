# MCP Gateway 服务验证计划

- PRD：`tasks/archive/P1-FEAT-20261109-104805-mcp-gateway-service.md`（由 runner 归档）
- Issue：https://github.com/ZataZhang/zata-code-template-rust/issues/4
- 执行日期：2026-11-09
- 证据目录：`tasks/evidence/P1-FEAT-20261109-104805-mcp-gateway-service/`

## 1. 验证目标

按 PRD §7.6 的 Realistic Validation 计划，通过真实服务入口验证三条 oracle：

| RV | 行为 | 真实入口 | 证据文件 |
|---|---|---|---|
| rv-1 | 获准用户只看到并调用授权工具；未授权用户、其他租户与另一消费服务的同租户连接均被拒绝 | `just mcp-e2e --scenario authorization` | `rv-1-authorization.txt` |
| rv-2 | 管理连接、加密凭据、出站安全、进程重启后真实 MCP 调用 | `just mcp-e2e --scenario remote-connection` | `rv-2-remote-connection.txt` |
| rv-3 | 已发布契约覆盖连接管理与网关，且明确不含市场/消费方 UI | 阅读 `docs/guides/mcp-service.md` 与 `services/mcp/openapi.yaml` | `rv-3-docs-scope.txt` |

## 2. 环境与边界

- macOS arm64；Rust 1.99.0；Docker 29.4.2。
- 真实 PostgreSQL：`tpl-testing-mcp-postgres-1`（`127.0.0.1:5436/mcp_dev`），由 `docker-compose.testing.yml` 提供。
- 真实服务进程：`scripts/mcp-e2e.sh` 构建并启动 `mcp-service` 二进制，执行真实迁移、真实 HMAC 签名上下文校验、真实 rmcp Streamable HTTP 客户端/服务端。
- 唯一替身：远端 MCP provider 由本地假上游替代；服务路由、认证、授权、仓储、AES-GCM 加密、出站地址校验与 MCP SDK 均为真实实现。
- 不捕获任何真实凭据；证据中的 token 均为测试固定值或临时生成值，且不写入证据文件。

## 3. 执行顺序

1. 先运行负控并记录红色结果（仅在测试边界临时反转断言或临时删除契约路径，不改生产代码；恢复后重跑）。
2. 运行 rv-1、rv-2、rv-3 场景，将每个场景的完整输出单独写入对应 `rv-<n>-<slug>.txt`，禁止多个 item 混写。
3. 运行 `just test`、`just ci`、`just test real`。
4. 运行 `just mcp-docker-build`，再通过 `just mcp-up` 启动 Compose 栈并请求 `/ready`。
5. 运行 `kc evidence check --evidence tasks/evidence/P1-FEAT-20261109-104805-mcp-gateway-service`，确认 manifest 3/3 通过。
6. 在最终代码状态重跑 rv-1/rv-2/rv-3，并记录工作树指纹。
7. pre-PR 复审修复后，在最终树重跑 rv-1/rv-2 负控（打红）、恢复断言与依赖，重捕证据，并重跑 `just ci`、`just test real` 与 `just mcp-docker-build`。
8. verifier 复审修复后，在最终树重跑 rv-1/rv-2/rv-3（两者以 `RUST_LOG=debug` 运行），重捕证据并更新 `evidence.json`、证据报告与 PRD Change Log。
9. pre-PR 复审（code-reviewer）修复后，在最终树重跑 rv-1/rv-2/rv-3 与 `just test`、`just ci`、`just test real`、`just mcp-docker-build`，重捕证据并更新证据报告与 PRD Change Log。

## 4. 负控设计

| RV | 负控方式 | 预期失败信号 |
|---|---|---|
| rv-1 | 测试边界把未授权用户断言反转为“必须看到工具” | `assertion failed`，`test result: FAILED`，退出码 101 |
| rv-2 A | 临时把 endpoint 地址策略改为全部放行，运行 rv-2 | `目标 https://169.254.169.254/latest/meta-data 必须在写入阶段被拒绝`，`left: 201`、`right: 400`，退出码 101 |
| rv-2 B | 临时移除 gateway 对单个连接失败的跳过逻辑，运行 rv-2 | `tools/list` 返回 `-32005`，`test result: FAILED`，退出码 101 |
| rv-3 | 临时从 OpenAPI 删除 `/mcp` 路径 | 输出 `失败：OpenAPI 缺少路径 /mcp`，脚本非零退出 |

负控只改测试/文档临时状态；rv-2 的两个负控在依赖边界临时注入，恢复时用 `cmp` 与备份逐字节核对，不保留任何测试专用生产开关。恢复后同一条命令必须变绿。

## 5. 通过标准

- 三个 RV 脚本退出码为 0，输出包含对应检查点与 `test result: ok`；rv-1/rv-2 以 `RUST_LOG=debug` 运行，rv-2 含 HTTPS 地址矩阵、开发模式私网拒绝与不可用上游隔离检查点。
- `kc evidence check` 的 `stdout_assertions` 全部满足，结果为 3/3 PASSED。
- `just test`、`just ci`、`just test real`、`just mcp-docker-build` 均 exit 0。
- `just mcp-up` 后 `/ready` 返回 `status=ready` 且 `checks.database=ok`。
- 证据文件不含凭据明文、工具参数或工具结果。

## 6. 已知限制

- 未连接真实第三方 MCP provider；TLS、证书与外部网络差异不在本机证据范围内。
- Compose 验证使用临时隔离 Nacos 组补齐共享开发 Nacos 中缺失的无关 Agent 键；验证后已删除临时配置，不修改现有部署配置。
- `rv-3` 的自动化只校验契约路径、方法与排除项关键词，最终 scope 清晰度仍由人工 reviewer 判断。
- 未创建 PR；PR 呈递与独立 verifier 结论由 runner 在交付门禁中完成。
