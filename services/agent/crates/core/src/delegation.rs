//! 受治理的 Sandbox 子 Agent 委派：权限、深度与子 Run 预算。

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AgentError;
use crate::hash::sha256_json;

/// 父 Agent 的委派策略。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegationPolicy {
    /// 父 Agent。
    pub parent_agent_id: Uuid,
    /// 允许委派到的同组织 Agent。
    pub child_agent_ids: Vec<Uuid>,
    /// 最大委派深度；0 表示禁止。
    pub max_depth: u8,
    /// 单次父 Run 最多子 Run 数。
    pub max_child_runs: u8,
}

/// 更新委派策略的输入。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegationPolicyUpdate {
    /// 允许委派到的同组织 Agent。
    pub child_agent_ids: Vec<Uuid>,
    /// 最大委派深度；0 表示禁止。
    pub max_depth: u8,
    /// 单次父 Run 最多子 Run 数。
    pub max_child_runs: u8,
}

/// 快照中的子 Agent 引用。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegationChildRef {
    /// 子 Agent UUID。
    pub agent_id: Uuid,
    /// 组织内名称。
    pub name: String,
    /// 展示名称。
    pub display_name: String,
}

/// Run 快照中的委派治理信息。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegationSnapshot {
    /// 允许的子 Agent。
    pub child_agents: Vec<DelegationChildRef>,
    /// 最大深度。
    pub max_depth: u8,
    /// 单次父 Run 的子 Run 预算。
    pub max_child_runs: u8,
    /// 策略摘要。
    pub checksum: String,
}

/// 委派被拒绝的稳定原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DelegationRejection {
    /// 子 Agent 不在策略内。
    NotAllowed,
    /// 超过最大深度。
    DepthExceeded,
    /// 子 Run 预算耗尽。
    BudgetExhausted,
}

impl DelegationRejection {
    /// 用于 delegation.rejected 事件的稳定原因码。
    pub fn reason_code(self) -> &'static str {
        match self {
            Self::NotAllowed => "child_not_allowed",
            Self::DepthExceeded => "depth_exceeded",
            Self::BudgetExhausted => "child_budget_exhausted",
        }
    }
}

/// 校验策略输入。
///
/// # Errors
///
/// 深度/预算越界、列表重复或包含父 Agent 自身时返回错误。
pub fn validate_policy(policy: &DelegationPolicy) -> Result<(), AgentError> {
    if policy.max_depth > 3 || policy.max_child_runs > 8 {
        return Err(AgentError::InvalidRequest(
            "委派深度最多 3，子 Run 预算最多 8".to_owned(),
        ));
    }
    if policy.child_agent_ids.len() > 8 {
        return Err(AgentError::InvalidRequest(
            "委派子 Agent 最多 8 个".to_owned(),
        ));
    }
    if policy.child_agent_ids.contains(&policy.parent_agent_id) {
        return Err(AgentError::InvalidRequest(
            "委派策略不能指向父 Agent 自身".to_owned(),
        ));
    }
    let mut unique = policy.child_agent_ids.clone();
    unique.sort_unstable();
    unique.dedup();
    if unique.len() != policy.child_agent_ids.len() {
        return Err(AgentError::InvalidRequest(
            "委派策略包含重复子 Agent".to_owned(),
        ));
    }
    Ok(())
}

/// 判断本次委派是否被策略允许。
///
/// # Errors
///
/// 越权、超深或预算耗尽时返回稳定拒绝原因。
pub fn admit_delegation(
    policy: &DelegationPolicy,
    child_depth: u8,
    existing_child_runs: u8,
    child_agent_id: Uuid,
) -> Result<(), DelegationRejection> {
    if policy.max_depth == 0 || !policy.child_agent_ids.contains(&child_agent_id) {
        return Err(DelegationRejection::NotAllowed);
    }
    if child_depth > policy.max_depth {
        return Err(DelegationRejection::DepthExceeded);
    }
    if existing_child_runs >= policy.max_child_runs {
        return Err(DelegationRejection::BudgetExhausted);
    }
    Ok(())
}

/// 生成委派快照。
///
/// # Errors
///
/// 序列化失败时返回内部错误。
pub fn delegation_snapshot(
    policy: &DelegationPolicy,
    child_agents: Vec<DelegationChildRef>,
) -> Result<DelegationSnapshot, AgentError> {
    let mut child_agents = child_agents;
    child_agents.sort_by_key(|child| child.agent_id);
    let checksum = sha256_json(&(
        policy.parent_agent_id,
        &child_agents,
        policy.max_depth,
        policy.max_child_runs,
    ))?;
    Ok(DelegationSnapshot {
        child_agents,
        max_depth: policy.max_depth,
        max_child_runs: policy.max_child_runs,
        checksum,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegation_requires_policy_membership_depth_and_budget() {
        let parent = Uuid::new_v4();
        let child = Uuid::new_v4();
        let policy = DelegationPolicy {
            parent_agent_id: parent,
            child_agent_ids: vec![child],
            max_depth: 1,
            max_child_runs: 1,
        };
        assert!(admit_delegation(&policy, 1, 0, child).is_ok());
        assert_eq!(
            admit_delegation(&policy, 2, 0, child),
            Err(DelegationRejection::DepthExceeded)
        );
        assert_eq!(
            admit_delegation(&policy, 1, 1, child),
            Err(DelegationRejection::BudgetExhausted)
        );
        assert_eq!(
            admit_delegation(&policy, 1, 0, Uuid::new_v4()),
            Err(DelegationRejection::NotAllowed)
        );
    }

    #[test]
    fn policy_rejects_self_delegation() {
        let parent = Uuid::new_v4();
        let policy = DelegationPolicy {
            parent_agent_id: parent,
            child_agent_ids: vec![parent],
            max_depth: 1,
            max_child_runs: 1,
        };
        assert!(validate_policy(&policy).is_err());
    }
}
