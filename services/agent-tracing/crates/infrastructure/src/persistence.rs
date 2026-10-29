//! Agent tracing PostgreSQL 持久化入口与连接生命周期。

use std::time::Duration;

use sqlx::postgres::{PgPool, PgPoolOptions};
use tpl_agent_tracing_core::event::EventEnvelope;
use tpl_agent_tracing_core::model::{
    AppendResult, Page, ProducerIdentity, ProducerRegistration, RunActivity, RunQuery, RunRecord,
    StoredEvent, ThreadQuery, ThreadSummary,
};
use tpl_agent_tracing_core::ports::{ExportAudit, TraceRepository, TraceStoreError};

mod append;
mod queries;

/// Agent tracing 服务独占的 PostgreSQL repository。
#[derive(Clone)]
pub struct PostgresTraceRepository {
    pub(super) pool: PgPool,
}

impl PostgresTraceRepository {
    /// 使用 tracing 专属连接串创建连接池。
    ///
    /// # Errors
    ///
    /// 数据库地址格式错误或连接建立失败时返回 SQLx 错误。
    pub async fn connect(
        database_url: &str,
        max_connections: u32,
        acquire_timeout: Duration,
    ) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections.clamp(1, 100))
            .acquire_timeout(acquire_timeout)
            .connect(database_url)
            .await?;
        Ok(Self { pool })
    }

    /// 执行 tracing 服务自己的迁移目录。
    ///
    /// # Errors
    ///
    /// 数据库不可达、迁移 SQL 出错或迁移校验和不一致时返回错误。
    pub async fn run_migrations(&self) -> Result<(), sqlx::migrate::MigrateError> {
        sqlx::migrate!("../../migrations").run(&self.pool).await
    }

    /// 执行有超时的 PostgreSQL readiness 探测。
    pub async fn check_readiness(&self, probe_timeout: Duration) -> Result<(), sqlx::Error> {
        let database_probe = sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&self.pool);
        match tokio::time::timeout(probe_timeout, database_probe).await {
            Ok(probe_result) => probe_result.map(|_| ()),
            Err(_elapsed) => Err(sqlx::Error::PoolTimedOut),
        }
    }

    /// 关闭连接池并等待连接释放。
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

#[async_trait::async_trait]
impl TraceRepository for PostgresTraceRepository {
    async fn check_readiness(&self) -> Result<(), TraceStoreError> {
        PostgresTraceRepository::check_readiness(self, Duration::from_secs(3))
            .await
            .map_err(|database_error| {
                tracing::warn!(error = %database_error, "tracing 数据库 readiness 检查失败");
                TraceStoreError::StorageUnavailable
            })
    }

    async fn register_producers(
        &self,
        registrations: &[ProducerRegistration],
    ) -> Result<(), TraceStoreError> {
        append::register_producers(&self.pool, registrations).await
    }

    async fn authenticate_producer(
        &self,
        token_hash: &[u8],
    ) -> Result<ProducerIdentity, TraceStoreError> {
        append::authenticate_producer(&self.pool, token_hash).await
    }

    async fn append_batch(
        &self,
        producer: &ProducerIdentity,
        run_id: &str,
        events: &[EventEnvelope],
    ) -> Result<AppendResult, TraceStoreError> {
        append::append_batch(&self.pool, producer, run_id, events).await
    }

    async fn list_runs(&self, query: &RunQuery) -> Result<Page<RunRecord>, TraceStoreError> {
        queries::list_runs(&self.pool, query).await
    }

    async fn summarize_run_activity(
        &self,
        run_ids: &[String],
    ) -> Result<std::collections::BTreeMap<String, RunActivity>, TraceStoreError> {
        queries::summarize_run_activity(&self.pool, run_ids).await
    }

    async fn get_run(
        &self,
        run_id: &str,
    ) -> Result<(RunRecord, Vec<StoredEvent>), TraceStoreError> {
        queries::get_run(&self.pool, run_id).await
    }

    async fn list_threads(
        &self,
        query: &ThreadQuery,
    ) -> Result<Page<ThreadSummary>, TraceStoreError> {
        queries::list_threads(&self.pool, query).await
    }

    async fn get_thread(
        &self,
        thread_id: &str,
    ) -> Result<Vec<(RunRecord, Vec<StoredEvent>)>, TraceStoreError> {
        queries::get_thread(&self.pool, thread_id).await
    }

    async fn record_export(&self, audit: &ExportAudit) -> Result<(), TraceStoreError> {
        queries::record_export(&self.pool, audit).await
    }
}
