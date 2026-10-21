# Documentation Standards

## Docs Are Part Of The Product

`docs/`、`README.md` 以及源码里的 `///` 文档注释是代码库的一部分。文档错误按代码错误对待。

## Synchronization Rules

出现以下变化时，必须同步更新文档：

- 业务逻辑变化
- 配置项变化（同时更新 `config.toml` 的注释与 `.env.example`）
- 公共函数、类型、trait 签名变化（更新 `///`）
- 新增 crate、模块、流程或开发规范
- 依赖方向变化（更新 `docs/architecture/system-design.md` 与 `docs/ai-standards/architecture.md`）

## API 文档

不要在 Markdown 里手工复制 Rust 函数说明。API 说明写在 `///` 里，用 `cargo doc` 生成：

```bash
just doc          # 以 -D warnings 构建文档，坏链接与失效引用会报错
cargo doc --workspace --no-deps --open
```

## 第三方行为的断言

见 [Comments And Docs](comments-docs.md) 的同名小节。

## 新增长期文档页

1. 创建对应的 Markdown 文件（放在 `docs/` 对应子目录）
2. 在 `docs/ai-standards/index.md` 或相关索引里加入链接，保证可发现
3. 若是 AI 规范页，同步更新 `AGENTS.md` 的路由表

## Markdown Encoding

所有 Markdown 文件使用 UTF-8 编码。
