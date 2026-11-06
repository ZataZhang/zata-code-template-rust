//! Organization、部门、显式成员关系与管理员 scope。
//!
//! 部门没有父子关系，因此授权不沿层级继承；一个用户在一个组织内最多属于一个部门。
//! public 成员关系与 accounts 管理员 scope 是两套独立授权，不互相推导。

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::AgentError;
use crate::identity::AdminIdentity;

/// 组织（租户）目录项。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Organization {
    /// 组织 UUID。
    pub id: Uuid,
    /// 全局唯一短名。
    pub slug: String,
    /// 展示名称。
    pub display_name: String,
    /// 创建时间。
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// 组织内部门。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrganizationUnit {
    /// 部门 UUID。
    pub id: Uuid,
    /// 所属组织。
    pub organization_id: Uuid,
    /// 组织内唯一名称。
    pub name: String,
    /// 创建时间。
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// 显式 public 用户成员关系。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrganizationMembership {
    /// 所属组织。
    pub organization_id: Uuid,
    /// accounts 用户 ID。
    pub user_id: i64,
    /// 可选显式部门。
    pub unit_id: Option<Uuid>,
    /// 组织内展示名称。
    pub display_name: Option<String>,
    /// 建立时间。
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// accounts 管理员对单一组织的显式管理 scope。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminScope {
    /// 被授权管理的组织。
    pub organization_id: Uuid,
    /// accounts 管理员 ID。
    pub admin_user_id: i64,
    /// 授予者管理员 ID。
    pub granted_by_admin_id: i64,
    /// 授权时间。
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// 平台管理员名单；只用于创建组织和授予组织 scope，不自动获得组织内权限。
#[derive(Clone, Debug, Default)]
pub struct PlatformAdminPolicy {
    admin_user_ids: Vec<i64>,
}

impl PlatformAdminPolicy {
    /// 由部署配置中的 accounts 管理员 ID 构造。
    pub fn new(admin_user_ids: Vec<i64>) -> Self {
        Self { admin_user_ids }
    }

    /// 判断是否为平台管理员。
    pub fn is_platform_admin(&self, admin: &AdminIdentity) -> bool {
        self.admin_user_ids.contains(&admin.admin_user_id)
    }

    /// 要求平台管理员身份。
    ///
    /// # Errors
    ///
    /// 非平台管理员返回 `platform_admin_required`。
    pub fn require_platform_admin(&self, admin: &AdminIdentity) -> Result<(), AgentError> {
        if self.is_platform_admin(admin) {
            Ok(())
        } else {
            Err(AgentError::Forbidden("platform_admin_required"))
        }
    }
}

/// 校验 slug 并返回规范化值。
///
/// # Errors
///
/// slug 不符合 `^[a-z0-9][a-z0-9-]{1,62}$` 时返回错误。
pub fn validate_slug(slug: &str) -> Result<(), AgentError> {
    let bytes = slug.as_bytes();
    let length_is_valid = (2..=63).contains(&bytes.len());
    let first_is_valid = bytes
        .first()
        .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
    let is_valid = length_is_valid
        && first_is_valid
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-');
    if is_valid {
        Ok(())
    } else {
        Err(AgentError::InvalidRequest(
            "组织 slug 必须是 2-63 位小写字母、数字或连字符".to_owned(),
        ))
    }
}

/// 校验组织展示名称。
///
/// # Errors
///
/// 空名称或超过 200 字符时返回错误。
pub fn validate_display_name(display_name: &str) -> Result<(), AgentError> {
    validate_text(display_name, 1, 200, "展示名称")
}

/// 校验一般文本字段的字符长度。
///
/// # Errors
///
/// 长度越界或含控制字符时返回错误。
pub fn validate_text(
    value: &str,
    minimum_chars: usize,
    maximum_chars: usize,
    field_name: &str,
) -> Result<(), AgentError> {
    let length = value.chars().count();
    if length < minimum_chars || length > maximum_chars || value.chars().any(char::is_control) {
        return Err(AgentError::InvalidRequest(format!(
            "{field_name}长度必须在 {minimum_chars}-{maximum_chars} 个字符之间且不含控制字符"
        )));
    }
    Ok(())
}

