//! Agent 服务独占 PostgreSQL 持久化入口与共享行映射。
//!
//! 所有 SQL 都使用运行时查询：服务拥有独立数据库与迁移目录，不依赖 accounts 的表。
//! 仓储 trait 的每个实现位于对应子模块，共享事务、约束映射与解码辅助。

mod audit;
mod catalog;
mod conversation;
mod directory;
mod resources;
mod runs;

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{PgPool, Postgres, Row};
use tpl_agent_core::error::AgentError;

/// Agent 服务 PostgreSQL repository。
#[derive(Clone)]
pub struct PostgresAgentRepository {
    pool: PgPool,
}

impl PostgresAgentRepository {
    /// 使用 Agent 专属连接串创建连接池。
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

    /// 执行 Agent 服务自己的迁移目录。
    ///
    /// # Errors
    ///
    /// 数据库不可达、迁移 SQL 出错或迁移校验和不一致时返回错误。
    pub async fn run_migrations(&self) -> Result<(), sqlx::migrate::MigrateError> {
        sqlx::migrate!("../../migrations").run(&self.pool).await
    }

    /// 执行有超时的 PostgreSQL readiness 探测。
    ///
    /// # Errors
    ///
    /// 数据库不可达或探测超时。
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
impl tpl_agent_core::ports::ReadinessProbe for PostgresAgentRepository {
    async fn check_ready(&self) -> Result<(), AgentError> {
        self.check_readiness(Duration::from_secs(2))
            .await
            .map_err(|probe_error| {
                tracing::warn!(error = %probe_error, "Agent 数据库 readiness 探测失败");
                AgentError::StorageUnavailable
            })
    }
}

/// 读取一列；解码失败视为存储契约破坏，不向调用方暴露列名。
fn column<'row, T>(row: &'row PgRow, column_name: &'static str) -> Result<T, AgentError>
where
    T: sqlx::Decode<'row, Postgres> + sqlx::Type<Postgres>,
{
    row.try_get::<T, _>(column_name).map_err(|decode_error| {
        tracing::warn!(
            column = column_name,
            error = %decode_error,
            "读取 Agent 数据列失败"
        );
        AgentError::StorageUnavailable
    })
}

/// 数据库错误统一映射；SQL 细节只进入日志。
fn storage_failure(error: sqlx::Error) -> AgentError {
    tracing::warn!(error = %error, "Agent 数据库操作失败");
    AgentError::StorageUnavailable
}

/// 判断是否为指定唯一约束冲突。
fn is_unique_violation(error: &sqlx::Error, constraint_name: &str) -> bool {
    matches!(
        error,
        sqlx::Error::Database(database_error)
            if database_error.is_unique_violation()
                && database_error.constraint() == Some(constraint_name)
    )
}

/// 判断是否为指定外键约束冲突。
fn is_foreign_key_violation(error: &sqlx::Error, constraint_name: &str) -> bool {
    matches!(
        error,
        sqlx::Error::Database(database_error)
            if database_error.is_foreign_key_violation()
                && database_error.constraint() == Some(constraint_name)
    )
}

/// 将 UUID 列表编码为 JSONB 数组。
fn uuid_array_json(identifiers: &[uuid::Uuid]) -> Value {
    Value::Array(
        identifiers
            .iter()
            .map(|identifier| Value::String(identifier.to_string()))
            .collect(),
    )
}

/// 从 JSONB 数组解出 UUID 列表。
fn uuids_from_json(value: &Value) -> Result<Vec<uuid::Uuid>, AgentError> {
    let Value::Array(items) = value else {
        return Err(AgentError::StorageUnavailable);
    };
    items
        .iter()
        .map(|item| {
            item.as_str()
                .and_then(|text| text.parse::<uuid::Uuid>().ok())
                .ok_or(AgentError::StorageUnavailable)
        })
        .collect()
}

/// 读取消息的 `resource_ids` JSONB 数组。
fn uuid_list_from_column(
    row: &PgRow,
    column_name: &'static str,
) -> Result<Vec<uuid::Uuid>, AgentError> {
    let raw_value = column::<Value>(row, column_name)?;
    uuids_from_json(&raw_value)
}

/// 反序列化持久化的 JSONB 结构。
fn deserialize_column<T: DeserializeOwned>(
    raw_value: Value,
    structure_name: &'static str,
) -> Result<T, AgentError> {
    serde_json::from_value::<T>(raw_value).map_err(|deserialize_error| {
        tracing::warn!(
            structure = structure_name,
            error = %deserialize_error,
            "持久化 JSON 结构无法反序列化"
        );
        AgentError::StorageUnavailable
    })
}

/// 序列化写入 JSONB 的结构。
fn serialize_structure<T: serde::Serialize>(value: &T) -> Result<Value, AgentError> {
    serde_json::to_value(value).map_err(|serialize_error| {
        tracing::warn!(error = %serialize_error, "数据结构无法序列化为 JSONB");
        AgentError::Internal
    })
}

/// 将解析出的数据库字符串映射回枚举。
fn parse_database_enum<T>(
    value: &str,
    parser: impl FnOnce(&str) -> Option<T>,
) -> Result<T, AgentError> {
    parser(value).ok_or_else(|| {
        tracing::warn!(value, "数据库枚举值不在已知范围");
        AgentError::StorageUnavailable
    })
}

/// 审计详情为空对象时保持 `{}`，避免写入非对象 JSON。
fn audit_detail_or_empty(detail: &Value) -> Value {
    if detail.is_object() {
        detail.clone()
    } else {
        json!({})
    }
}
