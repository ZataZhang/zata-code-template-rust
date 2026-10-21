//! 基础设施层（infrastructure）。
//!
//! 放置配置、日志、数据库、Redis、密码哈希等具体实现，对接外部系统。
//! 依赖 `tpl-core` 是为了实现它定义的端口（trait）；不得依赖 `tpl-api`、`tpl-engines`。

pub mod config;
pub mod logging;
pub mod password;
pub mod persistence;
pub mod redis_attempts;
pub mod redis_session;
