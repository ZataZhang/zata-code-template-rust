//! 管理员轨迹读取、线程账本和原始事件导出路由。

mod export;
mod runs;
mod threads;

use axum::http::HeaderMap;
use tpl_agent_tracing_core::ports::AdminIdentity;

use crate::AppState;
use crate::error::ApiError;

pub(crate) use export::export_run;
pub(crate) use runs::{get_run, list_runs};
pub(crate) use threads::{get_thread, list_threads};

async fn require_admin(
    app_state: &AppState,
    headers: &HeaderMap,
) -> Result<AdminIdentity, ApiError> {
    let cookie_header = headers
        .get(axum::http::header::COOKIE)
        .and_then(|header_value| header_value.to_str().ok());
    app_state
        .admin_sessions
        .authenticate(cookie_header)
        .await
        .map_err(ApiError::from_admin_auth)
}
