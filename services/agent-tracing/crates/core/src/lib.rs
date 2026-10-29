//! Agent tracing 核心层：定义语言无关事件契约、序号规则与可重建诊断投影。
//!
//! 本层只依赖标准数据类型与端口，不访问数据库、HTTP 客户端或 accounts 私有 crate。

pub mod event;
pub mod model;
pub mod ports;
pub mod projection;
