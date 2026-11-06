//! HTTP 装配：状态、路由与共享中间件。
//!
//! 全局只限制 JSON 请求体；上传路由单独提高上限。SSE 路由不挂全局超时层，
//! 否则长连接会被统一超时切断。

use std::sync::Arc;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Request};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use tower_http::trace::TraceLayer;
use tpl_agent_core::identity::{
    AdminIdentityValidator, ServiceTokenValidator, UserIdentityValidator,
};
use tpl_agent_core::ports::ReadinessProbe;
use tpl_agent_core::service::AgentService;

use crate::error::ApiError;
use crate::routes;

/// JSON 请求体上限；提示词与消息正文远低于该值。
const MAX_JSON_BODY_BYTES: usize = 256 * 1024;
/// 上传路由允许的 multipart 包装开销。
const MULTIPART_OVERHEAD_BYTES: usize = 1024 * 1024;

/// API 层共享状态。
#[derive(Clone)]
pub struct AgentApiState {
    /// 用例入口。
    pub service: Arc<AgentService>,
    /// 调用服务凭据校验。
    pub services: Arc<dyn ServiceTokenValidator>,
    /// public 用户身份校验。
    pub users: Arc<dyn UserIdentityValidator>,
    /// 管理员身份校验。
    pub admins: Arc<dyn AdminIdentityValidator>,
    /// 关键依赖就绪探测。
    pub readiness: Arc<dyn ReadinessProbe>,
    /// 单个上传文件上限。
    pub max_upload_bytes: usize,
}

/// 构建完整路由。
pub fn build_router(state: AgentApiState) -> Router {
    let max_upload_limit = state
        .max_upload_bytes
        .saturating_add(MULTIPART_OVERHEAD_BYTES);
    Router::new()
        .merge(routes::health::routes())
        .merge(routes::public::routes(max_upload_limit))
        .merge(routes::admin::routes(max_upload_limit))
        .merge(routes::openapi::routes())
        .layer(DefaultBodyLimit::max(MAX_JSON_BODY_BYTES))
        .layer(middleware::from_fn(normalize_payload_too_large))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// 把框架层请求体上限拒绝统一成 JSON 错误契约。
async fn normalize_payload_too_large(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    if response.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            "请求体超过上限",
        )
        .into_response();
    }
    response
}
