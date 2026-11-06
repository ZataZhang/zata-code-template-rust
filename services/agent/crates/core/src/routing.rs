//! 显式 Agent 与 Auto 入口策略裁决；只从已授权且 runnable 的候选中选择。

use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::AgentError;
use crate::hash::sha256_json;

/// 组织 Auto 入口策略。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryPolicy {
    /// 候选 Agent。
    pub agent_id: Uuid,
    /// 所属组织。
    pub organization_id: Uuid,
    /// 是否允许 Auto 选择。
    pub auto_visible: bool,
    /// 数字越小越优先。
    pub priority: i32,
    /// 最近更新时间。
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Auto 候选及其策略优先级。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutingCandidate {
    /// 候选 Agent。
    pub agent_id: Uuid,
    /// 组织内名称，仅用于同优先级稳定排序。
    pub name: String,
    /// 策略优先级。
    pub priority: i32,
}

/// 路由裁决结果，也是 Run 快照与 tracing 事件的来源。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingDecision {
    /// 入口来源：explicit 或 auto。
    pub entry_source: String,
    /// Session 显式指定的 Agent。
    pub entry_agent_id: Option<Uuid>,
    /// 参与裁决的候选（已授权且 runnable）。
    pub candidate_agent_ids: Vec<Uuid>,
    /// 最终选择。
    pub selected_agent_id: Uuid,
    /// 稳定裁决原因。
    pub reason_code: String,
    /// 入口策略摘要。
    pub policy_checksum: String,
    /// 候选能力集合摘要。
    pub capability_checksum: String,
}

impl RoutingDecision {
    /// 转为 tracing `routing.selected` 事件载荷。
    pub fn event_payload(&self) -> serde_json::Value {
        json!({
            "entry_source": self.entry_source,
            "entry_agent_id": self.entry_agent_id.map(|id| id.to_string()),
            "candidate_agent_ids": self
                .candidate_agent_ids
                .iter()
                .map(Uuid::to_string)
                .collect::<Vec<_>>(),
            "selected_agent_id": self.selected_agent_id.to_string(),
            "reason_code": self.reason_code,
            "policy_checksum": self.policy_checksum,
            "capability_checksum": self.capability_checksum,
        })
    }
}

/// 作出路由裁决。
///
/// # Errors
///
/// Auto 没有候选，或显式 Agent 不在可用候选中时返回稳定错误。
pub fn decide_route(
    entry_agent_id: Option<Uuid>,
    candidates: Vec<RoutingCandidate>,
) -> Result<RoutingDecision, AgentError> {
    let mut candidates = candidates;
    candidates.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.agent_id.cmp(&right.agent_id))
    });
    candidates.dedup_by_key(|candidate| candidate.agent_id);
    let capability_checksum = sha256_json(
        &candidates
            .iter()
            .map(|candidate| candidate.agent_id)
            .collect::<Vec<_>>(),
    )?;
    let policy_checksum = sha256_json(
        &candidates
            .iter()
            .map(|candidate| (candidate.agent_id, candidate.priority))
            .collect::<Vec<_>>(),
    )?;

    let selected_agent_id = match entry_agent_id {
        Some(explicit_agent_id) => {
            if !candidates
                .iter()
                .any(|candidate| candidate.agent_id == explicit_agent_id)
            {
                return Err(AgentError::Forbidden("agent_not_authorized"));
            }
            explicit_agent_id
        }
        None => candidates
            .first()
            .map(|candidate| candidate.agent_id)
            .ok_or(AgentError::NoRunnableAgent)?,
    };
    let candidate_agent_ids = candidates
        .iter()
        .map(|candidate| candidate.agent_id)
        .collect();
    Ok(RoutingDecision {
        entry_source: if entry_agent_id.is_some() {
            "explicit".to_owned()
        } else {
            "auto".to_owned()
        },
        entry_agent_id,
        candidate_agent_ids,
        selected_agent_id,
        reason_code: if entry_agent_id.is_some() {
            "explicit_agent".to_owned()
        } else {
            "auto_entry_priority".to_owned()
        },
        policy_checksum,
        capability_checksum,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(name: &str, priority: i32) -> RoutingCandidate {
        RoutingCandidate {
            agent_id: Uuid::new_v4(),
            name: name.to_owned(),
            priority,
        }
    }

    #[test]
    fn auto_uses_lowest_priority_number() {
        let low = candidate("second", 20);
        let high = candidate("first", 5);
        let decision = decide_route(None, vec![low, high.clone()]).unwrap();
        assert_eq!(decision.selected_agent_id, high.agent_id);
        assert_eq!(decision.entry_source, "auto");
        assert_eq!(decision.candidate_agent_ids.len(), 2);
    }

    #[test]
    fn explicit_agent_must_be_authorized() {
        let allowed = candidate("allowed", 1);
        assert_eq!(
            decide_route(Some(Uuid::new_v4()), vec![allowed.clone()]),
            Err(AgentError::Forbidden("agent_not_authorized"))
        );
        let decision = decide_route(Some(allowed.agent_id), vec![allowed]).unwrap();
        assert_eq!(decision.entry_source, "explicit");
    }

    #[test]
    fn auto_without_candidates_fails_closed() {
        assert_eq!(decide_route(None, vec![]), Err(AgentError::NoRunnableAgent));
    }
}
