//! HTTP 请求与响应 DTO。
//!
//! 请求结构只做字段承载与 `serde` 边界；业务校验留在 core。响应直接复用
//! core 的序列化结构，无法直接序列化的用例结果在这里转换。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tpl_agent_core::agent_catalog::{AccessRuleInput, AgentAccessRule, BusinessAgent};
use tpl_agent_core::delegation::DelegationPolicy;
use tpl_agent_core::resources::{Artifact, UserResource};
use tpl_agent_core::routing::EntryPolicy;
use tpl_agent_core::run::{CancelOutcome, CreateRunOutcome, RunEvent, RunRecord, RunStatus};
use tpl_agent_core::service::public::{AvailableAgent, SessionDetail};
use uuid::Uuid;

/// 统一错误响应。
#[derive(Serialize)]
pub struct ErrorResponse {
    /// 错误对象。
    pub error: ErrorBody,
}

/// 错误对象：稳定错误码 + 中文说明。
#[derive(Serialize)]
pub struct ErrorBody {
    /// 机器可读错误码。
    pub code: String,
    /// 展示用说明。
    pub message: String,
}

/// 存活探针响应。
#[derive(Serialize)]
pub struct LiveResponse {
    /// 固定为 `ok`。
    pub status: &'static str,
}

/// 基本信息响应。
#[derive(Serialize)]
pub struct HealthResponse {
    /// 服务标识。
    pub service: &'static str,
    /// 包版本。
    pub version: &'static str,
}

/// 就绪探针响应。
#[derive(Serialize)]
pub struct ReadinessResponse {
    /// `ready` 或 `unavailable`。
    pub status: &'static str,
    /// 依赖名 → `ok` / `fail`。
    pub checks: BTreeMap<&'static str, &'static str>,
}

/// 列表响应包装。
#[derive(Serialize)]
pub struct ListResponse<T> {
    /// 列表项。
    pub items: Vec<T>,
}

impl<T> ListResponse<T> {
    /// 包装列表。
    pub fn new(items: Vec<T>) -> Self {
        Self { items }
    }
}

/// 创建组织请求。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateOrganizationRequest {
    /// 全局唯一短名。
    pub slug: String,
    /// 展示名称。
    pub display_name: String,
}

/// 创建部门请求。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateUnitRequest {
    /// 组织内唯一名称。
    pub name: String,
}

/// 新增或更新成员关系请求。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertMembershipRequest {
    /// accounts public 用户 ID。
    pub user_id: i64,
    /// 可选显式部门。
    #[serde(default)]
    pub unit_id: Option<Uuid>,
    /// 组织内展示名称。
    #[serde(default)]
    pub display_name: Option<String>,
}

/// 授予组织管理员 scope 请求。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantAdminScopeRequest {
    /// accounts 管理员 ID。
    pub admin_user_id: i64,
}

/// 创建外部 A2A Runtime 请求。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRuntimeRequest {
    /// 组织内唯一名称。
    pub name: String,
    /// 展示名称。
    pub display_name: String,
    /// Agent Card 基础 URL。
    pub endpoint_url: String,
    /// 可选凭据别名。
    #[serde(default)]
    pub credential_alias: Option<String>,
}

/// 创建 Business Agent 请求。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAgentRequest {
    /// 创建时绑定的 Runtime。
    pub runtime_connection_id: Uuid,
    /// 组织内唯一名称。
    pub name: String,
    /// 展示名称。
    pub display_name: String,
    /// 能力描述。
    #[serde(default)]
    pub description: String,
    /// 基础提示词。
    pub prompt: String,
}

/// 更新 Agent descriptor 请求；不含 Runtime binding。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateAgentRequest {
    /// 展示名称。
    pub display_name: String,
    /// 能力描述。
    #[serde(default)]
    pub description: String,
    /// 基础提示词。
    pub prompt: String,
    /// 是否启用。
    pub enabled: bool,
}

