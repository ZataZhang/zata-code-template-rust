//! Agent 服务的核心层：组织目录、Runtime/Agent 授权、Session 与 Canonical Run 规则。
//!
//! 本层不依赖 axum、sqlx 或任何具体 Runtime SDK；所有外部能力经 [`ports`] 中的端口注入。
//! 事件只保存 tracing 冻结目录允许的脱敏投影，完整输入留在 Agent 数据库的消息与快照中。

pub mod agent_catalog;
pub mod audit;
pub mod delegation;
pub mod error;
pub mod event_contract;
pub mod hash;
pub mod identity;
pub mod organization;
pub mod ports;
pub mod resources;
pub mod routing;
pub mod run;
pub mod run_events;
pub mod runtime_catalog;
pub mod service;
pub mod session;
pub mod skill_access;
pub mod time_utils;

pub use error::AgentError;
pub use service::AgentService;
