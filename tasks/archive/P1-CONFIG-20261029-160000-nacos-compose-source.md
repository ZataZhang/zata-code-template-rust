# PRD: Nacos 替代本地 Compose 部署配置

> ✅ **交付前置**：无，可立即开工。投影自 §8。
> 🧍 **验收状态**：待人工验收。投影自 §9。

## Feature Overview (功能一览)

- FR-1：部署命令从 Nacos 读取配置，在进程中注入 Compose。
- FR-2：现有部署参数与密码原样迁移，核对后移除本地实际配置文件。
- FR-3：备份、恢复和诊断使用同一配置来源；远端故障立即失败。
- FR-4：提供真实集成验证、独立 CI 配置源及准确使用说明。

# Part A · 人审层 (Review Layer)

## 1. Introduction & Goals

### Problem Statement

当前虽然已把部署参数存入远端，但启动、停止、日志和备份恢复仍依赖本地部署文件，首次启动甚至会生成本地密码。用户明确要求远端配置成为实际来源，部署参数和密钥不再保存在本地部署文件。

### Interpretation (解读回显)

| 验证方式 | 输入 / 操作 | 期望观察到的结果 |
|---|---|---|
| 👀 人审 + 自动验证 | 将现有部署参数迁移后运行部署命令 | 参数和密码保留，远端成为真实来源，本地部署配置文件不存在且不会再生成 |
| 🤖 自动验证 | 在隔离环境启动服务并执行备份恢复 | 服务实际使用远端值，用户/admin认证与数据恢复通过 |
| 🤖 自动验证 | 远端不可达、拒绝认证、配置不存在或格式错误 | 命令在启动Docker前失败，不回退本地旧文件或生成密码 |
| 🤖 自动验证 | 本机应用继续读取其已有应用配置 | 应用配置与部署配置分别定位，不把宿主数据库地址误用于容器 |

**我默默定了这些**

- 应用启动配置和容器部署配置保持两份，容器数据库继续走内部服务地址。
- 本机环境只需远端地址、用户名和密码；命名空间、分组和两份配置ID集中在项目的非敏感配置中，采用项目名前缀。实际部署参数只在远端及运行中进程/容器环境中存在。
- 保留所有现有密码，不重置数据库或管理员，不升级用户正在运行的容器。

**我理解为不做**

- 不实现动态热更新、密钥轮换、SSO或新的配置平台。
- 不把运行中容器的正常进程环境描述成“完全不留任何副本”，目标是移除本地部署配置文件。

所有正式部署入口都必须读取同一个远端部署配置。远端不可用时停止操作；本机应用现有配置来源保持独立。最终交付必须实际删除本地部署配置文件，而非仅保存副本；引导环境不再要求用户填写配置ID。

### What The User Gets

以后启动和维护容器都使用远端配置，不需要在项目目录保存一份部署密码文件；已有密码与数据保持有效。

### Measurable Objectives

- 正式命令不读取、不生成本地部署配置文件，也不通过临时文件保存部署密钥。
- 远端读取失败时 Docker 不被调用。
- 新配置源驱动的真实隔离容器认证、限流和备份恢复通过。

## 2. Human Review Map (介入与风险地图)

### 决策：远端作为部署参数唯一持久来源

建议保留必要的远端登录引导信息，将现有部署参数及密码原样迁入独立的远端部署配置。远端故障会使运维命令失败，因此需要保证远端先于业务服务启动。

**请确认：** 迁移后的本地部署密码文件已移除、正常维护命令直接使用远端参数，是否满足“不再本地存”的目标？用户已授权实施，交付效果待最终审阅。

**验收：** 查看脱敏迁移报告，确认远端发布与回读一致、本地文件不存在，以及真实容器验证结果。

### 自动门禁，不需要逐项人工审阅

真实配置源读取、失败关闭、参数解析、命令注入隔离、实际容器和备份恢复均由自动验证与独立审查负责。

### 本次明确不涉及

不改数据库模型、认证权限、现有密码、应用配置解析格式或用户正在运行的部署。

## 3. Usage And Impact After Implementation

