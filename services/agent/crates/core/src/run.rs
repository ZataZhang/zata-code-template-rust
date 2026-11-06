//! Canonical Run：状态机、规范化幂等请求、不可变执行快照与终态命令。
//!
//! Run 创建时把 Agent descriptor、Runtime binding、有效提示词、输入、资源、
//! Skill、路由与委派治理冻结为快照；后续配置更新不改写历史 Run。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::agent_catalog::BusinessAgent;
use crate::delegation::DelegationSnapshot;
use crate::error::AgentError;
use crate::hash::{sha256_hex, sha256_json};
use crate::resources::ResourceSnapshot;
use crate::routing::RoutingDecision;
use crate::runtime_catalog::{RuntimeAdapterKind, RuntimeConnection, RuntimeOrigin};
use crate::session::ChatMessage;
use crate::skill_access::SkillSnapshot;
use crate::time_utils::rfc3339;

/// 当前 Run 快照 schema 版本。
pub const RUN_SNAPSHOT_VERSION: u16 = 1;
/// 单个 Run 最多引用的资源数。
pub const MAX_RUN_RESOURCES: usize = 20;
/// 单个 Run 最多选择的 Skill 数。
pub const MAX_RUN_SKILLS: usize = 20;
/// 幂等键最大字节数。
pub const MAX_IDEMPOTENCY_KEY_BYTES: usize = 128;

/// Canonical Run 状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// 已创建，等待执行。
    Queued,
    /// 执行中。
    Running,
    /// 已请求协作式取消，等待当前工作停止。
    Cancelling,
    /// 成功完成。
    Succeeded,
    /// 失败结束。
    Failed,
    /// 已取消结束。
    Cancelled,
}

impl RunStatus {
    /// 数据库与 API 使用的稳定字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// 从数据库字符串恢复。
    pub fn from_database(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "cancelling" => Some(Self::Cancelling),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// 是否为终态。
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    /// 是否仍可能继续工作。
    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Cancelling)
    }

    /// 是否允许从当前状态转换到目标状态。
    pub fn can_transition_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            (
                Self::Queued,
                Self::Running | Self::Cancelling | Self::Failed | Self::Cancelled
            ) | (
                Self::Running,
                Self::Succeeded | Self::Failed | Self::Cancelling
            ) | (Self::Cancelling, Self::Cancelled | Self::Failed)
        )
    }
}

/// Run 的来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOrigin {
    /// Session 显式指定 Agent。
    Direct,
    /// 按 Auto 入口策略选择 Agent。
    Auto,
    /// 父 Agent 的受治理委派。
    Delegation,
}

impl RunOrigin {
    /// 数据库与 API 使用的稳定字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Auto => "auto",
            Self::Delegation => "delegation",
        }
    }

    /// 从数据库字符串恢复。
    pub fn from_database(value: &str) -> Option<Self> {
        match value {
            "direct" => Some(Self::Direct),
            "auto" => Some(Self::Auto),
            "delegation" => Some(Self::Delegation),
            _ => None,
        }
    }
}

/// 快照中的 Runtime binding。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    /// Runtime UUID。
    pub runtime_id: Uuid,
    /// Runtime 名称。
    pub name: String,
    /// 来源。
    pub origin: RuntimeOrigin,
    /// 协议。
    pub adapter_kind: RuntimeAdapterKind,
    /// 外部端点；只保存在 Agent 数据库。
    pub endpoint_url: Option<String>,
    /// 凭据别名；真实密钥不写入快照。
    pub credential_alias: Option<String>,
    /// 工具白名单。
    pub allowed_tools: Vec<String>,
    /// 最大步骤数。
    pub max_steps: u32,
}

impl RuntimeSnapshot {
    fn from_connection(runtime: &RuntimeConnection) -> Self {
        Self {
            runtime_id: runtime.id,
            name: runtime.name.clone(),
            origin: runtime.origin,
            adapter_kind: runtime.adapter_kind,
            endpoint_url: runtime.endpoint_url.clone(),
            credential_alias: runtime.credential_alias.clone(),
            allowed_tools: runtime.config.allowed_tools.clone(),
            max_steps: runtime.config.max_steps,
        }
    }

    /// 判断工具是否在快照白名单内。
    pub fn allows_tool(&self, tool_name: &str) -> bool {
        self.allowed_tools.iter().any(|tool| tool == tool_name)
    }
}

