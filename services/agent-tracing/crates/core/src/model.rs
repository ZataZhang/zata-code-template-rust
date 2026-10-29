//! Agent tracing 查询模型与 API 共享的稳定响应结构。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

use crate::event::EventEnvelope;

/// Producer Bearer Token 最小字节数。
pub const MIN_PRODUCER_TOKEN_BYTES: usize = 32;
/// Producer Bearer Token 最大字节数。
pub const MAX_PRODUCER_TOKEN_BYTES: usize = 4096;

/// producer token 对应的固定来源与项目身份。
#[derive(Clone, PartialEq, Eq)]
pub struct ProducerIdentity {
    /// 来源标识。
    pub source_id: String,
    /// 项目标识。
    pub project_id: String,
}

/// 运行期 producer 注册项；`token_hash` 是 SHA-256 摘要，不是 token 明文。
#[derive(Clone)]
pub struct ProducerRegistration {
    /// 来源标识。
    pub source_id: String,
    /// 项目标识。
    pub project_id: String,
    /// bearer token 的 SHA-256 摘要。
    pub token_hash: Vec<u8>,
}

/// 已持久化的事件记录。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredEvent {
    /// Run 标识。
    pub run_id: String,
    /// producer 来源。
    pub source_id: String,
    /// producer 项目。
    pub project_id: String,
    /// Run 内从 1 开始的连续序号。
    pub seq: u64,
    /// 目录或扩展事件类型。
    pub event_type: String,
    /// producer 声明的发生时间。
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    /// 数据库提交时间。
    #[serde(with = "time::serde::rfc3339")]
    pub recorded_at: OffsetDateTime,
    /// trace 标识。
    pub trace_id: Option<String>,
    /// span 标识。
    pub span_id: Option<String>,
    /// 父 span 标识。
    pub parent_span_id: Option<String>,
    /// 事件载荷。
    pub payload: Value,
}

impl StoredEvent {
    /// 转换为 producer 原始 envelope，供完整事件导出使用。
    pub fn envelope(&self) -> EventEnvelope {
        EventEnvelope {
            seq: self.seq,
            event_type: self.event_type.clone(),
            occurred_at: self.occurred_at,
            trace_id: self.trace_id.clone(),
            span_id: self.span_id.clone(),
            parent_span_id: self.parent_span_id.clone(),
            payload: self.payload.clone(),
        }
    }
}

/// 已提交的 Run 查询投影。
#[derive(Clone, Debug)]
pub struct RunRecord {
    /// Run 标识。
    pub run_id: String,
    /// 来源标识。
    pub source_id: String,
    /// 项目标识。
    pub project_id: String,
    /// Canonical thread 标识。
    pub thread_id: String,
    /// Agent 标识。
    pub agent_id: String,
    /// 当前 Run 状态。
    pub status: RunStatus,
    /// 首条事件时间。
    pub created_at: OffsetDateTime,
    /// Run 开始时间。
    pub started_at: Option<OffsetDateTime>,
    /// Run 终态时间。
    pub finished_at: Option<OffsetDateTime>,
    /// trace 标识。
    pub trace_id: Option<String>,
    /// 已确认的连续序号。
    pub accepted_through_seq: u64,
    /// 最近活动时间。
    pub last_activity_at: OffsetDateTime,
}

/// Run 列表所需的低敏事件活动汇总。
#[derive(Clone, Debug, Default)]
pub struct RunActivity {
    /// 是否存在模型、工具、产物或判定事件。
    pub has_step_events: bool,
    /// 唯一模型调用数。
    pub model_call_count: u32,
    /// 唯一工具调用数。
    pub tool_call_count: u32,
    /// 失败模型、工具或阻断判定数。
    pub error_count: u32,
    /// usage 计数汇总。
    pub usage: BTreeMap<String, u64>,
}

/// Canonical Run 生命周期状态。
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// 尚未开始运行。
    Queued,
    /// 正在运行。
    Running,
    /// 正在取消。
    Cancelling,
    /// 成功结束。
    Succeeded,
    /// 失败结束。
    Failed,
    /// 取消结束。
    Cancelled,
    /// 中断结束。
    Interrupted,
}

impl RunStatus {
    /// 返回数据库与 JSON 契约使用的稳定小写名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }

    /// 判断 Run 是否处于不可再追加新事件的终态。
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }

    /// 从 PostgreSQL 中保存的小写状态名解析领域状态。
    pub fn from_database(status_name: &str) -> Option<Self> {
        match status_name {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "cancelling" => Some(Self::Cancelling),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            "interrupted" => Some(Self::Interrupted),
            _ => None,
        }
    }
}

/// 通用分页查询条件。
#[derive(Clone, Debug, Default)]
pub struct RunQuery {
    /// 精确 Run ID 条件。
    pub run_id: Option<String>,
    /// Agent ID 条件。
    pub agent_id: Option<String>,
    /// Run 状态条件。
    pub status: Option<RunStatus>,
    /// 创建时间下界。
    pub created_after: Option<OffsetDateTime>,
    /// 创建时间上界。
    pub created_before: Option<OffsetDateTime>,
    /// 搜索 source/project/agent/run ID 的文本。
    pub search: Option<String>,
    /// 从 1 开始的页码。
    pub page: u32,
    /// 每页条数。
    pub page_size: u32,
}

