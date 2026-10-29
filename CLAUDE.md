# Claude Project Adapter

本文件是 Claude 的项目入口适配层，不是完整规范正文。

- 跨工具入口摘要见 `AGENTS.md`（**路由表与关键约束都在那里，先读它**）
- 统一规范源在 `docs/ai-standards/`
- 详细架构权威文档在 `docs/architecture/system-design.md`
- 迁移状态与未迁移内容见 `docs/migration-status.md`

## Claude Notes

- 任何任务都先读 `docs/ai-standards/index.md`，再按 `AGENTS.md` 的路由表补读相关页面，不要一次性读完所有标准页。
- 开始新的后端功能前，额外阅读 `docs/architecture/system-design.md`。
- 任何代码变更开始前，以及收集验证证据或声称完成前，重新核对 `docs/ai-standards/testing.md`。
- 修改任何 `Cargo.toml` 的 `[dependencies]` 前，先核对 `docs/ai-standards/architecture.md` 里的依赖方向。
- Skill 权威源统一为 `~/code/zata_code_template/skills/`；本仓库不保存 skill 副本，也不通过模板同步清单复制 skill 目录。
- 记录或总结「随手想法」时，遵循 `docs/guides/idea-inbox.md` 与权威源中的 `idea-inbox` skill。
- 合并前评审使用权威源中的 `code-reviewer` skill。
- 本机的 `cargo` 可能不在 PATH：Homebrew 的 rustup 是 keg-only，需要先 `export PATH=/opt/homebrew/opt/rustup/bin:$PATH`。
- 共享规范应放回 `docs/ai-standards/`，不要在本文件中复制成长篇正文。

## Maintenance Rule

- 共享规范优先写入 `docs/ai-standards/`，不要把长篇规则重新复制回本文件
- 新增标准页时，同时在 `AGENTS.md` 的路由表中加一行触发条件
