//! 调用身份：逐服务机器凭据、accounts public 用户与 admin 管理员身份。
//!
//! public 用户身份只从 accounts `GET /auth/me` 的 Cookie 校验结果得到，管理员只从
//! `GET /admin/auth/me` 得到；两者不合并，也不信任请求体或自定义头里的用户 ID。

use async_trait::async_trait;

use crate::error::AgentError;

/// 经 Bearer 凭据验证的调用服务身份。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServicePrincipal {
    /// 部署配置里的稳定服务标识，例如 `web` 或业务服务名。
    pub service_id: String,
}

/// accounts 公开 HTTP 验证后的 public 用户身份。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserIdentity {
    /// accounts 用户 ID。
    pub user_id: i64,
    /// accounts 邮箱，仅用于日志关联；不写入组织成员关系。
    pub email: String,
}

/// accounts 公开 HTTP 验证后的管理员身份。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdminIdentity {
    /// accounts 管理员 ID，与 public 用户 ID 不混用。
    pub admin_user_id: i64,
    /// 管理员展示名。
    pub display_name: String,
}

/// 校验调用服务 Bearer 凭据的端口。
pub trait ServiceTokenValidator: Send + Sync {
    /// 校验 `Authorization` 头；无效时返回 [`AgentError::ServiceUnauthorized`]。
    fn authenticate_service_token(
        &self,
        authorization_header: Option<&str>,
    ) -> Result<ServicePrincipal, AgentError>;
}

/// 通过 accounts 公开接口校验最终用户 Cookie 的端口。
#[async_trait]
pub trait UserIdentityValidator: Send + Sync {
    /// 校验 public 用户 Cookie；未登录返回 [`AgentError::Unauthenticated`]。
    async fn authenticate_user(
        &self,
        cookie_header: Option<&str>,
    ) -> Result<UserIdentity, AgentError>;
}

/// 通过 accounts 公开接口校验管理员 Cookie 的端口。
#[async_trait]
pub trait AdminIdentityValidator: Send + Sync {
    /// 校验 admin Cookie；未登录返回 [`AgentError::Unauthenticated`]。
    async fn authenticate_admin(
        &self,
        cookie_header: Option<&str>,
    ) -> Result<AdminIdentity, AgentError>;
}
