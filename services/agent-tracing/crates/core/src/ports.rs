//! Core 定义的存储、身份校验与可选 OTLP 出口端口。

use async_trait::async_trait;
use std::collections::BTreeMap;
use time::OffsetDateTime;

use crate::event::EventEnvelope;
use crate::model::{
    AppendResult, Page, ProducerIdentity, ProducerRegistration, RunActivity, RunQuery, RunRecord,
    StoredEvent, ThreadQuery, ThreadSummary,
};

/// Run/Event 存储错误。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TraceStoreError {
    /// producer token 未登记或已停用。
    #[error("producer_unauthorized")]
    ProducerUnauthorized,
    /// 目标 Run 或 thread 不存在。
    #[error("not_found")]
    NotFound,
    /// 请求序号存在未填补的缺口。
    #[error("sequence_gap")]
    SequenceGap {
        /// 服务端下一条期望的序号。
        expected_seq: u64,
    },
    /// 已存在序号携带不同事件内容。
    #[error("sequence_conflict")]
    SequenceConflict {
        /// 内容冲突的序号。
        seq: u64,
    },
    /// 生命周期状态迁移不合法。
    #[error("run_state_conflict")]
    RunStateConflict,
    /// 终态 Run 不再接受新事件。
    #[error("run_terminal")]
    RunTerminal,
    /// 同一 Run 的 trace ID 不一致。
    #[error("trace_id_conflict")]
    TraceIdConflict,
    /// 一个 thread ID 已归属其他 source。
    #[error("thread_source_conflict")]
    ThreadSourceConflict,
    /// 导出要求 Run 已终结且事件连续。
    #[error("export_not_ready")]
    ExportNotReady,
    /// 导出超出条数或字节上限。
    #[error("export_too_large")]
    ExportTooLarge,
    /// 存储后端失败；内部原因只可在服务端受控记录。
    #[error("storage_unavailable")]
    StorageUnavailable,
}

/// 导出审计所需的低敏元数据。
#[derive(Clone, Debug)]
pub struct ExportAudit {
    /// 被导出的 Run。
    pub run_id: String,
    /// 已由 accounts-service 验证的管理员 ID。
    pub admin_user_id: String,
    /// 导出事件数。
    pub event_count: u32,
    /// 导出文件字节数。
    pub byte_count: u64,
    /// 导出准备时间。
    pub exported_at: OffsetDateTime,
}

/// Agent tracing 的持久化端口。
#[async_trait]
pub trait TraceRepository: Send + Sync {
    /// 检查 tracing 数据库是否可用。
    async fn check_readiness(&self) -> Result<(), TraceStoreError>;

    /// 按配置登记 producer 来源，只持久化 token 摘要。
    async fn register_producers(
        &self,
        registrations: &[ProducerRegistration],
    ) -> Result<(), TraceStoreError>;

    /// 按 bearer token 摘要解析固定来源与项目身份。
    async fn authenticate_producer(
        &self,
        token_hash: &[u8],
    ) -> Result<ProducerIdentity, TraceStoreError>;

    /// 原子写入事件批次并返回 durable checkpoint。
    async fn append_batch(
        &self,
        producer: &ProducerIdentity,
        run_id: &str,
        events: &[EventEnvelope],
    ) -> Result<AppendResult, TraceStoreError>;

    /// 分页查询管理员可见的全部 Run。
    async fn list_runs(&self, query: &RunQuery) -> Result<Page<RunRecord>, TraceStoreError>;

    /// 批量读取 Run 列表需要的活动计数，不加载消息或工具原文。
    async fn summarize_run_activity(
        &self,
        run_ids: &[String],
    ) -> Result<BTreeMap<String, RunActivity>, TraceStoreError>;

    /// 按全局唯一 Run ID 读取 Run 和完整事件序列。
    async fn get_run(&self, run_id: &str)
    -> Result<(RunRecord, Vec<StoredEvent>), TraceStoreError>;

    /// 分页查询 thread 会话。
    async fn list_threads(
        &self,
        query: &ThreadQuery,
    ) -> Result<Page<ThreadSummary>, TraceStoreError>;

    /// 按全局唯一 thread ID 读取其 Run 和事件。
    async fn get_thread(
        &self,
        thread_id: &str,
    ) -> Result<Vec<(RunRecord, Vec<StoredEvent>)>, TraceStoreError>;

    /// 成功准备导出响应后追加一条低敏审计记录。
    async fn record_export(&self, audit: &ExportAudit) -> Result<(), TraceStoreError>;
}

/// 已验证的管理员身份。
#[derive(Clone, Debug)]
pub struct AdminIdentity {
    /// accounts-service 响应里的管理员 `user_id`。
    pub user_id: String,
}

/// accounts-service 管理员会话验证端口。
#[async_trait]
pub trait AdminSessionValidator: Send + Sync {
    /// 通过 accounts 的公开 `/admin/auth/me` 契约验证管理员 Cookie。
    async fn authenticate(
        &self,
        cookie_header: Option<&str>,
    ) -> Result<AdminIdentity, AdminAuthError>;
}

/// 管理员认证错误。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AdminAuthError {
    /// 未提供有效管理员 Cookie。
    #[error("admin_unauthenticated")]
    Unauthenticated,
    /// accounts-service 当前不可用。
    #[error("admin_auth_unavailable")]
    Unavailable,
}

/// OTLP 最佳努力出口错误。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("OTLP 导出失败")]
pub struct OtlpExportError;

/// 可选的 OTLP/HTTP span 导出端口。
#[async_trait]
pub trait OtlpExporter: Send + Sync {
    /// 导出已提交事件形成的 span；实现必须隔离失败，不回滚事件提交。
    async fn export_committed_events(
        &self,
        append_result: &AppendResult,
    ) -> Result<(), OtlpExportError>;
}

/// 管理员过滤查询的标准化文本搜索。
pub fn normalized_search_value(raw_search: Option<&str>) -> Option<String> {
    raw_search
        .map(str::trim)
        .filter(|search_text| !search_text.is_empty())
        .map(|search_text| search_text.chars().take(128).collect())
}
