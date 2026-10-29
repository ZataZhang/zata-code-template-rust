//! 核心编排层（core）。
//!
//! 放置用例、领域规则和**端口**（trait）。这是最内层：
//!
//! - 不依赖任何其他内部 crate（Cargo.toml 里没声明，编译器就不允许 `use`；声明本身由 `services/accounts/tests/architecture.rs` 检查）；
//! - 不依赖具体 SDK（数据库、HTTP 客户端等），只通过端口与外界对话；
//! - 端口的具体实现由账号服务内部的 `tpl-infrastructure` / `tpl-engines` 提供，由 `tpl-accounts` 装配。

pub mod admin;
pub mod auth;
pub mod health;