/// 会话分页查询条件。
#[derive(Clone, Debug, Default)]
pub struct ThreadQuery {
    /// Agent ID 条件。
    pub agent_id: Option<String>,
    /// 会话包含的 Run 状态条件。
    pub status: Option<RunStatus>,
    /// 创建时间下界。
    pub created_after: Option<OffsetDateTime>,
    /// 创建时间上界。
    pub created_before: Option<OffsetDateTime>,
    /// 搜索 thread/source/project/agent。
    pub search: Option<String>,
    /// 从 1 开始的页码。
    pub page: u32,
    /// 每页条数。
    pub page_size: u32,
}

/// 一页查询结果。
#[derive(Clone, Debug)]
pub struct Page<T> {
    /// 当前页元素。
    pub items: Vec<T>,
    /// 满足筛选的总条数。
    pub total: u64,
    /// 当前页码。
    pub page: u32,
    /// 每页条数。
    pub page_size: u32,
}

/// 事件成功提交后的持久化检查点。
#[derive(Clone, Debug, Serialize)]
pub struct AppendAck {
    /// Run 标识。
    pub run_id: String,
    /// 数据库事务已提交的连续序号。
    pub accepted_through_seq: u64,
    /// 本次请求中识别为精确重试的事件数量。
    pub replayed_events: u32,
}

/// Run 状态摘要及稳定 trace 投影。
#[derive(Clone, Debug, Serialize)]
pub struct TraceSummary {
    /// Run 标识。
    pub run_id: String,
    /// thread 标识。
    pub thread_id: String,
    /// producer 来源。
    pub source_id: String,
    /// producer 项目。
    pub project_id: String,
    /// Agent 标识。
    pub agent_id: String,
    /// Agent 显示名；当前 canonical 事件只提供 agent_id。
    pub agent_name: String,
    /// Run 状态。
    pub status: RunStatus,
    /// 创建时间。
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// 开始时间。
    #[serde(with = "time::serde::rfc3339::option", default)]
    pub started_at: Option<OffsetDateTime>,
    /// 结束时间。
    #[serde(with = "time::serde::rfc3339::option", default)]
    pub finished_at: Option<OffsetDateTime>,
    /// 运行耗时毫秒。
    pub duration_ms: Option<i64>,
    /// trace 标识。
    pub trace_id: String,
    /// 是否有模型/工具 tracing 事件。
    pub visibility: TraceVisibility,
    /// 模型调用数量。
    pub model_call_count: u32,
    /// 工具调用数量。
    pub tool_call_count: u32,
    /// 汇总 usage 计数。
    pub usage: Option<BTreeMap<String, u64>>,
    /// 失败步骤数量。
    pub error_count: u32,
}

/// 轨迹可见程度。
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceVisibility {
    /// 存在模型或工具事件。
    Full,
    /// 仅有历史/边界事件，没有模型或工具节点。
    Limited,
}

/// 轨迹节点类别。
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    /// Run 根节点。
    Root,
    /// 模型调用。
    Model,
    /// 工具调用。
    Tool,
    /// 产物。
    Artifact,
    /// 风险或策略判定。
    Decision,
}

/// 轨迹节点状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanStatus {
    /// 已开始，尚未结束。
    Running,
    /// 成功结束。
    Completed,
    /// 失败或被拦截。
    Failed,
    /// Run 结束时仍缺少完成事件。
    Incomplete,
}

/// 决策事件的完整可见载荷。
#[derive(Clone, Debug, Serialize)]
pub struct DecisionPayload {
    /// 决策标识。
    pub decision_id: String,
    /// 决策 gate。
    pub gate: String,
    /// 被判定工具。
    pub tool_name: String,
    /// 关联的工具调用 ID。
    pub tool_call_ref: String,
    /// 最终裁决。
    pub verdict: String,
    /// 模型回答。
    pub answer: String,
    /// 被选中项概率。
    pub probability: Option<f64>,
    /// 完整概率分布。
    pub probabilities: BTreeMap<String, f64>,
    /// 是否不可逆。
    pub irreversible: Option<bool>,
    /// 是否 fail-open。
    pub fail_open: bool,
    /// 生效策略指纹。
    pub policy_fingerprint: String,
    /// 决策耗时毫秒。
    pub elapsed_ms: u64,
    /// 实际判定输入摘要；来源通过事件 schema 控制可见范围。
    pub judgement_state: String,
}

