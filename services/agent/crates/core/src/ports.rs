//! 核心端口：持久化、Blob、Runtime 探测、执行、事件观察与委派。
//!
//! 核心层只定义契约；PostgreSQL、文件系统、Sandbox/A2A 与 tracing 客户端
//! 都由外层实现，并在 composition root 注入。

use std::sync::Arc;

use async_trait::async_trait;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::agent_catalog::{AgentAccessRule, BusinessAgent};
use crate::audit::{AuditRecord, NewAuditRecord};
use crate::delegation::DelegationPolicy;
use crate::error::AgentError;
use crate::organization::{AdminScope, Organization, OrganizationMembership, OrganizationUnit};
use crate::resources::{Artifact, NewArtifact, ResourceContent, UserResource};
use crate::routing::EntryPolicy;
use crate::run::{
    CancelOutcome, CreateRunOutcome, FinalizeRunCommand, NewRun, PendingTraceEvent, RunEvent,
    RunEventDraft, RunOrigin, RunRecord, RunStatus,
};
use crate::runtime_catalog::{BuiltinRuntimeDeclaration, RuntimeConnection};
use crate::session::{ChatMessage, ChatSession};
use crate::skill_access::{Skill, SkillAsset};

/// 组织、部门、成员关系与管理员 scope 的持久化端口。
#[async_trait]
pub trait DirectoryRepository: Send + Sync {
    /// 创建组织。
    async fn create_organization(
        &self,
        organization: &Organization,
        created_by_admin_id: i64,
    ) -> Result<(), AgentError>;
    /// 列出全部组织。
    async fn list_organizations(&self) -> Result<Vec<Organization>, AgentError>;
    /// 按 ID 读取组织。
    async fn get_organization(
        &self,
        organization_id: Uuid,
    ) -> Result<Option<Organization>, AgentError>;
    /// 按 slug 读取组织。
    async fn find_organization_by_slug(
        &self,
        slug: &str,
    ) -> Result<Option<Organization>, AgentError>;
    /// 创建部门。
    async fn create_unit(&self, unit: &OrganizationUnit) -> Result<(), AgentError>;
    /// 列出组织部门。
    async fn list_units(&self, organization_id: Uuid) -> Result<Vec<OrganizationUnit>, AgentError>;
    /// 删除部门；仍有成员时返回冲突。
    async fn delete_unit(&self, organization_id: Uuid, unit_id: Uuid) -> Result<(), AgentError>;
    /// 新增或更新显式成员关系。
    async fn upsert_membership(
        &self,
        membership: &OrganizationMembership,
    ) -> Result<(), AgentError>;
    /// 列出组织成员。
    async fn list_memberships(
        &self,
        organization_id: Uuid,
    ) -> Result<Vec<OrganizationMembership>, AgentError>;
    /// 列出用户加入的组织。
    async fn list_memberships_for_user(
        &self,
        user_id: i64,
    ) -> Result<Vec<OrganizationMembership>, AgentError>;
    /// 读取单个成员关系。
    async fn get_membership(
        &self,
        organization_id: Uuid,
        user_id: i64,
    ) -> Result<Option<OrganizationMembership>, AgentError>;
    /// 删除成员关系。
    async fn remove_membership(
        &self,
        organization_id: Uuid,
        user_id: i64,
    ) -> Result<(), AgentError>;
    /// 授予管理员组织 scope。
    async fn grant_admin_scope(&self, scope: &AdminScope) -> Result<(), AgentError>;
    /// 列出组织 scope。
    async fn list_admin_scopes(&self, organization_id: Uuid)
    -> Result<Vec<AdminScope>, AgentError>;
    /// 列出管理员拥有的 scope。
    async fn list_scopes_for_admin(
        &self,
        admin_user_id: i64,
    ) -> Result<Vec<AdminScope>, AgentError>;
    /// 读取单个管理员 scope。
    async fn get_admin_scope(
        &self,
        organization_id: Uuid,
        admin_user_id: i64,
    ) -> Result<Option<AdminScope>, AgentError>;
    /// 撤销管理员组织 scope。
    async fn revoke_admin_scope(
        &self,
        organization_id: Uuid,
        admin_user_id: i64,
    ) -> Result<(), AgentError>;
}

