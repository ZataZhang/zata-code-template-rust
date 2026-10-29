//! Agent tracing 基础设施层：配置、日志、PostgreSQL、accounts 身份客户端和 OTLP 出口。
//!
//! 本层依赖 tracing core 端口，不访问 accounts 的数据库、Redis 或私有 crate。

pub mod config;
pub mod logging;
pub mod otlp;
pub mod persistence;
pub mod sessions;