/// 替换访问规则请求。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaceAccessRulesRequest {
    /// 新规则集合。
    pub rules: Vec<AccessRuleInput>,
}

/// 新增或更新入口策略请求。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryPolicyRequest {
    /// 是否进入 Auto 候选。
    pub auto_visible: bool,
    /// 数字越小越优先。
    pub priority: i32,
}

/// 新增或更新委派策略请求。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelegationPolicyRequest {
    /// 允许委派到的子 Agent。
    pub child_agent_ids: Vec<Uuid>,
    /// 最大委派深度。
    pub max_depth: u8,
    /// 单次父 Run 的子 Run 预算。
    pub max_child_runs: u8,
}

/// 创建 Skill 请求。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSkillRequest {
    /// 组织内唯一名称。
    pub name: String,
    /// 展示说明。
    #[serde(default)]
    pub description: String,
    /// 指令。
    pub instructions: String,
}

/// 更新 Skill 请求。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateSkillRequest {
    /// 展示说明。
    #[serde(default)]
    pub description: String,
    /// 指令。
    pub instructions: String,
    /// 是否启用。
    pub enabled: bool,
}

/// 替换 Agent Skill 授权请求。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaceAgentSkillsRequest {
    /// Skill UUID 列表。
    pub skill_ids: Vec<Uuid>,
}

/// 创建 Session 请求。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSessionRequest {
    /// 显式 Agent；为空表示 Auto 入口。
    #[serde(default)]
    pub agent_id: Option<Uuid>,
    /// 可选标题。
    #[serde(default)]
    pub title: Option<String>,
}

/// 更新 Session 标题请求。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateSessionTitleRequest {
    /// 新标题。
    pub title: String,
}

/// 发送消息请求。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendMessageRequest {
    /// 显式 Agent；与 Session 固定 Agent 冲突时拒绝。
    #[serde(default)]
    pub agent_id: Option<Uuid>,
    /// 输入正文。
    pub content: String,
    /// 引用的用户资源。
    #[serde(default)]
    pub resource_ids: Vec<Uuid>,
    /// 选择的 Skill。
    #[serde(default)]
    pub skill_ids: Vec<Uuid>,
}

/// Run 事件查询参数。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunEventsQuery {
    /// 只返回该序号之后的事件。
    #[serde(default)]
    pub after_seq: Option<i64>,
    /// 单页上限。
    #[serde(default)]
    pub limit: Option<usize>,
}

/// SSE 查询参数。
#[derive(Deserialize)]
pub struct StreamQuery {
    /// 从该序号之后续读。
    #[serde(default)]
    pub after_seq: Option<i64>,
}

/// Run 创建/重放响应。
#[derive(Serialize)]
pub struct RunCreatedResponse {
    /// Run 记录（含冻结快照）。
    pub run: RunRecord,
    /// 是否本次新建。
    pub created: bool,
}

impl From<CreateRunOutcome> for RunCreatedResponse {
    fn from(outcome: CreateRunOutcome) -> Self {
        Self {
            run: outcome.run,
            created: outcome.created,
        }
    }
}

/// Run 取消响应。
#[derive(Serialize)]
pub struct CancelRunResponse {
    /// 取消后的 Run。
    pub run: RunRecord,
    /// 是否立即进入 cancelled 终态。
    pub cancelled_immediately: bool,
}

impl From<CancelOutcome> for CancelRunResponse {
    fn from(outcome: CancelOutcome) -> Self {
        Self {
            run: outcome.run,
            cancelled_immediately: outcome.cancelled_immediately,
        }
    }
}

/// Run 事件列表响应。
#[derive(Serialize)]
pub struct RunEventsResponse {
    /// Run UUID。
    pub run_id: Uuid,
    /// 当前状态。
    pub status: RunStatus,
    /// 已持久化的最大事件序号。
    pub last_event_seq: i64,
    /// 事件列表。
    pub items: Vec<RunEvent>,
}