/// Runtime、Agent、访问规则、入口/委派策略与 Skill 的持久化端口。
#[async_trait]
pub trait CatalogRepository: Send + Sync {
    /// 幂等对齐部署声明的内置 Runtime。
    async fn sync_builtin_runtimes(
        &self,
        declarations: &[BuiltinRuntimeDeclaration],
    ) -> Result<(), AgentError>;
    /// 插入组织外部 Runtime。
    async fn insert_external_runtime(&self, runtime: &RuntimeConnection) -> Result<(), AgentError>;
    /// 更新 Runtime 状态与配置。
    async fn update_runtime(&self, runtime: &RuntimeConnection) -> Result<(), AgentError>;
    /// 按 ID 读取 Runtime。
    async fn get_runtime(&self, runtime_id: Uuid) -> Result<Option<RuntimeConnection>, AgentError>;
    /// 按组织与名称读取 Runtime；`None` 组织表示内置。
    async fn find_runtime_by_name(
        &self,
        organization_id: Option<Uuid>,
        name: &str,
    ) -> Result<Option<RuntimeConnection>, AgentError>;
    /// 列出组织可见的内置与外部 Runtime。
    async fn list_runtimes_for_organization(
        &self,
        organization_id: Uuid,
    ) -> Result<Vec<RuntimeConnection>, AgentError>;
    /// 创建 Agent 与初始访问规则。
    async fn insert_agent(
        &self,
        agent: &BusinessAgent,
        rules: &[AgentAccessRule],
    ) -> Result<(), AgentError>;
    /// 更新 Agent descriptor；不修改 Runtime binding。
    async fn update_agent(&self, agent: &BusinessAgent) -> Result<(), AgentError>;
    /// 按 ID 读取 Agent。
    async fn get_agent(&self, agent_id: Uuid) -> Result<Option<BusinessAgent>, AgentError>;
    /// 列出组织 Agent。
    async fn list_agents(&self, organization_id: Uuid) -> Result<Vec<BusinessAgent>, AgentError>;
    /// 替换 Agent 访问规则。
    async fn replace_access_rules(
        &self,
        agent_id: Uuid,
        rules: &[AgentAccessRule],
    ) -> Result<(), AgentError>;
    /// 列出 Agent 访问规则。
    async fn list_access_rules(&self, agent_id: Uuid) -> Result<Vec<AgentAccessRule>, AgentError>;
    /// 读取 Agent 入口策略。
    async fn get_entry_policy(&self, agent_id: Uuid) -> Result<Option<EntryPolicy>, AgentError>;
    /// 新增或更新入口策略。
    async fn upsert_entry_policy(&self, policy: &EntryPolicy) -> Result<(), AgentError>;
    /// 列出组织入口策略。
    async fn list_entry_policies(
        &self,
        organization_id: Uuid,
    ) -> Result<Vec<EntryPolicy>, AgentError>;
    /// 读取委派策略。
    async fn get_delegation_policy(
        &self,
        parent_agent_id: Uuid,
    ) -> Result<Option<DelegationPolicy>, AgentError>;
    /// 新增或更新委派策略。
    async fn upsert_delegation_policy(&self, policy: &DelegationPolicy) -> Result<(), AgentError>;
    /// 创建 Skill。
    async fn insert_skill(&self, skill: &Skill) -> Result<(), AgentError>;
    /// 更新 Skill。
    async fn update_skill(&self, skill: &Skill) -> Result<(), AgentError>;
    /// 读取 Skill。
    async fn get_skill(&self, skill_id: Uuid) -> Result<Option<Skill>, AgentError>;
    /// 列出组织 Skill。
    async fn list_skills(&self, organization_id: Uuid) -> Result<Vec<Skill>, AgentError>;
    /// 列出分配给 Agent 的 Skill ID。
    async fn list_skill_assignments(&self, agent_id: Uuid) -> Result<Vec<Uuid>, AgentError>;
    /// 替换 Agent 的 Skill 授权。
    async fn replace_skill_assignments(
        &self,
        agent_id: Uuid,
        skill_ids: &[Uuid],
    ) -> Result<(), AgentError>;
    /// 插入 Skill 资产元数据。
    async fn insert_skill_asset(
        &self,
        asset: &SkillAsset,
        storage_key: &str,
    ) -> Result<(), AgentError>;
    /// 列出 Skill 资产及存储键。
    async fn list_skill_assets(
        &self,
        skill_ids: &[Uuid],
    ) -> Result<Vec<StoredSkillAsset>, AgentError>;
    /// 读取单个 Skill 资产元数据。
    async fn get_skill_asset(&self, asset_id: Uuid)
    -> Result<Option<StoredSkillAsset>, AgentError>;
    /// 删除 Skill 资产元数据。
    async fn delete_skill_asset(&self, asset_id: Uuid) -> Result<(), AgentError>;
}