/// 快照中的 Agent descriptor 与 Runtime binding。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSnapshot {
    /// Agent UUID。
    pub agent_id: Uuid,
    /// 组织内名称。
    pub name: String,
    /// 展示名称。
    pub display_name: String,
    /// 能力描述。
    pub description: String,
    /// 生效提示词。
    pub prompt: String,
    /// descriptor 与 Runtime binding 摘要。
    pub descriptor_checksum: String,
    /// 冻结的 Runtime。
    pub runtime: RuntimeSnapshot,
}

/// 快照中的用户输入。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputSnapshot {
    /// 输入消息 UUID。
    pub message_id: Uuid,
    /// 完整输入正文；只保存在 Agent 数据库。
    pub content: String,
    /// 输入正文 SHA-256。
    pub content_checksum: String,
}

impl InputSnapshot {
    fn new(message_id: Uuid, content: String) -> Self {
        let content_checksum = sha256_hex(content.as_bytes());
        Self {
            message_id,
            content,
            content_checksum,
        }
    }
}

/// Run 创建时冻结的完整执行快照。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSnapshot {
    /// schema 版本。
    pub version: u16,
    /// Agent descriptor 与 Runtime binding。
    pub agent: AgentSnapshot,
    /// 输入。
    pub input: InputSnapshot,
    /// 引用的用户资源。
    pub resources: Vec<ResourceSnapshot>,
    /// 选择的 Skill。
    pub skills: Vec<SkillSnapshot>,
    /// 路由裁决。
    pub routing: RoutingDecision,
    /// 授权依据与 ACL 摘要。
    pub access_policy_checksum: String,
    /// 委派治理快照。
    pub delegation: Option<DelegationSnapshot>,
    /// 输入、资源、Skill、路由与授权的组合摘要。
    pub context_pack_checksum: String,
    /// 快照创建时间。
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// 构建 Run 快照的输入。
#[derive(Clone, Debug)]
pub struct RunSnapshotInput {
    /// 当前 Agent descriptor。
    pub agent: BusinessAgent,
    /// Agent 绑定的 Runtime。
    pub runtime: RuntimeConnection,
    /// 输入消息 UUID。
    pub input_message_id: Uuid,
    /// 完整输入正文。
    pub input_content: String,
    /// 资源快照。
    pub resources: Vec<ResourceSnapshot>,
    /// Skill 快照。
    pub skills: Vec<SkillSnapshot>,
    /// 路由裁决。
    pub routing: RoutingDecision,
    /// 授权依据与 ACL 摘要。
    pub access_policy_checksum: String,
    /// 委派治理快照。
    pub delegation: Option<DelegationSnapshot>,
    /// 快照时间。
    pub created_at: OffsetDateTime,
}

impl RunSnapshot {
    /// 构建并校验执行快照。
    ///
    /// # Errors
    ///
    /// 资源/Skill 超限、Runtime binding 不一致或摘要计算失败时返回错误。
    pub fn build(input: RunSnapshotInput) -> Result<Self, AgentError> {
        if input.resources.len() > MAX_RUN_RESOURCES || input.skills.len() > MAX_RUN_SKILLS {
            return Err(AgentError::InvalidRequest(
                "Run 引用的资源或 Skill 超过上限".to_owned(),
            ));
        }
        if input.runtime.id != input.agent.runtime_connection_id {
            return Err(AgentError::RuntimeBindingImmutable);
        }
        let mut resources = input.resources;
        resources.sort_by_key(|resource| resource.resource_id);
        resources.dedup_by_key(|resource| resource.resource_id);
        let mut skills = input.skills;
        skills.sort_by_key(|skill| skill.skill_id);
        let input_snapshot = InputSnapshot::new(input.input_message_id, input.input_content);
        let context_pack_checksum = sha256_json(&(
            &input_snapshot.content_checksum,
            &resources,
            skills
                .iter()
                .map(|skill| &skill.checksum)
                .collect::<Vec<_>>(),
            &input.routing,
            &input.access_policy_checksum,
        ))?;
        Ok(Self {
            version: RUN_SNAPSHOT_VERSION,
            agent: AgentSnapshot {
                agent_id: input.agent.id,
                name: input.agent.name,
                display_name: input.agent.display_name,
                description: input.agent.description,
                prompt: input.agent.prompt,
                descriptor_checksum: input.agent.descriptor_checksum,
                runtime: RuntimeSnapshot::from_connection(&input.runtime),
            },
            input: input_snapshot,
            resources,
            skills,
            routing: input.routing,
            access_policy_checksum: input.access_policy_checksum,
            delegation: input.delegation,
            context_pack_checksum,
            created_at: input.created_at,
        })
    }

