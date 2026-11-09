//! 调用方身份提取器与 MCP endpoint 认证中间件。
//!
//! 每个请求都必须同时携带消费服务 Bearer token 与短时签名用户上下文；
//! 浏览器永远不直接持有服务 token 或签名密钥。

use axum::extract::{FromRequestParts, Request, State};
use axum::http::HeaderMap;
use axum::http::header::{AUTHORIZATION, AsHeaderName};
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::Response;
use tpl_mcp_core::IdentityContext;

use crate::error::ApiError;
use crate::server::McpApiState;

/// 短时签名用户上下文请求头。
pub const USER_CONTEXT_HEADER: &str = "x-user-context";

/// 已验证的调用方身份。
pub struct CallerIdentity(pub IdentityContext);

impl FromRequestParts<McpApiState> for CallerIdentity {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &McpApiState,
    ) -> Result<Self, Self::Rejection> {
        authenticate(state, &parts.headers).await.map(Self)
    }
}

pub(crate) async fn authenticate(
    state: &McpApiState,
    headers: &HeaderMap,
) -> Result<IdentityContext, ApiError> {
    state
        .authenticator
        .authenticate(
            header_str(headers, AUTHORIZATION),
            header_str(headers, USER_CONTEXT_HEADER),
        )
        .await
        .map_err(ApiError::from)
}

fn header_str(headers: &HeaderMap, name: impl AsHeaderName) -> Option<&str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// MCP endpoint 的认证中间件；身份写入请求扩展供 rmcp handler 读取。
pub(crate) async fn require_mcp_identity(
    State(state): State<McpApiState>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let identity = authenticate(&state, request.headers()).await?;
    request.extensions_mut().insert(identity);
    Ok(next.run(request).await)
}