/// Skill 资产元数据与 Blob 存储键。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredSkillAsset {
    /// 资产元数据。
    pub asset: SkillAsset,
    /// Blob 存储键。
    pub storage_key: String,
}

/// Session 与消息的持久化端口。
#[async_trait]
pub trait ConversationRepository: Send + Sync {
    /// 创建 Session。
    async fn insert_session(&self, session: &ChatSession) -> Result<(), AgentError>;
    /// 按 ID 读取 Session。
    async fn get_session(&self, session_id: Uuid) -> Result<Option<ChatSession>, AgentError>;
    /// 列出用户在某组织的 Session。
    async fn list_sessions(
        &self,
        organization_id: Uuid,
        owner_user_id: i64,
    ) -> Result<Vec<ChatSession>, AgentError>;
    /// 更新 Session 标题。
    async fn update_session_title(
        &self,
        session_id: Uuid,
        owner_user_id: i64,
        title: &str,
    ) -> Result<(), AgentError>;
    /// 列出用户可见消息；委派子 Run 的内部消息被过滤。
    async fn list_visible_messages(&self, session_id: Uuid)
    -> Result<Vec<ChatMessage>, AgentError>;
    /// 列出 Session 的 Run。
    async fn list_session_runs(
        &self,
        session_id: Uuid,
        owner_user_id: i64,
    ) -> Result<Vec<RunRecord>, AgentError>;
    /// 读取 Run 的成功 assistant 消息。
    async fn get_assistant_message(&self, run_id: Uuid) -> Result<Option<ChatMessage>, AgentError>;
}

