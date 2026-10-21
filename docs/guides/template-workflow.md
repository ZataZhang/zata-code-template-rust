# Rust 模板的复制与同步

模板来源仓库：[`ZataZhang/zata-code-template-rust`](https://github.com/ZataZhang/zata-code-template-rust)。复制和同步由 `cargo xtask` 实现，统一通过 `just` 调用。

## 从模板创建项目

```bash
just copy ../my-service
```

目标目录必须不存在或为空。对已有目录运行时，显式传 `--force` 才会覆盖同名文件；其他文件不会删除。复制会跳过 `.git`、构建产物、前端依赖、编辑器与 AI 工具本地状态、日志、数据库文件、`.env` 密钥、本仓库的任务记录与执行证据，也不会跟随目标中的符号链接写到目录外；`.env.example` 等示例配置会保留。复制不会初始化 Git 或创建提交，新项目的 Git 历史由项目维护者自行建立。

复制会保留模板的 `Cargo.toml`、`README.md` 与业务示例。派生项目确定名称和范围后，应按需调整 crate 前缀、配置、服务和 README；`docs/zata_rust_template-plan.md` 记录了 `tpl-` 前缀的重命名步骤。

## 从上游同步通用规则

项目根目录的 `.template-sync` 是唯一同步清单。每行是一个仓库相对文件或目录；路径只能加入适合被上游覆盖的通用文件。当前清单包括 AI 入口、AI 规范、PRD/想法工作流、对应 skills 与 `xtask`，不包括业务 crate、前端、部署配置或项目专属 `justfile`。

```bash
# 默认从 Rust 模板仓库读取，逐文件查看差异并确认
just sync-template

# 只列出新增、变更和上游已删除但本地保留的文件
just sync-template --list

# 明确同意覆盖清单内所有新增或变更文件
just sync-template --apply

# 也可从本地模板目录同步
just sync-template --upstream /path/to/zata-code-template-rust
```

交互模式每个文件都展示 diff 并单独询问；非交互终端默认只报告差异、不写入。`--apply` 只覆盖清单内的新增或变更文件，不删除本地其他文件。若某个上游文件消失，会报告并保留本地版本，删除需项目维护者自行判断。

## 模板维护

修改通用规范或工作流时更新 Rust 模板仓库里的权威文件，并按需增加 `.template-sync` 路径。业务 crate、配置和服务编排应留在项目自身，避免同步时覆盖派生项目的实现。