    /// Agent descriptor 快照摘要，用于 tracing `run.created`。
    pub fn agent_snapshot_checksum(&self) -> &str {
        &self.agent.descriptor_checksum
    }

    /// 上下文组合摘要，用于 tracing `run.created`。
    pub fn context_pack_checksum(&self) -> &str {
        &self.context_pack_checksum
    }

    /// Runtime binding 摘要。
    pub fn runtime(&self) -> &RuntimeSnapshot {
        &self.agent.runtime
    }

    /// 路由摘要。
    pub fn routing_checksum(&self) -> &str {
        &self.routing.policy_checksum
    }
}

/// 规范化 Run 请求；幂等键在同一 owner 内与摘要共同判定重放或冲突。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalRunRequest {
    /// Session UUID。
    pub session_id: Uuid,
    /// 调用方希望使用的 Agent；为空表示 Auto。
    pub requested_agent_id: Option<Uuid>,
    /// 输入正文。
    pub content: String,
    /// 资源 UUID；计算摘要前排序。
    pub resource_ids: Vec<Uuid>,
    /// Skill UUID；计算摘要前排序。
    pub skill_ids: Vec<Uuid>,
}

impl CanonicalRunRequest {
    /// 计算稳定的请求摘要。
    ///
    /// # Errors
    ///
    /// 序列化失败时返回内部错误。
    pub fn digest(&self) -> Result<String, AgentError> {
        let mut resource_ids = self.resource_ids.clone();
        resource_ids.sort_unstable();
        let mut skill_ids = self.skill_ids.clone();
        skill_ids.sort_unstable();
        sha256_json(&(
            self.session_id,
            self.requested_agent_id,
            &self.content,
            &resource_ids,
            &skill_ids,
        ))
    }