/// 单个诊断 span。
#[derive(Clone, Debug, Serialize)]
pub struct TraceSpan {
    /// 稳定 span ID。
    pub span_id: String,
    /// 已裁决父级 span ID。
    pub parent_span_id: Option<String>,
    /// span 类别。
    pub kind: SpanKind,
    /// UI 显示名称。
    pub name: String,
    /// 当前状态。
    pub status: SpanStatus,
    /// 开始时间。
    #[serde(with = "time::serde::rfc3339::option", default)]
    pub started_at: Option<OffsetDateTime>,
    /// 结束时间。
    #[serde(with = "time::serde::rfc3339::option", default)]
    pub finished_at: Option<OffsetDateTime>,
    /// 耗时毫秒。
    pub duration_ms: Option<i64>,
    /// 首个来源事件序号。
    pub event_seq: u64,
    /// 模型调用 ID。
    pub model_call_id: Option<String>,
    /// 模型名。
    pub model_name: Option<String>,
    /// 模型轮次。
    pub turn_index: Option<i64>,
    /// usage 计数。
    pub usage: Option<BTreeMap<String, u64>>,
    /// 模型完成原因。
    pub finish_reason: Option<String>,
    /// 工具调用 ID。
    pub tool_call_id: Option<String>,
    /// 工具名称。
    pub tool_name: Option<String>,
    /// 工具原始参数。
    pub tool_arguments: Option<Value>,
    /// 工具原始结果。
    pub tool_result: Option<Value>,
    /// 工具结果 checksum。
    pub result_checksum: Option<String>,
    /// 结果字符数。
    pub result_length: Option<u64>,
    /// 稳定错误码。
    pub error_code: Option<String>,
    /// canonical 错误内容。
    pub error_message: Option<String>,
    /// 节点级诊断码。
    pub diagnostics: Vec<String>,
    /// 完整判定数据。
    pub decision: Option<DecisionPayload>,
}

/// 嵌套的树节点。
#[derive(Clone, Debug, Serialize)]
pub struct TraceTreeNode {
    /// 节点内容。
    pub span: TraceSpan,
    /// 子节点。
    pub children: Vec<TraceTreeNode>,
}

/// 轨迹级诊断信息。
#[derive(Clone, Debug, Serialize)]
pub struct TraceDiagnostic {
    /// 稳定诊断码。
    pub code: String,
    /// 低敏说明。
    pub message: String,
    /// 相关 span。
    pub span_id: Option<String>,
}

/// Run 的完整事件诊断投影。
#[derive(Clone, Debug, Serialize)]
pub struct TraceDetail {
    /// Run 摘要。
    pub summary: TraceSummary,
    /// 根 span ID。
    pub root_span_id: Option<String>,
    /// 扁平节点。
    pub spans: Vec<TraceSpan>,
    /// 嵌套树。
    pub tree: Vec<TraceTreeNode>,
    /// 诊断信息。
    pub diagnostics: Vec<TraceDiagnostic>,
    /// canonical 内容策略标识。
    pub content_policy: String,
}

/// thread 会话摘要。
#[derive(Clone, Debug, Serialize)]
pub struct ThreadSummary {
    /// thread 标识。
    pub thread_id: String,
    /// producer 来源。
    pub source_id: String,
    /// producer 项目。
    pub project_id: String,
    /// thread 中 Run 数。
    pub run_count: u64,
    /// 没有模型/工具 tracing 的 Run 数。
    pub limited_run_count: u64,
    /// Agent 名称列表。
    pub agent_names: Vec<String>,
    /// 各状态 Run 数。
    pub status_counts: BTreeMap<String, u64>,
    /// 首次 Run 创建时间。
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// 最近活动时间。
    #[serde(with = "time::serde::rfc3339")]
    pub last_activity_at: OffsetDateTime,
}

/// thread 账本记录。
#[derive(Clone, Debug, Serialize)]
pub struct ThreadTraceRecord {
    /// 稳定记录 ID。
    pub record_id: String,
    /// 记录类别。
    pub kind: String,
    /// thread 标识。
    pub thread_id: String,
    /// 来源 Run 标识。
    pub run_id: String,
    /// 来源事件序号。
    pub seq: u64,
    /// 发生时间。
    #[serde(with = "time::serde::rfc3339::option", default)]
    pub occurred_at: Option<OffsetDateTime>,
    /// 消息角色。
    pub role: Option<String>,
    /// canonical 消息正文或块数组。
    pub content: Option<Value>,
    /// 关联 span。
    pub span: Option<TraceSpan>,
    /// 委派目标 Run。
    pub child_run_id: Option<String>,
}

/// thread 的完整诊断投影。
#[derive(Clone, Debug, Serialize)]
pub struct ThreadTraceDetail {
    /// thread 摘要。
    pub summary: ThreadSummary,
    /// thread 下的 Run 诊断。
    pub runs: Vec<TraceDetail>,
    /// 统一事件账本。
    pub records: Vec<ThreadTraceRecord>,
}

/// Append 数据存储后的处理结果，含仅供异步导出的新增事件。
#[derive(Clone, Debug)]
pub struct AppendResult {
    /// 持久化检查点。
    pub acknowledgement: AppendAck,
    /// 追加完成后的 Run 状态。
    pub current_status: RunStatus,
    /// 追加后的 Run 查询投影。
    pub run_record: RunRecord,
    /// 本次真正新增的已提交事件；精确重试不包含在内。
    pub inserted_events: Vec<StoredEvent>,
    /// 插入后当前 Run 的事件快照，用于 OTLP 配对和状态导出。
    pub run_events: Vec<StoredEvent>,
}
