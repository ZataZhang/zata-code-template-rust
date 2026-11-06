//! Business Agent descriptor、访问规则与 runnable 判定。
//!
//! 组织 membership 是前提；organization/unit/user allow 取并集，user deny 再收窄。
//! 部门不沿层级继承；个人 deny 覆盖任何 allow。

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::AgentError;
use crate::hash::sha256_json;
use crate::organization::{OrganizationMembership, validate_text};
use crate::runtime_catalog::{RuntimeConnection, RuntimeStatus, ensure_runtime_ready};

/// Agent 业务定义；Run 创建时冻结其快照。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BusinessAgent {
    /// Agent UUID。
    pub id: Uuid,
    /// 所属组织。
    pub organization_id: Uuid,
    /// 创建时绑定的 Runtime。
    pub runtime_connection_id: Uuid,
    /// 组织内稳定名称。
    pub name: String,
    /// 展示名称。
    pub display_name: String,
    /// 能力描述。
    pub description: String,
    /// 基础提示词。
    pub prompt: String,
    /// 负责管理员 ID。
    pub owner_admin_id: i64,
    /// 是否允许创建新 Run。
    pub enabled: bool,
    /// descriptor 与 Runtime binding 的摘要。
    pub descriptor_checksum: String,
    /// 创建时间。
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// 最近更新时间。
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl BusinessAgent {
    /// 根据当前字段计算 descriptor checksum。
    ///
    /// # Errors
    ///
    /// 序列化失败时返回内部错误。
    pub fn compute_checksum(&self) -> Result<String, AgentError> {
        #[derive(Serialize)]
        struct Descriptor<'a> {
            name: &'a str,
            display_name: &'a str,
            description: &'a str,
            prompt: &'a str,
            runtime_connection_id: Uuid,
        }
        sha256_json(&Descriptor {
            name: &self.name,
            display_name: &self.display_name,
            description: &self.description,
            prompt: &self.prompt,
            runtime_connection_id: self.runtime_connection_id,
        })
    }
}

/// 创建 Agent 的输入；Runtime 在此刻绑定，之后不可更换。
#[derive(Clone, Debug)]
pub struct NewAgent {
    /// 所属组织。
    pub organization_id: Uuid,
    /// 选定的 Runtime。
    pub runtime_connection_id: Uuid,
    /// 组织内名称。
    pub name: String,
    /// 展示名称。
    pub display_name: String,
    /// 描述。
    pub description: String,
    /// 基础提示词。
    pub prompt: String,
    /// 负责管理员 ID。
    pub owner_admin_id: i64,
}

impl NewAgent {
    /// 校验 Agent 字段。
    ///
    /// # Errors
    ///
    /// 名称/提示词越界或含控制字符时返回错误。
    pub fn validate(&self) -> Result<(), AgentError> {
        validate_text(&self.name, 1, 128, "Agent 名称")?;
        validate_text(&self.display_name, 1, 200, "Agent 展示名称")?;
        if self.description.chars().count() > 4000 || self.description.chars().any(char::is_control)
        {
            return Err(AgentError::InvalidRequest(
                "Agent 描述不能超过 4000 字符或包含控制字符".to_owned(),
            ));
        }
        validate_text(&self.prompt, 1, 20000, "Agent 提示词")
    }
}

/// 允许更新的 Agent 字段；不含 Runtime binding。
#[derive(Clone, Debug)]
pub struct AgentUpdate {
    /// 展示名称。
    pub display_name: String,
    /// 描述。
    pub description: String,
    /// 基础提示词。
    pub prompt: String,
    /// 是否启用。
    pub enabled: bool,
}

impl AgentUpdate {
    /// 校验可变字段。
    ///
    /// # Errors
    ///
    /// 展示名/提示词越界、描述过长或含控制字符时返回错误。
    pub fn validate(&self) -> Result<(), AgentError> {
        validate_text(&self.display_name, 1, 200, "Agent 展示名称")?;
        if self.description.chars().count() > 4000 || self.description.chars().any(char::is_control)
        {
            return Err(AgentError::InvalidRequest(
                "Agent 描述不能超过 4000 字符或包含控制字符".to_owned(),
            ));
        }
        validate_text(&self.prompt, 1, 20000, "Agent 提示词")
    }
}

