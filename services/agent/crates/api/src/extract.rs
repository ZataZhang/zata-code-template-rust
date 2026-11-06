//! 调用身份提取器：服务 Bearer 凭据 + 经 accounts 校验的 Cookie 身份。
//!
//! public 用户与管理员分别走不同提取器，身份不合并；浏览器永远不持有服务 token，
//! 由前端服务端代理注入。

use axum::extract::FromRequestParts;
use axum::http::header::{AUTHORIZATION, COOKIE};
use axum::http::request::Parts;
use tpl_agent_core::identity::{AdminIdentity, ServicePrincipal, UserIdentity};

use crate::error::ApiError;
use crate::server::AgentApiState;

/// 只有调用服务身份。
pub struct ServiceCall {
    /// 已验证的调用服务。
    pub principal: ServicePrincipal,
}

/// 调用服务 + public 用户身份。
pub struct UserCall {
    /// 已验证的调用服务。
    pub principal: ServicePrincipal,
    /// 已验证的 public 用户。
    pub user: UserIdentity,
}

/// 调用服务 + accounts 管理员身份。
pub struct AdminCall {
    /// 已验证的调用服务。
    pub principal: ServicePrincipal,
    /// 已验证的管理员。
    pub admin: AdminIdentity,
}

fn header_str(
    headers: &axum::http::HeaderMap,
    header_name: axum::http::header::HeaderName,
) -> Option<&str> {
    headers
        .get(header_name)
        .and_then(|header_value| header_value.to_str().ok())
}

fn service_principal(
    state: &AgentApiState,
    headers: &axum::http::HeaderMap,
) -> Result<ServicePrincipal, ApiError> {
    state
        .services
        .authenticate_service_token(header_str(headers, AUTHORIZATION))
        .map_err(ApiError::from)
}

impl FromRequestParts<AgentApiState> for ServiceCall {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AgentApiState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self {
            principal: service_principal(state, &parts.headers)?,
        })
    }
}

impl FromRequestParts<AgentApiState> for UserCall {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AgentApiState,
    ) -> Result<Self, Self::Rejection> {
        let principal = service_principal(state, &parts.headers)?;
        let user = state
            .users
            .authenticate_user(header_str(&parts.headers, COOKIE))
            .await
            .map_err(ApiError::from)?;
        Ok(Self { principal, user })
    }
}

impl FromRequestParts<AgentApiState> for AdminCall {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AgentApiState,
    ) -> Result<Self, Self::Rejection> {
        let principal = service_principal(state, &parts.headers)?;
        let admin = state
            .admins
            .authenticate_admin(header_str(&parts.headers, COOKIE))
            .await
            .map_err(ApiError::from)?;
        Ok(Self { principal, admin })
    }
}