运维继续使用已有启动、停止、日志和备份恢复入口，操作前从远端加载部署参数。维护者通过远端配置修改后续启动参数，引导地址与认证凭据由本机环境提供，命名空间和配置ID由项目配置固定。普通用户和管理员继续使用原有页面与账号；本次不改变用户交互。开发者本机运行的应用继续读取它原有的远端 TOML。

## 4. Requirement Shape

- Actor: 运维、配置维护者、开发者、普通用户与管理员。
- Trigger: 执行容器部署、诊断、备份或恢复。
- Expected behavior: 远端部署配置直接进入进程，不写本地密码文件；原有行为兼容。
- Scope boundary: 当前账号栈的部署配置源迁移与必要测试工具。

# Part B · 执行器层 (Build Layer)

## 5. Repository Context And Architecture Fit

- Existing path: `scripts/stack-env.sh` 生成 `.env.compose`；justfile 和备份恢复脚本使用 `--env-file`；应用 Nacos 读取已在 `crates/infrastructure/src/config/nacos.rs`。
- Reuse candidates: 现有 Nacos 认证、超时、有界响应读取；现有 Compose 配置及备份恢复逻辑。
- Architecture: 配置源位于基础设施层，服务装配入口提供开发期 CLI；不让 core 依赖网络库。
- Frontend impact: No frontend impact — 不改页面、浏览器接口或前端业务，仅验证既有入口代理仍正常。
- Existing PRD relationship: pending 无其他PRD；已归档账号模块PRD定义现有部署行为，当前工作树还有已完成的应用Nacos读取改动，必须保留。
- Redundancy risks: 不复制第二份Nacos HTTP客户端，不维护本地与远端双轨部署模式。

## 6. Recommendation

### Recommended Approach

复用现有远端客户端，增加部署配置读取入口与独立开发期 CLI，向 Docker 子进程传入环境变量。远端部署配置继续用现有 Data ID，应用 TOML 的 Data ID 保持独立。

### Proposed Solution Summary (实现机制)

在项目 `config.toml` 的 `[nacos]` 中固定 `namespace_id=rust-study-dev`、`group=ACCOUNTS`、`app_data_id=rust-study-local.toml`、`compose_data_id=rust-study-compose.properties`。本机 `.env.local` 默认仅保留 `NACOS_SERVER_ADDR`、`NACOS_USERNAME`、`NACOS_PASSWORD`；可选令牌高级方式保留。应用和部署读取器共用配置定位解析，遵循已有 `APP_CONFIG_FILE` 选择本地非敏感项目配置；撤掉旧ID/namespace/group环境选择器。读取后解析受限字面量KEY=value，拒绝未知/重复键及非法控制字符，不做shell展开。所有正式命令显式 `--env-file /dev/null`，避免本地env回退；部署密钥通过进程环境传递，不写临时env或打印完整配置。公开CLI仅允许必要运维子命令，配置检查只允许 `config --quiet` / `config --services`，禁止裸config、环境导出与任意exec/run；内部受控备份恢复仍可运行必要Docker exec。明确拒绝遗留ENV_FILE模式。备份恢复脚本保留现有外部入口，由统一CLI包装内部实现。

迁移过程由主代理在内存读取现有 `.env.compose`，发布远端配置并回读逐项核对；保留现有密码，移除旧远端部署配置中的本机DATABASE_URL/REDIS_URL（其实际来源是应用TOML），最后删除本地实际部署文件。不操作现有用户栈。CI用独立真实Nacos及隔离配置，不能依赖用户私服凭据。

ROI：复用现有客户端和脚本，只增加一个实际部署引导入口；不新增配置服务或通用插件框架。

## 7. Implementation Guide

This section is a living implementation guide based on current repository analysis. If implementation discovers additional affected files, hidden dependencies, edge cases, or a better path, update this PRD before proceeding.

### 7.1 Core Logic

远端读取失败必须在Docker操作前返回错误。Compose使用其内部PostgreSQL/Redis地址，不使用应用TOML中的宿主回环连接串。引导凭据不得注入业务容器；不记录远端内容或密码。现有部署参数均原样迁移，只有隔离测试的project/端口可在进程环境显式覆盖。