/// 要求管理员拥有组织的显式 scope。
///
/// # Errors
///
/// 平台管理员身份不会自动通过；没有 scope 时返回 `organization_scope_denied`。
pub fn require_admin_scope(
    admin: &AdminIdentity,
    organization_id: Uuid,
    scopes: &[AdminScope],
) -> Result<(), AgentError> {
    if scopes.iter().any(|scope| {
        scope.organization_id == organization_id && scope.admin_user_id == admin.admin_user_id
    }) {
        Ok(())
    } else {
        Err(AgentError::Forbidden("organization_scope_denied"))
    }
}

/// 找出用户在组织内的显式成员关系。
///
/// # Errors
///
/// 用户不是组织成员时返回 `organization_membership_required`。
pub fn require_membership(
    memberships: &[OrganizationMembership],
    organization_id: Uuid,
    user_id: i64,
) -> Result<&OrganizationMembership, AgentError> {
    memberships
        .iter()
        .find(|membership| {
            membership.organization_id == organization_id && membership.user_id == user_id
        })
        .ok_or(AgentError::Forbidden("organization_membership_required"))
}

/// 校验部门属于组织且名称有效。
///
/// # Errors
///
/// 部门不存在、跨组织或名称不合法时返回错误。
pub fn validate_unit_assignment(
    units: &[OrganizationUnit],
    organization_id: Uuid,
    unit_id: Option<Uuid>,
) -> Result<(), AgentError> {
    let Some(unit_id) = unit_id else {
        return Ok(());
    };
    if units
        .iter()
        .any(|unit| unit.organization_id == organization_id && unit.id == unit_id)
    {
        Ok(())
    } else {
        Err(AgentError::InvalidRequest("部门不属于当前组织".to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::AdminIdentity;

    fn admin(admin_user_id: i64) -> AdminIdentity {
        AdminIdentity {
            admin_user_id,
            display_name: "tester".to_owned(),
        }
    }

    #[test]
    fn platform_admins_do_not_gain_org_scope_automatically() {
        let policy = PlatformAdminPolicy::new(vec![7]);
        let scopes = Vec::new();
        let organization_id = Uuid::new_v4();
        assert!(policy.is_platform_admin(&admin(7)));
        assert_eq!(
            require_admin_scope(&admin(7), organization_id, &scopes),
            Err(AgentError::Forbidden("organization_scope_denied"))
        );
    }

    #[test]
    fn membership_is_required_and_units_do_not_inherit() {
        let organization_id = Uuid::new_v4();
        let unit_id = Uuid::new_v4();
        let memberships = vec![OrganizationMembership {
            organization_id,
            user_id: 11,
            unit_id: Some(unit_id),
            display_name: None,
            created_at: crate::time_utils::now(),
        }];
        assert!(require_membership(&memberships, organization_id, 11).is_ok());
        assert_eq!(
            require_membership(&memberships, organization_id, 12),
            Err(AgentError::Forbidden("organization_membership_required"))
        );
        let units = vec![OrganizationUnit {
            id: unit_id,
            organization_id,
            name: "engineering".to_owned(),
            created_at: crate::time_utils::now(),
        }];
        assert!(validate_unit_assignment(&units, organization_id, Some(unit_id)).is_ok());
        assert!(validate_unit_assignment(&units, Uuid::new_v4(), Some(unit_id)).is_err());
    }

    #[test]
    fn slug_validation_rejects_uppercase_and_traversal() {
        assert!(validate_slug("acme-01").is_ok());
        assert!(validate_slug("Acme").is_err());
        assert!(validate_slug("../acme").is_err());
        assert!(validate_slug("a").is_err());
    }
}
