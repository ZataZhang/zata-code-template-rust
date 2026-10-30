---
id: tracing-verify
schedule: "0 2 * * *"
timezone: Asia/Shanghai
repo_id: zata-code-template-rust
issue_type: bug
labels:
  - loop/tracing-verify
managed_by: agent-daemon
output: issue
pre_command: "grep -l '⏸️ 验证阻塞' tasks/archive/*.md 2>/dev/null | wc -l | tr -d ' ' | sed 's/^/blocked_count=/'"
skip_when: blocked_count=0
---

# 闭合「验证阻塞」PRD 的证据缺口

仓库里归档的 PRD 如果横幅是 `⏸️ 验证阻塞`，它的 §9 还留着执行侧空框，Final Reconciliation 的
`Validation blocker` / `Validation follow-up` 记着卡在哪。这个 loop 负责把那些空框补掉。

## 每一步

1. `grep -l '⏸️ 验证阻塞' tasks/archive/*.md` 列出所有阻塞中的 PRD。
2. 逐条读它的 Final Reconciliation：`Validation blocker` 说卡在哪，`Validation follow-up` 说谁闭合。
3. 先解除阻塞本身（例如 Nacos 配置缺 `AGENT_TRACING_DB_PASSWORD` 就补该键，缺服务就起服务），
   再按 PRD §7.6 的 `rv-*` oracle 跑真实验证，证据落到 `tasks/evidence/<prd-stem>/`。
4. 只勾**这次真的跑过并通过**的执行侧项；跑不过就修代码，修不了就在 Issue 里写清失败输出。
5. 执行侧空框清零后：横幅从 `⏸️` 翻到 `🧍 待人工验收`（`Human-Confirmed` 仍有空框）或
   `✅ 已验收`（没有），并从 Final Reconciliation 删掉 `Validation blocker` / `Validation follow-up`。
6. 阻塞没解除时，只更新 `Validation blocker` 的措辞与被追踪的 follow-up，不勾任何框。

## 不要做

- 不要为了让门禁通过而勾掉没跑过的项，也不要把空框改写成 `[~]`。
- 不要动 `Human-Confirmed` 组：那是人的判断。
- 不要为了让真实栈跑起来而把生产配置换成替身；缺凭据就停下并在 Issue 里报告。