/// Canonical Run、事件、幂等与 tracing outbox 的持久化端口。
#[async_trait]
pub trait RunRepository: Send + Sync {
    /// 在单个事务内创建 Run、输入消息、首批事件、outbox 与幂等记录。
    async fn create_run(
        &self,
        new_run: &NewRun,
        initial_events: &[RunEventDraft],
    ) -> Result<CreateRunOutcome, AgentError>;
    /// 读取 Run。
    async fn get_run(&self, run_id: Uuid) -> Result<Option<RunRecord>, AgentError>;
    /// 读取检查点之后的持久事件，供轮询与 SSE 续读。
    async fn list_run_events(
        &self,
        run_id: Uuid,
        after_seq: i64,
        limit: usize,
    ) -> Result<Vec<RunEvent>, AgentError>;
    /// 将 queued Run 置为 running 并追加 `run.started`；状态已变化时返回 `None`。
    async fn mark_run_started(
        &self,
        run_id: Uuid,
        started_at: OffsetDateTime,
    ) -> Result<Option<RunRecord>, AgentError>;
    /// 追加事件并写入 outbox；事件序号在事务内连续分配。
    async fn append_events(
        &self,
        run_id: Uuid,
        drafts: &[RunEventDraft],
    ) -> Result<Vec<RunEvent>, AgentError>;
    /// 落盘终态、消息、产物与终止事件。
    async fn finalize_run(&self, command: &FinalizeRunCommand) -> Result<RunRecord, AgentError>;
    /// 幂等协作式取消。
    async fn request_cancel(
        &self,
        run_id: Uuid,
        owner_user_id: i64,
        requested_by: &str,
        requested_at: OffsetDateTime,
    ) -> Result<CancelOutcome, AgentError>;
    /// 读取轻量状态，供执行器取消探测。
    async fn get_run_status(&self, run_id: Uuid) -> Result<Option<RunStatus>, AgentError>;
    /// 统计父 Run 已创建的子 Run 数。
    async fn count_child_runs(&self, parent_run_id: Uuid) -> Result<u32, AgentError>;
    /// 列出等待执行的 queued Run，供后台调度器领取。
    async fn list_queued_run_ids(&self, limit: usize) -> Result<Vec<Uuid>, AgentError>;
    /// 服务启动时收敛遗留的 queued/running/cancelling Run。
    async fn recover_incomplete_runs(&self, now: OffsetDateTime) -> Result<u64, AgentError>;
    /// 列出存在待投递 outbox 的 Run。
    async fn list_pending_outbox_run_ids(&self, limit: usize) -> Result<Vec<Uuid>, AgentError>;
    /// 按 Run 读取检查点之后的待投递事件。
    async fn fetch_pending_outbox_for_run(
        &self,
        run_id: Uuid,
        limit: usize,
    ) -> Result<Vec<PendingTraceEvent>, AgentError>;
    /// 确认连续投递并推进检查点。
    async fn mark_trace_delivered(
        &self,
        run_id: Uuid,
        delivered_through_seq: i64,
        delivered_at: OffsetDateTime,
    ) -> Result<(), AgentError>;
    /// 记录一次投递失败与下次重试时间。
    async fn mark_trace_failed(
        &self,
        run_id: Uuid,
        seq: i64,
        error_code: &str,
        next_attempt_at: OffsetDateTime,
    ) -> Result<(), AgentError>;
    /// 读取 Run 来源，供 Session 侧过滤委派消息。
    async fn get_run_origin(&self, run_id: Uuid) -> Result<Option<RunOrigin>, AgentError>;
}

/// 用户资源与 Run 产物元数据的持久化端口。
#[async_trait]
pub trait ResourceRepository: Send + Sync {
    /// 插入用户资源元数据。
    async fn insert_resource(&self, resource: &UserResource) -> Result<(), AgentError>;
    /// 读取用户资源。
    async fn get_resource(&self, resource_id: Uuid) -> Result<Option<UserResource>, AgentError>;
    /// 列出用户资源。
    async fn list_resources(
        &self,
        organization_id: Uuid,
        owner_user_id: i64,
    ) -> Result<Vec<UserResource>, AgentError>;
    /// 软删除用户资源。
    async fn soft_delete_resource(
        &self,
        organization_id: Uuid,
        owner_user_id: i64,
        resource_id: Uuid,
    ) -> Result<(), AgentError>;
    /// 读取产物。
    async fn get_artifact(&self, artifact_id: Uuid) -> Result<Option<Artifact>, AgentError>;
    /// 列出 Run 产物。
    async fn list_artifacts_for_run(&self, run_id: Uuid) -> Result<Vec<Artifact>, AgentError>;
}

/// 审计记录持久化端口。
#[async_trait]
pub trait AuditRepository: Send + Sync {
    /// 追加审计记录。
    async fn append(&self, record: &NewAuditRecord) -> Result<(), AgentError>;
    /// 列出审计记录；`None` 组织表示只返回平台级记录。
    async fn list(
        &self,
        organization_id: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<AuditRecord>, AgentError>;
}

/// Blob 存储端口。
#[async_trait]
pub trait BlobStore: Send + Sync {
    /// 写入 Blob。
    async fn put(&self, storage_key: &str, bytes: &[u8]) -> Result<(), AgentError>;
    /// 读取 Blob；不存在时为 `None`。
    async fn get(&self, storage_key: &str) -> Result<Option<Vec<u8>>, AgentError>;
    /// 删除 Blob；不存在视为成功。
    async fn delete(&self, storage_key: &str) -> Result<(), AgentError>;
}

/// readiness 探测端口；由基础设施层实现，API 只读取结果。
#[async_trait]
pub trait ReadinessProbe: Send + Sync {
    /// 检查关键依赖；失败原因只进入日志。
    async fn check_ready(&self) -> Result<(), AgentError>;
}

/// 外部 A2A Runtime 的 Card 探测结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeProbeResult {
    /// 是否可用。
    pub ready: bool,
    /// 低敏状态说明。
    pub status_detail: Option<String>,
    /// Agent Card 展示名。
    pub card_name: Option<String>,
}