    /// 校验幂等键格式。
    ///
    /// # Errors
    ///
    /// 空、超长或含控制字符时返回错误。
    pub fn validate_idempotency_key(idempotency_key: &str) -> Result<(), AgentError> {
        if idempotency_key.is_empty()
            || idempotency_key.len() > MAX_IDEMPOTENCY_KEY_BYTES
            || idempotency_key.chars().any(char::is_control)
        {
            return Err(AgentError::InvalidRequest(
                "Idempotency-Key 必须在 1-128 字节之间且不含控制字符".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Run 持久化记录。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    /// Run UUID。
    pub run_id: Uuid,
    /// 所属组织。
    pub organization_id: Uuid,
    /// 所属 Session。
    pub session_id: Uuid,
    /// 所有者用户 ID。
    pub owner_user_id: i64,
    /// 使用的 Agent。
    pub agent_id: Uuid,
    /// 绑定的 Runtime。
    pub runtime_connection_id: Uuid,
    /// 父 Run。
    pub parent_run_id: Option<Uuid>,
    /// 委派标识。
    pub delegation_id: Option<Uuid>,
    /// 委派深度。
    pub delegation_depth: u8,
    /// 来源。
    pub origin: RunOrigin,
    /// 当前状态。
    pub status: RunStatus,
    /// 幂等键。
    pub idempotency_key: String,
    /// 规范化请求摘要。
    pub request_digest: String,
    /// 冻结的执行快照。
    pub snapshot: RunSnapshot,
    /// 已写入的最大事件序号。
    pub last_event_seq: i64,
    /// tracing 投递检查点。
    pub trace_delivered_through_seq: i64,
    /// 失败或取消的低敏错误类别。
    pub error_code: Option<String>,
    /// 首次取消请求时间。
    #[serde(with = "time::serde::rfc3339::option")]
    pub cancel_requested_at: Option<OffsetDateTime>,
    /// 开始执行时间。
    #[serde(with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    /// 进入终态时间。
    #[serde(with = "time::serde::rfc3339::option")]
    pub finished_at: Option<OffsetDateTime>,
    /// 创建时间。
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// 新 Run 的创建输入；由核心用例在事务内落盘。
#[derive(Clone, Debug)]
pub struct NewRun {
    /// Run UUID。
    pub run_id: Uuid,
    /// 所属组织。
    pub organization_id: Uuid,
    /// 所属 Session。
    pub session_id: Uuid,
    /// 所有者。
    pub owner_user_id: i64,
    /// Agent。
    pub agent_id: Uuid,
    /// Runtime。
    pub runtime_connection_id: Uuid,
    /// 父 Run。
    pub parent_run_id: Option<Uuid>,
    /// 委派标识。
    pub delegation_id: Option<Uuid>,
    /// 委派深度。
    pub delegation_depth: u8,
    /// 来源。
    pub origin: RunOrigin,
    /// 幂等键。
    pub idempotency_key: String,
    /// 请求摘要。
    pub request_digest: String,
    /// 执行快照。
    pub snapshot: RunSnapshot,
    /// 首个 Session 标题候选；仅当会话仍为默认标题时更新。
    pub session_title: Option<String>,
    /// 创建时间。
    pub created_at: OffsetDateTime,
}

/// Run 内一条持久事件。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEvent {
    /// 所属 Run。
    pub run_id: Uuid,
    /// 从 1 开始的连续序号。
    pub seq: i64,
    /// 事件类型。
    pub event_type: String,
    /// 发生时间。
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    /// 脱敏载荷。
    pub payload: Value,
}

/// 尚未分配序号的 Run 事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunEventDraft {
    /// 事件类型。
    pub event_type: String,
    /// 发生时间。
    pub occurred_at: OffsetDateTime,
    /// 脱敏载荷。
    pub payload: Value,
}

impl RunEventDraft {
    /// 构造事件草稿。
    pub fn new(event_type: impl Into<String>, occurred_at: OffsetDateTime, payload: Value) -> Self {
        Self {
            event_type: event_type.into(),
            occurred_at,
            payload,
        }
    }
}

/// Run 创建事务的结果。
#[derive(Clone, Debug)]
pub struct CreateRunOutcome {
    /// 新建或幂等重放的 Run。
    pub run: RunRecord,
    /// 是否本次新建；重放为 false。
    pub created: bool,
}

/// Run 取消请求的结果。
#[derive(Clone, Debug)]
pub struct CancelOutcome {
    /// 取消后的 Run。
    pub run: RunRecord,
    /// 是否在本次请求中立即进入 cancelled 终态。
    pub cancelled_immediately: bool,
}

/// Run 终态落盘命令。
#[derive(Clone, Debug)]
pub struct FinalizeRunCommand {
    /// Run UUID。
    pub run_id: Uuid,
    /// 终态：succeeded/failed/cancelled。
    pub final_status: RunStatus,
    /// 成功时写入的 assistant 消息。
    pub assistant_message: Option<ChatMessage>,
    /// 产物元数据；Blob 已写入。
    pub artifacts: Vec<crate::resources::Artifact>,
    /// 终态事件（含 message.completed / artifact.created / run.*）。
    pub events: Vec<RunEventDraft>,
    /// 失败或取消的低敏错误类别。
    pub error_code: Option<String>,
    /// 进入终态时间。
    pub finished_at: OffsetDateTime,
}

/// tracing outbox 中待投递的事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingTraceEvent {
    /// Run UUID。
    pub run_id: Uuid,
    /// 事件序号。
    pub seq: i64,
    /// 事件类型。
    pub event_type: String,
    /// 发生时间。
    pub occurred_at: OffsetDateTime,
    /// 已脱敏载荷。
    pub payload: Value,
}

impl PendingTraceEvent {
    /// 转为 tracing producer 的事件 envelope JSON。
    ///
    /// # Errors
    ///
    /// 序号超出 `u64` 或时间格式化失败时返回内部错误。
    pub fn to_envelope(&self) -> Result<Value, AgentError> {
        let seq = u64::try_from(self.seq).map_err(|_| AgentError::Internal)?;
        Ok(serde_json::json!({
            "seq": seq,
            "event_type": self.event_type,
            "occurred_at": rfc3339(self.occurred_at),
            "payload": self.payload,
        }))
    }
}

/// 判断终态是否与事件序列的最后一个终止事件一致。
///
/// # Errors
///
/// 终态与最后事件不匹配时返回内部错误，避免写入不一致记录。
pub fn ensure_terminal_event(
    final_status: RunStatus,
    events: &[RunEventDraft],
) -> Result<(), AgentError> {
    let expected_event_type = match final_status {
        RunStatus::Succeeded => "run.completed",
        RunStatus::Failed => "run.failed",
        RunStatus::Cancelled => "run.cancelled",
        _ => return Err(AgentError::Internal),
    };
    let matches = events
        .last()
        .is_some_and(|event| event.event_type == expected_event_type);
    if matches {
        Ok(())
    } else {
        Err(AgentError::Internal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_catalog::{RuntimeConfig, RuntimeStatus};
    use crate::time_utils::now;

    fn fixture() -> (BusinessAgent, RuntimeConnection) {
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
        let mut agent = BusinessAgent {
            id: Uuid::new_v4(),
            organization_id: Uuid::new_v4(),
            runtime_connection_id: runtime.id,
            name: "assistant".to_owned(),
            display_name: "Assistant".to_owned(),
            description: "helper".to_owned(),
            prompt: "Help the user.".to_owned(),
            owner_admin_id: 1,
            enabled: true,
            descriptor_checksum: String::new(),
            created_at: now(),
            updated_at: now(),
        };
        agent.descriptor_checksum = agent.compute_checksum().unwrap();
        (agent, runtime)
    }

    fn snapshot(agent: BusinessAgent, runtime: RuntimeConnection, content: &str) -> RunSnapshot {
        let routing = RoutingDecision {
            entry_source: "explicit".to_owned(),
            entry_agent_id: Some(agent.id),
            candidate_agent_ids: vec![agent.id],
            selected_agent_id: agent.id,
            reason_code: "explicit_agent".to_owned(),
            policy_checksum: "a".repeat(64),
            capability_checksum: "b".repeat(64),
        };
        RunSnapshot::build(RunSnapshotInput {
            agent,
            runtime,
            input_message_id: Uuid::new_v4(),
            input_content: content.to_owned(),
            resources: Vec::new(),
            skills: Vec::new(),
            routing,
            access_policy_checksum: "c".repeat(64),
            delegation: None,
            created_at: now(),
        })
        .unwrap()
    }

    #[test]
    fn idempotency_digest_ignores_reference_order() {
        let base = CanonicalRunRequest {
            session_id: Uuid::new_v4(),
            requested_agent_id: None,
            content: "hello".to_owned(),
            resource_ids: vec![Uuid::new_v4(), Uuid::new_v4()],
            skill_ids: vec![Uuid::new_v4()],
        };
        let mut reordered = base.clone();
        reordered.resource_ids.reverse();
        assert_eq!(base.digest().unwrap(), reordered.digest().unwrap());
        let mut changed = base.clone();
        changed.content = "hello!".to_owned();
        assert_ne!(base.digest().unwrap(), changed.digest().unwrap());
    }

    #[test]
    fn snapshot_freezes_runtime_and_changes_with_input() {
        let (agent, runtime) = fixture();
        let first = snapshot(agent.clone(), runtime.clone(), "hello");
        assert_eq!(first.runtime().runtime_id, runtime.id);
        assert_eq!(first.agent_snapshot_checksum(), agent.descriptor_checksum);
        let second = snapshot(agent, runtime, "hello again");
        assert_ne!(
            first.context_pack_checksum(),
            second.context_pack_checksum()
        );
    }

    #[test]
    fn snapshot_rejects_mismatched_runtime_binding() {
        let (agent, runtime) = fixture();
        let other_runtime = RuntimeConnection {
            id: Uuid::new_v4(),
            ..runtime
        };
        let routing = RoutingDecision {
            entry_source: "explicit".to_owned(),
            entry_agent_id: Some(agent.id),
            candidate_agent_ids: vec![agent.id],
            selected_agent_id: agent.id,
            reason_code: "explicit_agent".to_owned(),
            policy_checksum: "a".repeat(64),
            capability_checksum: "b".repeat(64),
        };
        let result = RunSnapshot::build(RunSnapshotInput {
            agent,
            runtime: other_runtime,
            input_message_id: Uuid::new_v4(),
            input_content: "hello".to_owned(),
            resources: Vec::new(),
            skills: Vec::new(),
            routing,
            access_policy_checksum: "c".repeat(64),
            delegation: None,
            created_at: now(),
        });
        assert_eq!(result, Err(AgentError::RuntimeBindingImmutable));
    }

    #[test]
    fn run_status_transitions_are_bounded() {
        assert!(RunStatus::Queued.can_transition_to(RunStatus::Running));
        assert!(RunStatus::Running.can_transition_to(RunStatus::Succeeded));
        assert!(RunStatus::Cancelling.can_transition_to(RunStatus::Cancelled));
        assert!(!RunStatus::Succeeded.can_transition_to(RunStatus::Running));
        assert!(!RunStatus::Queued.can_transition_to(RunStatus::Succeeded));
    }

    #[test]
    fn terminal_event_must_match_status() {
        let event = RunEventDraft::new(
            "run.completed",
            now(),
            serde_json::json!({"message_id": Uuid::new_v4().to_string()}),
        );
        assert!(ensure_terminal_event(RunStatus::Succeeded, std::slice::from_ref(&event)).is_ok());
        assert_eq!(
            ensure_terminal_event(RunStatus::Cancelled, &[event]),
            Err(AgentError::Internal)
        );
    }
}
