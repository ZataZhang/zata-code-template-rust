# Architecture Standards

本页是 AI 代理的架构速查版。详细权威说明在 [`docs/architecture/system-design.md`](../architecture/system-design.md)。

## Mandatory Rule

开始任何新的后端功能前，先阅读 `docs/architecture/system-design.md`。

## 当前服务边界

`services/accounts/` 是当前唯一业务服务和 composition root（Cargo package `tpl-accounts`，二进制 `accounts-service`）。`crates/*` 是账号服务私有层，不是供其他业务模块依赖的通用库。后续业务模块若需独立数据和部署，应有自己的服务入口、接口与迁移，不依赖账号服务的领域/存储 crate。

## 账号服务内部层

每一层是一个独立的 Cargo crate，目录在 `crates/` 下：

| 层 | crate（目录） | 职责 |
|---|---|---|
| 接入层 | `tpl-api`（`crates/api`） | HTTP 路由、参数校验、DTO 转换；调用用例，不写业务规则 |
| 核心编排层 | `tpl-core`（`crates/core`） | 用例、领域规则、**端口（trait）**；不依赖任何其他内部 crate、不依赖具体 SDK |
| 平台能力层 | `tpl-engines`（`crates/engines`） | 可插拔的业务能力，实现 core 定义的端口（由派生项目添加） |
| 基础设施层 | `tpl-infrastructure`（`crates/infrastructure`） | 配置、日志、数据库等具体实现，实现 core 定义的端口 |
| 装配层 | `tpl-accounts`（`services/accounts`） | composition root：创建对象、读配置、把实现接到用例上、启动账号服务 |

## Dependency Direction

**依赖向内：外层可以依赖 `tpl-core`，`tpl-core` 不能依赖任何外层。** 依赖关系写在各 crate 的 `Cargo.toml` 里，有两道防线：

1. **Cargo**：没声明的依赖就 `use` 不了；
2. **守卫测试** `services/accounts/tests/architecture.rs`：读取 `cargo metadata`，断言账号服务私有层的依赖都符合允许名单。只靠第 1 道不够——有人把违规依赖写进 `Cargo.toml`，照样能编译通过。

因此不需要 Python 版那种逐文件扫描 import 的脚本，但这一个测试要保留。它失败时修 `Cargo.toml`，不要改测试；只有约定本身变更时才改它的名单，并同步更新本页。新增 crate 时必须同时登记到名单里，否则测试会失败。

```text
tpl-api            → tpl-core
tpl-engines        → tpl-core
tpl-infrastructure → tpl-core
tpl-core           → （无）
tpl-accounts       → 以上全部（账号服务唯一允许依赖全部各层的地方）
```

允许与禁止：

- `tpl-core` 不得依赖 `tpl-api` / `tpl-engines` / `tpl-infrastructure`，也不得在公共签名里出现具体 SDK 类型（如 `sqlx::PgPool`）
- `tpl-api` 只依赖 `tpl-core`；不得直接依赖 `tpl-infrastructure` 或 `tpl-engines`
- `tpl-infrastructure` 不得依赖 `tpl-api` / `tpl-engines`
- `tpl-engines` 不得依赖 `tpl-api`；需要基础设施能力时，通过 core 的端口，由 `tpl-accounts` 注入
- 跨层协作一律经由 `tpl-core` 里的端口（trait）
- 未来业务服务不得依赖 `tpl-api` / `tpl-core` / `tpl-infrastructure` 等账号私有 crate；跨业务复用须提取与账号域无关的契约或平台 crate

> 与 Python 版模板的差异：Python 版规则写着「infrastructure 不得导入 core」，靠的是 Python 的结构化类型（Protocol 鸭子类型）让基础设施不 import 就能满足 core 的接口。Rust 的 trait 必须显式 `impl`，实现方必须依赖定义 trait 的 crate，所以这里采用标准的 Clean Architecture 方向：所有外层依赖 core。

**新增或修改 `Cargo.toml` 的 `[dependencies]` 时，必须核对上面的依赖方向。** 这是评审时最该看的一行。

## Capability Placement

在**现有账号能力内部**新增平台适配（检索、OCR、信息抽取、爬虫……）时：

1. 在 `tpl-core` 里定义端口（trait）；
2. 在 `tpl-engines` 里实现；
3. 在 `tpl-accounts` 的装配代码里接线；
4. 外部请求仍统一经 `tpl-api` 进入。

如果能力属于新的业务边界，应新增独立服务及其数据迁移，不要塞入账号私有 crate。是否拆成服务依据数据所有权、独立发布与运行需求决定，不因 Cargo crate 层级而自动拆分。

## Placement Checklist

新增代码前先判断：

1. 这是入口适配、业务编排、平台能力、基础设施实现，还是装配？
2. 依赖方向是否仍然向内（看 `Cargo.toml`）？
3. 是否在已有 crate 的职责内扩展，而不是塞进最近的文件？
4. 是否需要先在 core 里定义端口，再做具体实现？

## Composition Root

- `services/accounts/` 是账号服务的 composition root，只负责对象创建、配置转换、依赖连接、进程启动与退出
- 业务规则不写在 `tpl-accounts`；账号内部层不得反向依赖服务入口
- 处理器和用例里不要自己 `new` 具体实现，一律由装配层注入（`Arc<dyn Trait>` 或泛型）

## Frontend Boundary

后端的 Cargo workspace 不包含前端。前端是后端分层之外的独立 Web 客户端，通过 HTTP 调用 `tpl-api`，遵循自己目录下的约定：

| 前端 | 目录 | 技术栈 | 状态 |
|---|---|---|---|
| 前台站点 | `frontend-public/` | Next.js 16 + React 19 + Tailwind 4，pnpm | 已迁移：营销页、注册、登录、受保护区域 |
| 账号 UI 包 | `packages/accounts-ui/` | React 19、TypeScript、pnpm | 提供可复用的普通用户注册/登录组件与 HTTP 客户端；宿主配置同源 API base 和文案 |
| 第二宿主示例 | `examples/accounts-host/` | Vite + React | 独立应用，依赖打包后的 `@tpl/accounts-ui` tarball，用于验证模块可以跨应用安装和构建 |
| 管理后台 | `frontend-admin/` | Vite + React | 已迁移：管理员登录、用户搜索与启停；保持 `/admin/*` 独立认证域，不复用普通用户账号 UI |

前端有自己的规范，见 `frontend-public/README.md`（接口契约、部署注意事项）和 `frontend-public/AGENTS.md`（Next.js 版本有破坏性变更，写代码前先读 `node_modules/next/dist/docs/`）。

任何带用户可见界面的功能，PRD 与实现都要把对应前端当作一等公民规划；纯后端任务需显式声明 `No frontend impact`，而不是默默省略前端。