/// Runtime 探测端口。
#[async_trait]
pub trait RuntimeProbe: Send + Sync {
    /// 探测 A2A Agent Card。
    async fn probe(
        &self,
        endpoint_url: &str,
        credential_alias: Option<&str>,
    ) -> Result<RuntimeProbeResult, AgentError>;
}

/// 凭据别名解析端口；真实密钥只来自部署环境。
pub trait CredentialResolver: Send + Sync {
    /// 解析别名；未配置时返回 `None`。
    fn resolve(&self, alias: &str) -> Option<String>;
}

/// Skill 资产正文；执行器只能读取本次 Run 快照涉及的资产。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillAssetContent {
    /// Skill UUID。
    pub skill_id: Uuid,
    /// 资产名。
    pub name: String,
    /// MIME 类型。
    pub media_type: String,
    /// 正文。
    pub bytes: Vec<u8>,
}

/// 一次 Run 执行所需的只读上下文与协作端口。
pub struct ExecutionRequest {
    /// Run 及其冻结快照。
    pub run: RunRecord,
    /// 本次 assistant 消息 UUID；流式增量与完成事件共用该 ID。
    pub assistant_message_id: Uuid,
    /// 输入资源正文。
    pub resources: Vec<ResourceContent>,
    /// 选择的 Skill 资产正文。
    pub skill_assets: Vec<SkillAssetContent>,
    /// 事件观察者。
    pub observer: Arc<dyn ExecutionObserver>,
    /// 取消探测器。
    pub cancellation: Arc<dyn CancellationProbe>,
    /// 委派处理器。
    pub delegation: Arc<dyn DelegationHandler>,
}

/// 执行器成功返回的结果；服务层负责消息与终态落盘。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionOutcome {
    /// assistant 消息完整正文。
    pub assistant_content: String,
    /// 待保存产物。
    pub artifacts: Vec<NewArtifact>,
    /// 是否已确认停止外部执行。
    pub external_stop_confirmed: bool,
}

/// 执行过程中的事件观察端口。
#[async_trait]
pub trait ExecutionObserver: Send + Sync {
    /// 立即持久化一条事件。
    async fn observe(&self, draft: RunEventDraft) -> Result<(), AgentError>;
}

/// 协作式取消探测端口。
#[async_trait]
pub trait CancellationProbe: Send + Sync {
    /// 是否已请求取消。
    async fn is_cancel_requested(&self) -> Result<bool, AgentError>;
}

/// 委派请求。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegationRequest {
    /// 父 Run。
    pub parent_run_id: Uuid,
    /// 父 Run 的工具调用标识。
    pub tool_call_id: String,
    /// 子 Agent。
    pub child_agent_id: Uuid,
    /// 子任务输入。
    pub input: String,
}

/// 委派执行结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegationResult {
    /// 子 Run。
    pub child_run_id: Uuid,
    /// 子 Run 终态。
    pub status: RunStatus,
    /// 成功时的结果正文。
    pub content: Option<String>,
    /// 失败或取消的错误类别。
    pub error_code: Option<String>,
}

/// 受治理子 Run 创建与执行端口。
#[async_trait]
pub trait DelegationHandler: Send + Sync {
    /// 校验策略、创建子 Run 并等待其结束。
    async fn delegate(&self, request: DelegationRequest) -> Result<DelegationResult, AgentError>;
}

/// 执行适配器端口。
#[async_trait]
pub trait RunExecutor: Send + Sync {
    /// 按 Run 快照中的 Runtime 协议执行。
    async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionOutcome, AgentError>;
}

/// 将任意执行错误转为低敏错误类别。
pub fn error_code(error: &AgentError) -> String {
    error.code().to_owned()
}
