//! PRD 工具链：契约解析、归档门禁、执行锁、状态看板。
//!
//! PRD 是 `tasks/pending/`（执行中）→ `tasks/archive/`（执行侧已交付）流转的 Markdown 文件，
//! 格式由上游模板仓库 `zata_code_template/skills/prd/` 的 Machine Contract 规定。本模块族实现其中的 Rust 解析与门禁。

pub mod check;
pub mod contract;
pub mod deps;
pub mod filename;
pub mod lock;
pub mod review;
pub mod status;
