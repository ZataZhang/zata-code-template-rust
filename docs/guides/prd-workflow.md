# PRD 工作流

PRD（产品需求文档）是本项目里**较大改动**的计划载体：先写清楚「要什么、怎么证明做对了」，再动手实现。
流程与格式由 `~/code/zata_code_template/skills/prd/` 中的权威 skill 规定；本仓库不保留副本。本页只讲**怎么用工具**——工具是 Rust 写的（`tools/xtask`），通过 `just prd ...` 调用。

> 什么时候该写 PRD：新增一项业务能力、改动层间契约、改变对外接口、数据库结构变化——凡是「做错了代价高，或者需要人先确认方向」的事。
> 改个文案、修个小 bug、调个配置，不用。想法还不成熟时先落 `tasks/inbox/`（见 [想法收集流程](idea-inbox.md)）。

## 生命周期

```text
tasks/inbox/      想法（原话 + AI 背景，只追加）
      │ 想法成熟
      ▼
tasks/pending/    PRD：写计划 → kc 建 issue 并认领 → 实现 → 收集证据
      │ 执行侧全部交付（verifier 通过、对账完成；kc closeout 门禁通过后 git mv）
      ▼
tasks/archive/    执行侧已交付；横幅 🧍 待人工验收 → ✅ 已验收
```

**执行权归 KedaCode（kc）**：PRD 的开工、认领与归档由 kc 的 issue 生命周期负责（`kc issue create` → runner 认领 → closeout 归档）。本页的 `just prd` 工具是**人工直跑**时的旁路，不是 agent 的正常入口；两套不要同时用——经 kc 执行时不要再领锁，否则同一个 PRD 会出现两个 owner。

两条轴相互正交，别混为一谈：

- **目录轴**（`pending` → `archive`）只表示「执行侧的活做完了，并且和代码一起落地了」。
- **验收轴**（PRD 头部的横幅）表示「人确认了没有」：`⬜ 未开工` → `🧍 待人工验收` → `✅ 已验收`。**人工验收可以晚于归档，也从不拦归档。**

## 一次完整的流程

```bash
# 1. 写 PRD：用 PRD skill 生成到 tasks/pending/，文件名 P<优先级>-<类型>-<日期>-<时间>-<slug>.md
just prd check --structure-only tasks/pending/<prd>.md     # 新写出来的 PRD 先过结构检查

# 2. 开工：交给 kc（正常路径）
kc issue create tasks/pending/<prd>.md                     # 建 issue 并把 PRD 链接写回文件
just prd status                                            # 只读看板：进度、依赖、验收横幅

#    人工直跑（不经 kc）才需要互斥锁
just prd start <prd> --tool claude
just prd heartbeat <prd>                                   # 长任务续期

# 4. 交付：把验收清单勾完、写最终对账、横幅改成 🧍 待人工验收，再检查并移到 archive
just prd check --archive-ready tasks/pending/<prd>.md
git mv tasks/pending/<prd>.md tasks/archive/<prd>.md
just prd release <prd>

# 5. 人确认之后：勾选 Human-Confirmed、横幅改成 ✅ 已验收、追加 Change Log
just prd review <prd>                                      # 打开人工审查清单
```

## 命令速查

| 命令 | 作用 |
|---|---|
| `just prd status [all\|pending\|archive] [--detail]` | 看板：优先级、类型、日期、**清单进度**、**验收横幅**、**交付依赖**、**执行锁**、**证据包**。分 `PENDING` / `AWAITING HUMAN`（已归档、等人确认）/ `ARCHIVE`（按月折叠）三段 |
| `just prd start <prd> [--tool 名称] [--branch 名称]` | **仅人工直跑**：领执行锁。他人的新鲜锁 → 拒绝并告诉你是谁（退出码 1）；过期锁 → 自动接管并留档；自己已持有 → 刷新心跳。经 kc 执行时不要用 |
| `just prd heartbeat <prd>` | **仅人工直跑**：续期自己持有的锁 |
| `just prd release <prd> [--force]` | **仅人工直跑**：释放；不是自己持有的锁需要 `--force` |
| `just prd check <prd>... [--structure-only \| --archive-ready]` | 门禁，见下 |
| `just prd check --archived` | 审计 `tasks/archive/` 下全部 PRD |
| `just prd review <prd> [--print]` | 打开人工审查清单（没有就回退证据报告）；`--print` 只打印路径 |

`<prd>` 可以是路径、文件名，或 **slug**（文件名里日期之后的那段短标识）。PRD 归档后，同一个参数依然找得到。

## 门禁：检查什么