### 7.2 Change Impact Tree

```text
.
├── crates/infrastructure/src/config/ [修改] 【总结】复用Nacos客户端并解析部署参数
├── services/accounts/src/bin/ [新增] 【总结】内存加载配置并执行Docker/运维脚本
├── services/accounts/Cargo.toml [修改] 【总结】声明开发期配置CLI
├── scripts/ [修改] 【总结】正式入口统一读取远端，移除本地生成模式
├── justfile [修改] 【总结】部署与维护命令接入统一CLI
├── docker-compose.yml [修改] 【总结】提示远端配置来源并维持参数契约
├── .github/workflows/ci.yml [修改] 【总结】真实独立Nacos测试配置驱动stack验证
├── deploy/nacos/ [新增/移动] 【总结】无实际密钥的远端部署配置示例，CI临时本地TOML只存非敏感定位元数据
├── .env.example [修改] 【总结】示例只需远端地址和账密，ID集中于项目配置
├── config.toml [修改] 【总结】非敏感[nacos]定位信息按项目命名，应用与部署Data ID独立
└── docs/ [修改] 【总结】配置来源、部署、备份与迁移使用说明
```

### 7.3 Executor Drift Guard

- `rg -n 'env_file|env-file|\.env.compose|ENV_FILE|stack-env' justfile scripts docker-compose.yml .github docs README.md`：仅保留迁移说明、明确拒绝或无实际值的示例，不能保留活动读取/生成入口。
- `rg -n 'compose_data_id|app_data_id|compose-config' crates services scripts justfile config.toml .env.example`：正式入口完整。
- `rg -n 'NACOS_DATA_ID|NACOS_COMPOSE_DATA_ID|NACOS_NAMESPACE_ID|NACOS_GROUP' crates services scripts .env.example`：不保留旧环境选择器，历史迁移说明可注明废弃。

### Risk Classification Register

| 变更 | 风险与原因 | 干预与判别门禁 |
|---|---|---|
| 现有部署密钥迁移与本地删除 | R3：持久来源与数据可用性 | 原样保留、先回读核对、rv-1 |
| 统一CLI及运维流程 | R3：密钥与命令执行边界 | 白名单/字面量/不落盘、rv-2 |
| 文档、示例与CI接线 | R1：可逆入口说明 | rv-3 |

### 7.4 Flow Or Architecture Diagram

```mermaid
flowchart LR
    B[本机引导凭据] --> N[Nacos部署配置]
    N --> C[内存中的配置CLI]
    C --> D[Docker Compose进程]
    D --> S[账号与站点容器]
    C --> O[备份恢复脚本]
```

### 7.5 ER Diagram

No data model changes in this PRD.

### 7.6 Realistic Validation Plan

