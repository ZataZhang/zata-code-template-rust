//! HTTP 装配：状态、路由与共享中间件。
//!
//! 全局只限制 JSON 请求体；MCP endpoint 使用 rmcp 自带的请求体上限，
//! 认证中间件只挂在 `/mcp` 路由上，探针与 OpenAPI 保持匿名可读。

use std::sync::Arc;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Request};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use tower_http::trace::TraceLayer;
use tpl_mcp_core::{CallerAuthenticator, McpService, ReadinessProbe};

use crate::error::ApiError;
use crate::mcp_gateway::{self, GatewayHttpConfig};
use crate::{extract, routes};

/// REST JSON 请求体上限；凭据与管理字段远低于该值。
const MAX_JSON_BODY_BYTES: usize = 256 * 1024;

/// API 层共享状态。
#[derive(Clone)]
pub struct McpApiState {
    /// 用例入口。
    pub service: Arc<McpService>,
    /// 调用方服务凭据与签名上下文校验。
    pub authenticator: Arc<dyn CallerAuthenticator>,
    /// 关键依赖就绪探测。
    pub readiness: Arc<dyn ReadinessProbe>,
    /// MCP endpoint 允许的 Host 头。
    pub gateway_allowed_hosts: Vec<String>,
    /// MCP 请求体上限（字节）。
    pub mcp_max_request_body_bytes: usize,
}

/// 构建完整路由。
pub fn build_router(state: McpApiState) -> Router {
    let streamable = mcp_gateway::build_streamable_service(
        state.service.clone(),
        GatewayHttpConfig {
            allowed_hosts: state.gateway_allowed_hosts.clone(),
            max_request_body_bytes: state.mcp_max_request_body_bytes,
        },
    );
    let mcp_routes =
        Router::new()
            .nest_service("/mcp", streamable)
            .route_layer(middleware::from_fn_with_state(
                state.clone(),
                extract::require_mcp_identity,
            ));
    Router::new()
        .merge(routes::health::routes())
        .merge(routes::connections::routes())
        .merge(routes::openapi::routes())
        .merge(mcp_routes)
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