/// 访问规则的允许或拒绝效果。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessEffect {
    /// 扩大可见范围。
    Allow,
    /// 收窄到个人；只允许 user。
    Deny,
}

impl AccessEffect {
    /// 数据库字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }

    /// 从数据库字符串恢复。
    pub fn from_database(value: &str) -> Option<Self> {
        match value {
            "allow" => Some(Self::Allow),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }
}

/// 规则主体。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum AccessSubject {
    /// 组织全体显式成员。
    Organization(Uuid),
    /// 某一显式部门成员。
    Unit(Uuid),
    /// 单个 public 用户。
    User(i64),
}

impl AccessSubject {
    /// 数据库主体类型。
    pub fn kind_str(self) -> &'static str {
        match self {
            Self::Organization(_) => "organization",
            Self::Unit(_) => "unit",
            Self::User(_) => "user",
        }
    }

    /// 数据库主体标识。
    pub fn subject_id(self) -> String {
        match self {
            Self::Organization(id) | Self::Unit(id) => id.to_string(),
            Self::User(user_id) => user_id.to_string(),
        }
    }
}

/// 一条 Agent 访问规则。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAccessRule {
    /// 规则 UUID。
    pub id: Uuid,
    /// 所属 Agent。
    pub agent_id: Uuid,
    /// allow/deny。
    pub effect: AccessEffect,
    /// 规则主体。
    pub subject: AccessSubject,
    /// 创建时间。
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// 规则替换输入。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessRuleInput {
    /// allow/deny。
    pub effect: AccessEffect,
    /// 规则主体。
    pub subject: AccessSubject,
}

/// 授权通过时记录的依据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizationBasis {
    /// 命中的 allow 规则类别。
    pub matched_subject: String,
    /// 本次授权所依据的规则集合摘要。
    pub policy_checksum: String,
}