```yaml
- id: rv-1
  behavior: 将现有部署参数迁移后运行部署命令
  reviewer: human
  real_entry: "现有Nacos配置发布与回读；正式配置CLI"
  expected: "参数和密码保留，远端成为真实来源，本地部署配置文件不存在且不会再生成"
  mock_boundary: "真实本机Nacos，不模拟发布、回读或本地文件状态"
  tier: R3
  test_layer: smoke
  required_for_acceptance: true
  presentation: "tasks/evidence/P1-CONFIG-20261029-160000-nacos-compose-source/human-review-checklist.md；脱敏迁移报告"
  critical_value_source: "从用户现有.env.compose内存读取参数与密码，不重构或生成替代值"
  must_cross: "本地原值 -> 真实Nacos发布 -> 新请求回读 -> 逐项比对 -> 删除本地部署文件 -> 正式CLI读取"
  forbidden_bypasses: "不以样例、旧缓存或单纯上传成功代替比对，不保留.env.compose或临时env文件"
  fresh_state_probe: "新请求读取远端逐项相等，独立检查本地文件不存在并重复配置检查"
  final_tree_evidence: "源码清单哈希与脱敏报告；涉及配置源或迁移逻辑改动后重跑"
  negative_control: "在隔离进程指定不存在的远端Data ID"
  expected_fail: "返回非零且不生成本地env、不调用Docker"
- id: rv-2
  behavior: 在隔离环境启动服务并执行备份恢复，验证故障关闭
  reviewer: verifier
  real_entry: "正式Compose入口与隔离真实Nacos、just stack-smoke"
  expected: "服务实际使用远端值，认证与备份恢复通过；远端读取错误时Docker未执行"
  mock_boundary: "真实Nacos、Compose、PostgreSQL、Redis；认证成功用用户真实Nacos验证，认证拒绝允许仅HTTP状态传输边界stub；错误路径用记录调用的Docker替身证明未执行，报告必须区分真实与stub"
  tier: R3
  test_layer: smoke
  required_for_acceptance: true
  critical_value_source: "隔离Nacos配置产生的实际端口及密钥，真实服务返回Cookie和用户ID"
  must_cross: "Nacos读取 -> CLI进程环境 -> Docker插值 -> 容器 -> 真实API写入 -> 新请求查询 -> 备份恢复"
  forbidden_bypasses: "不使用.env.compose、--env-file真实路径、旧镜像参数或内存认证替身"
  fresh_state_probe: "检查容器有效环境与远端值一致（仅输出相等标记），新HTTP查询用户与恢复状态"
  final_tree_evidence: "最终源码清单、隔离项目与镜像来源、原始脱敏日志"
  negative_control: "隔离配置测试不可达地址、HTTP401认证拒绝传输stub、缺失Data ID、重复/未知键或非法值"
  expected_fail: "均在Docker调用前失败；无本地env副本及密钥输出"
- id: rv-3
  behavior: 构建、标准检查和入口说明保持一致
  reviewer: verifier
  real_entry: "just ci、actionlint、bash语法与入口搜索"
  expected: "检查通过，活动本地部署配置读取与生成路径清除"
  mock_boundary: "不模拟编译与静态搜索"
  tier: R1
  test_layer: integration
  required_for_acceptance: true
```

失败先检查bootstrap地址8848、独立部署Data ID、读取权限与配置键。缺凭据不使用假配置宣称通过。

### 7.7 Prototype Waiver

No frontend impact。仅运维配置源变化，无产品UI原型需求。

### 7.8 Interactive Prototype Change Log

No interactive prototype file changes in this PRD.

### 7.9 External Validation

沿用此前已核对的Nacos3 Client/Admin API，不增加第三方协议；真实服务结果作为最终依据。

## 8. Delivery Dependencies

- Depends on tasks/issues:
  - none
- Gate type: none
- Sequence: via-main
- Notes: 用户明确要求Nacos替代本地部署配置，先完成发布与回读核对再移除本地文件；不重建现有用户部署。

## 9. Acceptance Checklist

### 9.1 人读呈递区（Human Review Surface）

| # | 你要看什么 | 呈递物 | 想自己复核？ |
|---|---|---|---|
| 1 | 部署参数已迁入远端，本地密码文件已移除 | `just prd review P1-CONFIG-20261029-160000-nacos-compose-source.md`；[脱敏迁移记录](../evidence/P1-CONFIG-20261029-160000-nacos-compose-source/migration-report.md) | 查看原值回读一致、文件不存在及正式命令读取结果 |

rv-2真实容器、故障关闭与rv-3构建为verifier-only，不要求人工逐项查看。

### 9.2 Acceptance Evidence Package

证据位于 `tasks/evidence/P1-CONFIG-20261029-160000-nacos-compose-source/`：`migration-report.md` / `real-config-source.log` 证明原值迁移和本地文件删除；`isolated-harness.md` 证明真实容器、备份恢复与负控；`verification-summary.md` / `just-ci.md` 记录构建和静态检查；`source-manifest.json` / `source-provenance.md` 绑定最终工作树；独立判定见 `verifier-report.md`。

### Human-Confirmed

- [ ] 远端成为真实部署配置源、本地部署密码文件已移除，满足“不再本地存”的目标（§2，rv-1）。

### Architecture Acceptance

- [x] Nacos读取实现复用，应用与部署Data ID独立，正式命令全部走新入口（verifier-report.md）。

