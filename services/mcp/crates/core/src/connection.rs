//! MCP 连接与授权分配模型。

use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;

use crate::ids::{ConsumerId, GroupId, SubjectId, TenantId};
use crate::secret::{CredentialCiphertext, SecretString};

/// 上游 MCP 连接使用的认证方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthType {
    /// 匿名连接，不携带凭据。
    None,
    /// 在出站请求头中携带 Bearer 凭据。
    Bearer,
}

impl AuthType {
    /// 数据库与 API 使用的稳定字符串。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Bearer => "bearer",
        }
    }

    /// 从稳定字符串解析认证方式。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "bearer" => Some(Self::Bearer),
            _ => None,
        }
    }
}

/// 连接是否参与授权与工具发现。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// 连接正常参与授权。
    Active,
    /// 连接已停用，不再提供工具。
    Disabled,
}

impl ConnectionStatus {
    /// 数据库与 API 使用的稳定字符串。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
        }
    }

    /// 从稳定字符串解析状态。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
}

/// 授权主体类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubjectType {
    /// 用户主体。
    User,
    /// 分组主体。
    Group,
}

impl SubjectType {
    /// 数据库与 API 使用的稳定字符串。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Group => "group",
        }
    }

    /// 从稳定字符串解析主体类型。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "group" => Some(Self::Group),
            _ => None,
        }
    }
}

/// 授权主体：具体用户或分组。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignmentSubject {
    /// 直接授予某个用户。
    User(SubjectId),
    /// 授予某个分组。
    Group(GroupId),
}

impl AssignmentSubject {
    /// 主体类型。
    pub fn subject_type(&self) -> SubjectType {
        match self {
            Self::User(_) => SubjectType::User,
            Self::Group(_) => SubjectType::Group,
        }
    }

    /// 主体标识字符串。
    pub fn subject_id(&self) -> &str {
        match self {
            Self::User(subject_id) => subject_id.as_str(),
            Self::Group(group_id) => group_id.as_str(),
        }
    }
}

/// 消费服务与租户范围；所有持久化查询都必须显式携带。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantScope {
    /// 由服务 token 映射出的消费服务。
    pub consumer_id: ConsumerId,
    /// 签名上下文中声明的租户。
    pub tenant_id: TenantId,
}

/// 已登记的远端 MCP 连接。
#[derive(Debug, Clone)]
pub struct Connection {
    /// 连接标识，同时作为对外工具名前缀。
    pub id: Uuid,
    /// 消费服务与租户范围。
    pub scope: TenantScope,
    /// 租户内唯一的展示名称。
    pub name: String,
    /// 远端 Streamable HTTP endpoint。
    pub endpoint_url: Url,
    /// 认证方式。
    pub auth_type: AuthType,
    /// 认证加密后的凭据；匿名连接为空。
    pub credential: CredentialCiphertext,
    /// 当前状态。
    pub status: ConnectionStatus,
    /// 创建者主体。
    pub created_by: SubjectId,
    /// 创建时间。
    pub created_at: OffsetDateTime,
    /// 最后更新时间。
    pub updated_at: OffsetDateTime,
}

impl Connection {
    /// 凭据是否已配置；API 只暴露这一位信息。
    pub fn credential_configured(&self) -> bool {
        self.credential.is_configured()
    }

    /// 是否处于可授权状态。
    pub fn is_active(&self) -> bool {
        self.status == ConnectionStatus::Active
    }

    /// 凭据认证加密所需的身份范围。
    pub fn credential_scope(&self) -> crate::ports::CredentialScope<'_> {
        crate::ports::CredentialScope {
            consumer_id: self.scope.consumer_id.as_str(),
            tenant_id: self.scope.tenant_id.as_str(),
            connection_id: self.id,
        }
    }
}

/// 更新连接时可变更的字段；`None` 表示保持原值。
#[derive(Debug, Clone, Default)]
pub struct ConnectionChanges {
    /// 新的展示名称。
    pub name: Option<String>,
    /// 新的远端 endpoint，必须先通过出站策略校验。
    pub endpoint_url: Option<Url>,
    /// 新的认证方式；切换为匿名时会清除已有凭据。
    pub auth_type: Option<AuthType>,
    /// 新的连接状态。
    pub status: Option<ConnectionStatus>,
}

/// 创建连接的输入。
#[derive(Debug, Clone)]
pub struct NewConnection {
    /// 展示名称，同一租户内唯一。
    pub name: String,
    /// 远端 Streamable HTTP endpoint。
    pub endpoint_url: Url,
    /// 认证方式。
    pub auth_type: AuthType,
    /// Bearer 凭据明文；匿名连接必须为空。
    pub credential: Option<SecretString>,
}

/// 连接对用户或分组的显式授权。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    /// 被授权的连接。
    pub connection_id: Uuid,
    /// 授权主体。
    pub subject: AssignmentSubject,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_string_round_trips() {
        for auth_type in [AuthType::None, AuthType::Bearer] {
            assert_eq!(AuthType::parse(auth_type.as_str()), Some(auth_type));
        }
        for status in [ConnectionStatus::Active, ConnectionStatus::Disabled] {
            assert_eq!(ConnectionStatus::parse(status.as_str()), Some(status));
        }
        for subject_type in [SubjectType::User, SubjectType::Group] {
            assert_eq!(
                SubjectType::parse(subject_type.as_str()),
                Some(subject_type)
            );
        }
        assert_eq!(AuthType::parse("oauth"), None);
    }
}
