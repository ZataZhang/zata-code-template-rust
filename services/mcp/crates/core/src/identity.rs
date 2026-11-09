//! 调用方身份：消费服务凭据与短时签名用户上下文。

use crate::connection::{AssignmentSubject, TenantScope};
use crate::ids::{ConsumerId, GroupId, SubjectId, TenantId};

/// 调用方角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerRole {
    /// 普通用户，只能使用获准工具。
    User,
    /// 管理员，可管理当前租户的连接与授权。
    Admin,
}

impl CallerRole {
    /// 从签名声明中解析角色。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }
}

/// 验证通过的调用方身份；由服务凭据与签名上下文共同确定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityContext {
    /// 服务 token 映射出的消费服务。
    pub consumer_id: ConsumerId,
    /// 签名上下文中声明的租户。
    pub tenant_id: TenantId,
    /// 消费服务命名空间下的用户主体。
    pub subject_id: SubjectId,
    /// 用户所属分组。
    pub groups: Vec<GroupId>,
    /// 调用方角色。
    pub role: CallerRole,
}

impl IdentityContext {
    /// 当前主体是否持有管理权限。
    pub fn is_admin(&self) -> bool {
        self.role == CallerRole::Admin
    }

    /// 本次请求的消费服务与租户范围。
    pub fn scope(&self) -> TenantScope {
        TenantScope {
            consumer_id: self.consumer_id.clone(),
            tenant_id: self.tenant_id.clone(),
        }
    }

    /// 可能命中连接授权的全部主体（用户本身与所属分组）。
    pub fn assignment_subjects(&self) -> Vec<AssignmentSubject> {
        let mut subjects = Vec::with_capacity(self.groups.len() + 1);
        subjects.push(AssignmentSubject::User(self.subject_id.clone()));
        subjects.extend(self.groups.iter().cloned().map(AssignmentSubject::Group));
        subjects
    }
}

/// 调用方认证失败。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    /// 请求未携带消费服务凭据。
    #[error("缺少消费服务凭据")]
    MissingServiceToken,
    /// 消费服务凭据无效。
    #[error("消费服务凭据无效")]
    InvalidServiceToken,
    /// 请求未携带短时用户上下文。
    #[error("缺少用户上下文")]
    MissingUserContext,
    /// 用户上下文无法解析或签名无效。
    #[error("用户上下文签名无效")]
    InvalidUserContext,
    /// 用户上下文已过期或尚未生效。
    #[error("用户上下文已过期")]
    ExpiredUserContext,
    /// 用户上下文声明的消费服务与 token 不一致。
    #[error("用户上下文与消费服务不匹配")]
    ScopeMismatch,
    /// 服务端消费方配置不可用。
    #[error("消费服务配置无效")]
    Configuration,
}

/// 验证 HTTP 请求上的消费服务身份与短时签名用户上下文。
#[async_trait::async_trait]
pub trait CallerAuthenticator: Send + Sync {
    /// 验证请求头并返回调用方身份；任何缺失或无效字段都必须失败。
    async fn authenticate(
        &self,
        authorization: Option<&str>,
        user_context: Option<&str>,
    ) -> Result<IdentityContext, AuthError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> IdentityContext {
        IdentityContext {
            consumer_id: ConsumerId::new("consumer-a").expect("测试消费服务有效"),
            tenant_id: TenantId::new("tenant-a").expect("测试租户有效"),
            subject_id: SubjectId::new("user-1").expect("测试用户有效"),
            groups: vec![GroupId::new("group-1").expect("测试分组有效")],
            role: CallerRole::User,
        }
    }

    #[test]
    fn assignment_subjects_include_user_and_groups() {
        let subjects = identity().assignment_subjects();
        assert_eq!(subjects.len(), 2);
        assert_eq!(
            subjects[0].subject_type(),
            crate::connection::SubjectType::User
        );
        assert_eq!(
            subjects[1].subject_type(),
            crate::connection::SubjectType::Group
        );
    }

    #[test]
    fn role_parsing_is_closed() {
        assert_eq!(CallerRole::parse("admin"), Some(CallerRole::Admin));
        assert_eq!(CallerRole::parse("user"), Some(CallerRole::User));
        assert_eq!(CallerRole::parse("root"), None);
    }
}