### Behavior Acceptance

- [x] 原部署值回读一致，未轮换密码；真实配置CLI运行后本地.env.compose不存在（migration-report.md、real-config-source.log）。
- [x] Nacos读取失败在Docker启动前终止，无本地旧配置回退或生成（isolated-harness.md）。

### Documentation Acceptance

- [x] 配置、部署、备份、模板、CI与示例同步，无活动旧读取入口（verification-summary.md、verifier-report.md）。

### Validation Acceptance

- [x] just ci与相关静态检查通过（just-ci.md、verification-summary.md）。
- [x] 真实隔离Nacos/Compose的认证、限流、备份恢复和故障关闭通过（isolated-harness.md；HTTP401拒绝为传输stub）。
- [x] 最终源码清单、敏感值不落盘/不输出证据与独立审查通过（source-manifest.json、verifier-report.md）。

### Delivery Readiness

- [x] 推荐目标完整实现，脱敏报告与可打开的人审清单已交付，无未披露阻塞（human-review-checklist.md/.html、review-ui.log）。

## 10. Functional Requirements

- FR-1: 正式Compose启动/停止/日志命令从Nacos读取部署参数，仅通过进程环境传递，不使用本地env文件。
- FR-2: 原样迁移现有部署参数与密码，真实回读逐项一致后删除本地.env.compose，不生成新密码。
- FR-3: 备份、恢复、smoke复用统一入口，错误配置或Nacos不可用时失败关闭。
- FR-4: 真实隔离集成测试、无用户私服依赖的CI测试配置、文档及无实际密钥示例齐全；env默认只需URL和账密，定位信息集中项目配置。

## 11. Non-Goals

热更新、轮换密钥、数据库结构变更、用户栈自动升级、删除必要Nacos引导凭据均不在范围。

## 12. Risks And Follow-Ups

Nacos成为运维前置依赖，故障时维护命令会失败；必须文档化恢复路径。进程与运行容器仍需持有部署密钥，本次仅消除本地部署配置文件。远端已有配置的历史版本由Nacos保留，不通过本轮操作删除历史审计数据。

## 13. Decision Log

| # | 决策问题 | 选择 | 放弃的方案 | 理由 |
|---|---|---|---|---|
| D-01 | 配置来源 | Nacos唯一持久来源 | 本地.env.compose与远端双轨 | 用户明确要求不再本地存 |
| D-02 | 凭据传递 | 进程env | 临时env文件或eval脚本 | 无落盘且避免shell展开 |
| D-03 | 客户端复用 | 现有Nacos客户端+账号开发CLI | 新建通用crate或复制HTTP逻辑 | 减少依赖与重复实现 |

### Final Reconciliation

- Interpretation: 一致；部署值由Nacos持久保存，env仅3项引导信息，项目定位固定于config.toml。
- Public behavior and contracts: 原有just入口保留，运维改为先读Nacos；未改HTTP契约或用户密码。
- Related PRD status: 既有账号服务PRD已归档，无待处理依赖。
- Requirements and risks: 实际迁移与三项引导通过，隔离栈和just ci通过；Nacos为运维前置依赖，进程/容器仍持有必要密钥。
- Reconciled differences: HTTP401认证拒绝只覆盖传输stub，真实Nacos成功认证另证；测试Nacos初次OOM后降低heap并完整复验。最后修正仅为示例注释，不影响运行证据。

### Change Log

- 2026-10-29：按用户明确要求规划Nacos替换本地Compose配置，保留现有工作树。
- 2026-10-29：独立审查发现裸Compose config会输出秘密，公开CLI收敛为运维白名单及不输出配置的检查模式；未知键错误仅报行号，非敏感字段在Docker前校验类型和CIDR。

- 2026-10-29：用户要求ID按项目固定、env只填URL与账密；改用config.toml非敏感[nacos]定位段，应用/部署共享定位读取，CI只写无秘密的定位TOML。

- 2026-10-29：真实迁移、隔离全栈、just ci和静态检查通过，独立verifier PASS；完成执行侧交付，保留人工验收空框。