三档严格度，越往后越严：

| 档位 | 用在哪 | 检查内容 |
|---|---|---|
| `--structure-only` | 新写出来的 pending PRD | 13 个必需章节及顺序；Part A（给人审的）里不能出现执行器才用的元数据（`rv-N`、证据链字段）；§1 的**解读回显**（至少 3 行行为样例表 + 「我默默定了这些」「我理解为不做」两个块）；FR 编号从 1 连续；**验证 oracle** 的 YAML 模式（按风险等级 R0–R3 要求不同字段，R3 还要负控）；§8 交付依赖的取值词表与顶部横幅一致 |
| 默认（给了路径） | 交付中的 PRD | 上面全部 + 执行侧欠的验收项都已勾 |
| `--archive-ready` | 归档前 | 再加：**最终对账**（Final Reconciliation）四个字段填实；**横幅与 §9 一致** |

横幅与 §9 一致的规则：`Human-Confirmed` 组里还有空框 → 必须是 `🧍 待人工验收`；一个不剩 → 必须是 `✅ 已验收`；`⬜ 未开工` 永远不可归档。**`Human-Confirmed` 组的空框不拦归档**——只有人能回答的项必须放在这个组里，放在别处会让归档永远过不去。

提交时由 pre-commit 钩子自动检查「新归档」的 PRD（改动已归档 PRD 的人工验收记录不会被重新裁决）。

## 执行锁

**kc 接管后，本节描述的锁不参与 agent 流程**：kc 用 issue 标签、`.kc-worktrees/` 工作区和 `kc peers` 的认领记录来保证同一时刻只有一个执行者。锁只在人工直跑时作为本机互斥使用，留着是为了这条旁路，以及历史归档 PRD 的记录仍可解析。

`tasks/evidence/<prd-stem>/active.lock`，本机文件，不进 git。要点：

- **同一条 PRD 的领锁、续期、释放都被操作系统文件锁串行化**，所以多个进程、多个 worktree 同时领锁，恰好一个成功；进程崩溃时系统自动释放互斥，不会留下僵尸锁。
- **过期看心跳（30 分钟）加 worktree 活性，不看 pid。** 心跳过期但持锁者的 worktree 近期仍有文件改动，视为还在干活，拒绝接管。（agent 的每条命令常常是新会话、进程很快退出，看 pid 会让所有锁瞬间过期。）
- 在主仓库领的锁，被同一仓库的 linked worktree 再次领取时，视为「开工入口把活交给执行器」，归属自动移交。
- 锁在**主仓库**里，所以所有 worktree 看到同一份。

## 与 Python 版的差异

重写时保留了契约、门禁规则、锁语义和看板的核心；下面这些**没有移植**：

| 没移植 | 原因 |
|---|---|
| 看板的 `FILES` 列（影响树触达进度）、按分支 worktree 副本优先读取进度 | 依赖大量 git worktree 与文件系统探测，收益小、复杂度高；看板读主仓库里的副本，以主线为准 |
| `just ai implement`（AI 执行器 + 独立 verifier 的编排脚本）、证据密钥扫描脚本 | 绑定特定的 agent 命令行与提示词，不是通用底座；实现与验证仍按 skill 描述手动进行 |
| 人审清单的交互 HTML 生成与结构质检脚本 | 呈现层；模板随上游 `prd` skill 分发，由 agent 按 skill 手写 |

另有四处有意的改进（在 Rust 版里）：

1. **只有一份契约解析**。Python 里同一份解析被抄了三份（skill 的校验器、看板、钩子），靠一个守卫测试逐例比对防漂移；这里是一个库，所有命令共用。
2. **契约版本被测试钉死**：`SKILL.md` 里的 `Machine-Contract-Version` 与代码里的 `CONTRACT_VERSION` 不一致，测试直接失败。
3. **修了一个门禁漏洞**：模板里「无可执行行为时，本块写：……」那句提示本身就含有豁免声明，旧版只要看到这句话就整段放行——作者忘了删它，真实的 oracle 块就完全不被校验（连 `tier: R0|R1|R2|R3` 这样的占位符都能蒙混过关）。现在只有「声明了无可执行行为、且确实没有 oracle 条目」才豁免。
4. **锁的并发缺陷**：Python 的接管算法是「把疑似过期的锁改名挪走、事后核对、不对再放回去」，挪走到放回之间锁路径是空的，第三方可以趁机创建自己的锁，随后又被「放回」覆盖，造成两个会话同时认为自己持有锁。这个窗口在并发测试里实测复现过，所以改成了文件锁串行化。
