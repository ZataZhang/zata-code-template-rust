//! Agent tracing HTTP 接入层：producer 事件接收、管理员查询与健康探针。
//!
//! 本层只依赖 tracing core 契约和端口；数据库与 accounts HTTP 客户端由装配层注入。

mod admin;
mod error;
mod health;
mod ingest;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::routing::{get, post};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use tpl_agent_tracing_core::ports::{AdminSessionValidator, OtlpExporter, TraceRepository};

const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// 所有 HTTP 路由共享的 tracing 应用状态。
#[derive(Clone)]
pub struct AppState {
    /// Agent Run/Event 的持久化端口。
    pub repository: Arc<dyn TraceRepository>,
    /// accounts 管理员 Cookie 校验端口。
    pub admin_sessions: Arc<dyn AdminSessionValidator>,
    /// 不阻塞接收确认的可选 OTLP 出口。
    pub otlp_exporter: Arc<dyn OtlpExporter>,
}

/// 构造 tracing API 路由与请求日志中间件。
pub fn build_router(app_state: AppState) -> Router {
    Router::new()
        .route("/live", get(health::live))
        .route("/ready", get(health::ready))
        .route(
            "/api/v1/ingest/runs/{run_id}/events",
            post(ingest::append_events),
        )
        .route("/admin/agent-traces", get(admin::list_runs))
        .route("/admin/agent-traces/threads", get(admin::list_threads))
        .route(
            "/admin/agent-traces/threads/{thread_id}",
            get(admin::get_thread),
        )
        .route(
            "/admin/agent-traces/{run_id}/export",
            get(admin::export_run),
        )
        .route("/admin/agent-traces/{run_id}", get(admin::get_run))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            HTTP_REQUEST_TIMEOUT,
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(app_state)
}