/// public 可用 Agent 响应。
#[derive(Serialize)]
pub struct AvailableAgentResponse {
    /// Agent descriptor。
    pub agent: BusinessAgent,
    /// Runtime 展示名。
    pub runtime_name: String,
    /// 是否进入 Auto 候选。
    pub auto_visible: bool,
    /// Auto 优先级。
    pub priority: i32,
}

impl From<AvailableAgent> for AvailableAgentResponse {
    fn from(available: AvailableAgent) -> Self {
        Self {
            agent: available.agent,
            runtime_name: available.runtime_name,
            auto_visible: available.auto_visible,
            priority: available.priority,
        }
    }
}

/// Session 详情响应。
#[derive(Serialize)]
pub struct SessionDetailResponse {
    /// Session。
    pub session: tpl_agent_core::session::ChatSession,
    /// 用户可见消息。
    pub messages: Vec<tpl_agent_core::session::ChatMessage>,
    /// Session 内 Run。
    pub runs: Vec<RunRecord>,
}

impl From<SessionDetail> for SessionDetailResponse {
    fn from(detail: SessionDetail) -> Self {
        Self {
            session: detail.session,
            messages: detail.messages,
            runs: detail.runs,
        }
    }
}

/// 管理面 Agent 概览。
#[derive(Serialize)]
pub struct AgentOverviewResponse {
    /// Agent descriptor。
    pub agent: BusinessAgent,
    /// 访问规则。
    pub access_rules: Vec<AgentAccessRule>,
    /// 入口策略。
    pub entry_policy: Option<EntryPolicy>,
    /// 委派策略。
    pub delegation_policy: Option<DelegationPolicy>,
    /// 已授权 Skill ID。
    pub skill_ids: Vec<Uuid>,
}

/// 管理面身份响应。
#[derive(Serialize)]
pub struct AdminIdentityResponse {
    /// accounts 管理员 ID。
    pub admin_user_id: i64,
    /// 管理员展示名。
    pub display_name: String,
    /// 是否平台管理员；不自动获得组织 scope。
    pub platform_admin: bool,
}

/// 用户资源列表项：隐藏内部存储键。
#[derive(Serialize)]
pub struct UserResourceResponse {
    /// 资源 UUID。
    pub id: Uuid,
    /// 所属组织。
    pub organization_id: Uuid,
    /// 所有者用户 ID。
    pub owner_user_id: i64,
    /// 文件名。
    pub filename: String,
    /// MIME 类型。
    pub media_type: String,
    /// 字节数。
    pub size_bytes: i64,
    /// 内容 SHA-256。
    pub sha256: String,
    /// 上传时间。
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
}

impl From<UserResource> for UserResourceResponse {
    fn from(resource: UserResource) -> Self {
        Self {
            id: resource.id,
            organization_id: resource.organization_id,
            owner_user_id: resource.owner_user_id,
            filename: resource.filename,
            media_type: resource.media_type,
            size_bytes: resource.size_bytes,
            sha256: resource.sha256,
            created_at: resource.created_at,
        }
    }
}

/// Run 产物列表项：隐藏内部存储键。
#[derive(Serialize)]
pub struct ArtifactResponse {
    /// 产物 UUID。
    pub id: Uuid,
    /// 所属 Run。
    pub run_id: Uuid,
    /// 所有者用户 ID。
    pub owner_user_id: i64,
    /// 文件名。
    pub filename: String,
    /// MIME 类型。
    pub media_type: String,
    /// 字节数。
    pub size_bytes: i64,
    /// 内容 SHA-256。
    pub sha256: String,
    /// 创建时间。
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
}

impl From<Artifact> for ArtifactResponse {
    fn from(artifact: Artifact) -> Self {
        Self {
            id: artifact.id,
            run_id: artifact.run_id,
            owner_user_id: artifact.owner_user_id,
            filename: artifact.filename,
            media_type: artifact.media_type,
            size_bytes: artifact.size_bytes,
            sha256: artifact.sha256,
            created_at: artifact.created_at,
        }
    }
}
