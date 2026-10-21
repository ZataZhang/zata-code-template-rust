# Architecture Standards

本页是 AI 代理的架构速查版。详细权威说明在 [`docs/architecture/system-design.md`](../architecture/system-design.md)。

## Mandatory Rule

开始任何新的后端功能前，先阅读 `docs/architecture/system-design.md`。

## Layers

每一层是一个独立的 Cargo crate，目录在 `crates/` 下：

| 层 | crate（目录） | 职责 |
|---|---|---|
| 接入层 | `tpl-api`（`crates/api`） | HTTP 路由、参数校验、DTO 转换；调用用例，不写业务规则 |
| 核心编排层 | `tpl-core`（`crates/core`） | 用例、领域规则、**端口（trait）**；不依赖任何其他内部 crate、不依赖具体 SDK |
| 平台能力层 | `tpl-engines`（`crates/engines`） | 可插拔的业务能力，实现 core 定义的端口（由派生项目添加） |
| 基础设施层 | `tpl-infrastructure`（`crates/infrastructure`） | 配置、日志、数据库等具体实现，实现 core 定义的端口 |
| 装配层 | `tpl-app`（`crates/app`） | composition root：创建对象、读配置、把实现接到用例上、启动进程 |

## Dependency Direction

**依赖向内：外层可以依赖 `tpl-core`，`tpl-core` 不能依赖任何外层。** 依赖关系写在各 crate 的 `Cargo.toml` 里，有两道防线：

1. **Cargo**：没声明的依赖就 `use` 不了；
2. **守卫测试** `crates/app/tests/architecture.rs`：读取 `cargo metadata`，断言每一层声明的内部依赖都在允许名单里。只靠第 1 道不够——有人把违规依赖写进 `Cargo.toml`，照样能编译通过。

因此不需要 Python 版那种逐文件扫描 import 的脚本，但这一个测试要保留。它失败时修 `Cargo.toml`，不要改测试；只有约定本身变更时才改它的名单，并同步更新本页。新增 crate 时必须同时登记到名单里，否则测试会失败。

```text
tpl-api            → tpl-core
tpl-engines        → tpl-core
tpl-infrastructure → tpl-core
tpl-core           → （无）
tpl-app            → 以上全部（唯一允许依赖全部各层的地方）
```

允许与禁止：

- `tpl-core` 不得依赖 `tpl-api` / `tpl-engines` / `tpl-infrastructure`，也不得在公共签名里出现具体 SDK 类型（如 `sqlx::PgPool`）
- `tpl-api` 只依赖 `tpl-core`；不得直接依赖 `tpl-infrastructure` 或 `tpl-engines`
- `tpl-infrastructure` 不得依赖 `tpl-api` / `tpl-engines`
- `tpl-engines` 不得依赖 `tpl-api`；需要基础设施能力时，通过 core 的端口，由 `tpl-app` 注入
- 跨层协作一律经由 `tpl-core` 里的端口（trait）

> 与 Python 版模板的差异：Python 版规则写着「infrastructure 不得导入 core」，靠的是 Python 的结构化类型（Protocol 鸭子类型）让基础设施不 import 就能满足 core 的接口。Rust 的 trait 必须显式 `impl`，实现方必须依赖定义 trait 的 crate，所以这里采用标准的 Clean Architecture 方向：所有外层依赖 core。

**新增或修改 `Cargo.toml` 的 `[dependencies]` 时，必须核对上面的依赖方向。** 这是评审时最该看的一行。

## Capability Placement

新增面向业务的具体能力（检索、OCR、信息抽取、爬虫……）时：

1. 在 `tpl-core` 里定义端口（trait）；
2. 在 `tpl-engines` 里实现；
3. 在 `tpl-app` 的装配代码里接线；
4. 外部请求仍统一经 `tpl-api` 进入。

除非详细架构文档明确批准，不要为单一业务能力新增独立 HTTP 服务、独立端口或旁路的用户级 API。

## Placement Checklist

新增代码前先判断：

1. 这是入口适配、业务编排、平台能力、基础设施实现，还是装配？
2. 依赖方向是否仍然向内（看 `Cargo.toml`）？
3. 是否在已有 crate 的职责内扩展，而不是塞进最近的文件？
4. 是否需要先在 core 里定义端口，再做具体实现？

## Composition Root

- `crates/app/` 是唯一的 composition root，只负责对象创建、配置转换、依赖连接、进程启动与退出
- 业务规则不写在 `tpl-app`；其他 crate 不得依赖 `tpl-app`（它是 bin crate，本来也无法被依赖）
- 处理器和用例里不要自己 `new` 具体实现，一律由装配层注入（`Arc<dyn Trait>` 或泛型）

## Frontend Boundary

后端的 Cargo workspace 不包含前端。前端是后端分层之外的独立 Web 客户端，通过 HTTP 调用 `tpl-api`，遵循自己目录下的约定：

| 前端 | 目录 | 技术栈 | 状态 |
|---|---|---|---|
| 前台站点 | `frontend-public/` | Next.js 16 + React 19 + Tailwind 4，pnpm | 已迁移：营销页、注册、登录、受保护区域 |
| 管理后台 | （无） | Vite + React | **未迁移**：依赖管理员账号与 `/admin/*` 接口，见 `docs/migration-status.md` |

前端有自己的规范，见 `frontend-public/README.md`（接口契约、部署注意事项）和 `frontend-public/AGENTS.md`（Next.js 版本有破坏性变更，写代码前先读 `node_modules/next/dist/docs/`）。

任何带用户可见界面的功能，PRD 与实现都要把对应前端当作一等公民规划；纯后端任务需显式声明 `No frontend impact`，而不是默默省略前端。
