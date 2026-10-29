//! 开发期工具库：PRD 工具链、提交钩子检查与本机文件查看器。二进制入口在 `main.rs`（`cargo xtask ...`）。
//!
//! 做成「库 + 薄 main」，是为了让每个模块都能被单元测试和集成测试直接调用。

pub mod commands;
pub mod git;
pub mod hooks;
pub mod prd;
pub mod template;
pub mod view;