/// 校验规则替换请求。
///
/// # Errors
///
/// 规则过多、deny 非个人或 organization 主体与 Agent 组织不一致时返回错误。
pub fn validate_access_rules(
    agent: &BusinessAgent,
    rules: &[AccessRuleInput],
) -> Result<(), AgentError> {
    if rules.len() > 100 {
        return Err(AgentError::InvalidRequest(
            "单个 Agent 最多 100 条访问规则".to_owned(),
        ));
    }
    for rule in rules {
        match (rule.effect, rule.subject) {
            (AccessEffect::Deny, AccessSubject::User(user_id)) if user_id > 0 => {}
            (AccessEffect::Deny, _) => {
                return Err(AgentError::InvalidRequest(
                    "deny 规则只能针对单个用户".to_owned(),
                ));
            }
            (AccessEffect::Allow, AccessSubject::Organization(organization_id))
                if organization_id == agent.organization_id => {}
            (AccessEffect::Allow, AccessSubject::Organization(_)) => {
                return Err(AgentError::InvalidRequest(
                    "organization allow 必须指向 Agent 所属组织".to_owned(),
                ));
            }
            (AccessEffect::Allow, AccessSubject::User(user_id)) if user_id > 0 => {}
            (AccessEffect::Allow, AccessSubject::User(_)) => {
                return Err(AgentError::InvalidRequest(
                    "用户 ID 必须为正整数".to_owned(),
                ));
            }
            (AccessEffect::Allow, AccessSubject::Unit(_)) => {}
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for rule in rules {
        let key = (
            rule.effect.as_str(),
            rule.subject.kind_str(),
            rule.subject.subject_id(),
        );
        if !seen.insert(key) {
            return Err(AgentError::InvalidRequest("访问规则包含重复项".to_owned()));
        }
    }
    Ok(())
}

/// 计算规则集合的稳定摘要。
///
/// # Errors
///
/// 序列化失败时返回内部错误。
pub fn access_policy_checksum(rules: &[AgentAccessRule]) -> Result<String, AgentError> {
    let mut canonical_rules: Vec<_> = rules
        .iter()
        .map(|rule| {
            (
                rule.effect.as_str(),
                rule.subject.kind_str(),
                rule.subject.subject_id(),
            )
        })
        .collect();
    canonical_rules.sort();
    sha256_json(&canonical_rules)
}

/// 判断 Agent 对用户是否 runnable，并返回授权依据。
///
/// # Errors
///
/// 非组织成员、Agent 停用、Runtime 不可用或没有命中 allow / 命中 deny 时返回稳定错误。
pub fn authorize_agent(
    agent: &BusinessAgent,
    runtime: &RuntimeConnection,
    rules: &[AgentAccessRule],
    membership: Option<&OrganizationMembership>,
    user_id: i64,
) -> Result<AuthorizationBasis, AgentError> {
    let membership = membership.ok_or(AgentError::Forbidden("organization_membership_required"))?;
    if membership.organization_id != agent.organization_id
        || membership.user_id != user_id
        || runtime.id != agent.runtime_connection_id
    {
        return Err(AgentError::Forbidden("agent_not_authorized"));
    }
    if !agent.enabled {
        return Err(AgentError::AgentNotRunnable);
    }
    ensure_runtime_ready(runtime)?;

    let denied = rules.iter().any(|rule| {
        rule.effect == AccessEffect::Deny && rule.subject == AccessSubject::User(user_id)
    });
    if denied {
        return Err(AgentError::Forbidden("agent_access_denied"));
    }

    let mut matched_subject = None;
    for rule in rules {
        if rule.effect != AccessEffect::Allow {
            continue;
        }
        let matched = match rule.subject {
            AccessSubject::Organization(organization_id) => {
                organization_id == agent.organization_id
            }
            AccessSubject::Unit(unit_id) => membership.unit_id == Some(unit_id),
            AccessSubject::User(rule_user_id) => rule_user_id == user_id,
        };
        if matched {
            matched_subject = Some(rule.subject);
            break;
        }
    }
    let Some(matched_subject) = matched_subject else {
        return Err(AgentError::Forbidden("agent_access_denied"));
    };

    Ok(AuthorizationBasis {
        matched_subject: match matched_subject {
            AccessSubject::Organization(_) => "organization".to_owned(),
            AccessSubject::Unit(unit_id) => format!("unit:{unit_id}"),
            AccessSubject::User(rule_user_id) => format!("user:{rule_user_id}"),
        },
        policy_checksum: access_policy_checksum(rules)?,
    })
}

/// 判断 Runtime 是否可供组织使用（内置全局或同组织外部）。
pub fn runtime_visible_to_organization(organization_id: Uuid, runtime: &RuntimeConnection) -> bool {
    match runtime.organization_id {
        None => runtime.status == RuntimeStatus::Ready,
        Some(runtime_organization_id) => runtime_organization_id == organization_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_catalog::{RuntimeAdapterKind, RuntimeConfig, RuntimeOrigin};
    use crate::time_utils::now;

    fn fixture() -> (
        BusinessAgent,
        RuntimeConnection,
        OrganizationMembership,
        OrganizationMembership,
    ) {
        let organization_id = Uuid::new_v4();
        let unit_id = Uuid::new_v4();
        let runtime = RuntimeConnection {
            id: Uuid::new_v4(),
            organization_id: None,
            name: "sandbox-default".to_owned(),
            display_name: "Sandbox".to_owned(),
            origin: RuntimeOrigin::Builtin,
            adapter_kind: RuntimeAdapterKind::Sandbox,
            endpoint_url: None,
            credential_alias: None,
            status: RuntimeStatus::Ready,
            status_detail: None,
            config: RuntimeConfig::default(),
            created_at: now(),
            updated_at: now(),
        };
        let agent = BusinessAgent {
            id: Uuid::new_v4(),
            organization_id,
            runtime_connection_id: runtime.id,
            name: "assistant".to_owned(),
            display_name: "Assistant".to_owned(),
            description: "test".to_owned(),
            prompt: "help".to_owned(),
            owner_admin_id: 1,
            enabled: true,
            descriptor_checksum: String::new(),
            created_at: now(),
            updated_at: now(),
        };
        let membership = OrganizationMembership {
            organization_id,
            user_id: 42,
            unit_id: Some(unit_id),
            display_name: None,
            created_at: now(),
        };
        let other_membership = OrganizationMembership {
            organization_id,
            user_id: 43,
            unit_id: Some(Uuid::new_v4()),
            display_name: None,
            created_at: now(),
        };
        (agent, runtime, membership, other_membership)
    }

    fn rule(
        agent: &BusinessAgent,
        effect: AccessEffect,
        subject: AccessSubject,
    ) -> AgentAccessRule {
        AgentAccessRule {
            id: Uuid::new_v4(),
            agent_id: agent.id,
            effect,
            subject,
            created_at: now(),
        }
    }

    #[test]
    fn user_deny_overrides_unit_allow() {
        let (agent, runtime, membership, _) = fixture();
        let rules = vec![
            rule(
                &agent,
                AccessEffect::Allow,
                AccessSubject::Unit(membership.unit_id.unwrap()),
            ),
            rule(&agent, AccessEffect::Deny, AccessSubject::User(42)),
        ];
        assert_eq!(
            authorize_agent(&agent, &runtime, &rules, Some(&membership), 42),
            Err(AgentError::Forbidden("agent_access_denied"))
        );
    }

    #[test]
    fn unit_allow_does_not_apply_to_other_unit_members() {
        let (agent, runtime, membership, other_membership) = fixture();
        let rules = vec![rule(
            &agent,
            AccessEffect::Allow,
            AccessSubject::Unit(membership.unit_id.unwrap()),
        )];
        assert_eq!(
            authorize_agent(&agent, &runtime, &rules, Some(&other_membership), 43),
            Err(AgentError::Forbidden("agent_access_denied"))
        );
    }

    #[test]
    fn organization_allow_covers_all_members_and_records_basis() {
        let (agent, runtime, membership, _) = fixture();
        let rules = vec![rule(
            &agent,
            AccessEffect::Allow,
            AccessSubject::Organization(agent.organization_id),
        )];
        let basis = authorize_agent(&agent, &runtime, &rules, Some(&membership), 42).unwrap();
        assert_eq!(basis.matched_subject, "organization");
        assert_eq!(basis.policy_checksum.len(), 64);
    }

    #[test]
    fn disabled_agent_and_unavailable_runtime_are_not_runnable() {
        let (mut agent, mut runtime, membership, _) = fixture();
        let rules = vec![rule(
            &agent,
            AccessEffect::Allow,
            AccessSubject::Organization(agent.organization_id),
        )];
        agent.enabled = false;
        assert_eq!(
            authorize_agent(&agent, &runtime, &rules, Some(&membership), 42),
            Err(AgentError::AgentNotRunnable)
        );
        agent.enabled = true;
        runtime.status = RuntimeStatus::Unavailable;
        assert_eq!(
            authorize_agent(&agent, &runtime, &rules, Some(&membership), 42),
            Err(AgentError::RuntimeUnavailable)
        );
    }

    #[test]
    fn descriptor_checksum_changes_with_prompt_but_not_with_enabled() {
        let (mut agent, _, _, _) = fixture();
        let initial = agent.compute_checksum().unwrap();
        agent.enabled = false;
        assert_eq!(agent.compute_checksum().unwrap(), initial);
        agent.prompt.push('!');
        assert_ne!(agent.compute_checksum().unwrap(), initial);
    }

    #[test]
    fn access_rule_validation_rejects_deny_for_units() {
        let (agent, _, _, _) = fixture();
        let invalid = vec![AccessRuleInput {
            effect: AccessEffect::Deny,
            subject: AccessSubject::Unit(Uuid::new_v4()),
        }];
        assert!(validate_access_rules(&agent, &invalid).is_err());
    }
}
