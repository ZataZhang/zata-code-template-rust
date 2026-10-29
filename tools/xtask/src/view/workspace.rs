//! 查看器工作区 API 的模块路由。

mod changes;
mod files;
mod shared;

pub(super) use changes::{changes, diff};
pub(super) use files::{file_payload, info, markdown, open, raw_file, stage, tree};
pub(super) use shared::Payload;
