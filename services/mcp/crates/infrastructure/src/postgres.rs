//! MCP 服务独占 PostgreSQL 持久化：连接、凭据密文与授权分配。
//!
//! 所有 SQL 使用运行时查询；本库不依赖 accounts、Agent 或 tracing 的表。

use std::time::Duration;

use async_trait::async_trait;
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{PgPool, Row};
use tpl_mcp_core::{
    Assignment, AssignmentSubject, AuthType, Connection, ConnectionRepository, ConnectionStatus,
    CredentialCiphertext, GroupId, ReadinessError, ReadinessProbe, RepositoryError, SubjectId,
    TenantScope,
};
use url::Url;
use uuid::Uuid;

/// MCP 服务 PostgreSQL repository。
#[derive(Clone)]
pub struct PostgresMcpRepository {
    pool: PgPool,
}

impl PostgresMcpRepository {
    /// 使用 MCP 专属连接串创建连接池。
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

    /// 执行 MCP 服务自己的迁移目录。
    ///
    /// # Errors
    ///
    /// 数据库不可达、迁移 SQL 出错或迁移校验和不一致时返回错误。
    pub async fn run_migrations(&self) -> Result<(), sqlx::migrate::MigrateError> {
        sqlx::migrate!("../../migrations").run(&self.pool).await
    }

    /// 关闭连接池并等待连接释放。
    pub async fn close(&self) {
        self.pool.close().await;
    }

    async fn ensure_connection_exists(
        &self,
        scope: &TenantScope,
        connection_id: Uuid,
    ) -> Result<(), RepositoryError> {
        self.find(scope, connection_id).await.map(|_| ())
    }
}

#[async_trait]
impl ConnectionRepository for PostgresMcpRepository {
    async fn create(&self, connection: &Connection) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO mcp_connections (
                id, consumer_id, tenant_id, name, endpoint_url, auth_type,
                credential_ciphertext, status, created_by, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(connection.id)
        .bind(connection.scope.consumer_id.as_str())
        .bind(connection.scope.tenant_id.as_str())
        .bind(&connection.name)
        .bind(connection.endpoint_url.as_str())
        .bind(connection.auth_type.as_str())
        .bind(connection.credential.expose().map(<[u8]>::to_vec))
        .bind(connection.status.as_str())
        .bind(connection.created_by.as_str())
        .bind(connection.created_at)
        .bind(connection.updated_at)
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(map_write_error)
    }

    async fn update(&self, connection: &Connection) -> Result<(), RepositoryError> {
        let result = sqlx::query(
            "UPDATE mcp_connections
             SET name = $1, endpoint_url = $2, auth_type = $3, credential_ciphertext = $4,
                 status = $5, updated_at = $6
             WHERE id = $7 AND consumer_id = $8 AND tenant_id = $9",
        )
        .bind(&connection.name)
        .bind(connection.endpoint_url.as_str())
        .bind(connection.auth_type.as_str())
        .bind(connection.credential.expose().map(<[u8]>::to_vec))
        .bind(connection.status.as_str())
        .bind(connection.updated_at)
        .bind(connection.id)
        .bind(connection.scope.consumer_id.as_str())
        .bind(connection.scope.tenant_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(map_write_error)?;
        if result.rows_affected() == 0 {
            return Err(RepositoryError::NotFound);
        }
        Ok(())
    }

    async fn delete(
        &self,
        scope: &TenantScope,
        connection_id: Uuid,
    ) -> Result<(), RepositoryError> {
        let result = sqlx::query(
            "DELETE FROM mcp_connections
             WHERE id = $1 AND consumer_id = $2 AND tenant_id = $3",
        )
        .bind(connection_id)
        .bind(scope.consumer_id.as_str())
        .bind(scope.tenant_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(map_write_error)?;
        if result.rows_affected() == 0 {
            return Err(RepositoryError::NotFound);
        }
        Ok(())
    }

    async fn find(
        &self,
        scope: &TenantScope,
        connection_id: Uuid,
    ) -> Result<Connection, RepositoryError> {
        let row = sqlx::query(
            "SELECT id, consumer_id, tenant_id, name, endpoint_url, auth_type,
                    credential_ciphertext, status, created_by, created_at, updated_at
             FROM mcp_connections
             WHERE id = $1 AND consumer_id = $2 AND tenant_id = $3",
        )
        .bind(connection_id)
        .bind(scope.consumer_id.as_str())
        .bind(scope.tenant_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|error| match error {
            sqlx::Error::RowNotFound => RepositoryError::NotFound,
            other => map_read_error(other),
        })?;
        connection_from_row(&row)
    }

    async fn list(&self, scope: &TenantScope) -> Result<Vec<Connection>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, consumer_id, tenant_id, name, endpoint_url, auth_type,
                    credential_ciphertext, status, created_by, created_at, updated_at
             FROM mcp_connections
             WHERE consumer_id = $1 AND tenant_id = $2
             ORDER BY name ASC, id ASC",
        )
        .bind(scope.consumer_id.as_str())
        .bind(scope.tenant_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(map_read_error)?;
        rows.iter().map(connection_from_row).collect()
    }

    async fn list_authorized(
        &self,
        scope: &TenantScope,
        subjects: &[AssignmentSubject],
    ) -> Result<Vec<Connection>, RepositoryError> {
        let (user_ids, group_ids) = split_subjects(subjects);
        let rows = sqlx::query(
            "SELECT c.id, c.consumer_id, c.tenant_id, c.name, c.endpoint_url, c.auth_type,
                    c.credential_ciphertext, c.status, c.created_by, c.created_at, c.updated_at
             FROM mcp_connections c
             WHERE c.consumer_id = $1 AND c.tenant_id = $2 AND c.status = 'active'
               AND EXISTS (
                   SELECT 1 FROM mcp_connection_assignments a
                   WHERE a.connection_id = c.id
                     AND (
                         (a.subject_type = 'user' AND a.subject_id = ANY($3))
                         OR (a.subject_type = 'group' AND a.subject_id = ANY($4))
                     )
               )
             ORDER BY c.name ASC, c.id ASC",
        )
        .bind(scope.consumer_id.as_str())
        .bind(scope.tenant_id.as_str())
        .bind(user_ids)
        .bind(group_ids)
        .fetch_all(&self.pool)
        .await
        .map_err(map_read_error)?;
        rows.iter().map(connection_from_row).collect()
    }

    async fn is_authorized(
        &self,
        scope: &TenantScope,
        connection_id: Uuid,
        subjects: &[AssignmentSubject],
    ) -> Result<bool, RepositoryError> {
        let (user_ids, group_ids) = split_subjects(subjects);
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM mcp_connections c
                WHERE c.id = $1 AND c.consumer_id = $2 AND c.tenant_id = $3
                  AND c.status = 'active'
                  AND EXISTS (
                      SELECT 1 FROM mcp_connection_assignments a
                      WHERE a.connection_id = c.id
                        AND (
                            (a.subject_type = 'user' AND a.subject_id = ANY($4))
                            OR (a.subject_type = 'group' AND a.subject_id = ANY($5))
                        )
                  )
             )",
        )
        .bind(connection_id)
        .bind(scope.consumer_id.as_str())
        .bind(scope.tenant_id.as_str())
        .bind(user_ids)
        .bind(group_ids)
        .fetch_one(&self.pool)
        .await
        .map_err(map_read_error)
    }

    async fn list_assignments(
        &self,
        scope: &TenantScope,
        connection_id: Uuid,
    ) -> Result<Vec<Assignment>, RepositoryError> {
        self.ensure_connection_exists(scope, connection_id).await?;
        let rows = sqlx::query(
            "SELECT connection_id, subject_type, subject_id
             FROM mcp_connection_assignments
             WHERE connection_id = $1
             ORDER BY subject_type ASC, subject_id ASC",
        )
        .bind(connection_id)
        .fetch_all(&self.pool)
        .await
        .map_err(map_read_error)?;
        rows.iter().map(assignment_from_row).collect()
    }

    async fn add_assignment(
        &self,
        scope: &TenantScope,
        assignment: &Assignment,
    ) -> Result<(), RepositoryError> {
        self.ensure_connection_exists(scope, assignment.connection_id)
            .await?;
        sqlx::query(
            "INSERT INTO mcp_connection_assignments (
                id, connection_id, subject_type, subject_id, created_at
             ) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (connection_id, subject_type, subject_id) DO NOTHING",
        )
        .bind(Uuid::new_v4())
        .bind(assignment.connection_id)
        .bind(assignment.subject.subject_type().as_str())
        .bind(assignment.subject.subject_id())
        .bind(time::OffsetDateTime::now_utc())
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(map_write_error)
    }

    async fn remove_assignment(
        &self,
        scope: &TenantScope,
        connection_id: Uuid,
        subject: &AssignmentSubject,
    ) -> Result<(), RepositoryError> {
        self.ensure_connection_exists(scope, connection_id).await?;
        sqlx::query(
            "DELETE FROM mcp_connection_assignments
             WHERE connection_id = $1 AND subject_type = $2 AND subject_id = $3",
        )
        .bind(connection_id)
        .bind(subject.subject_type().as_str())
        .bind(subject.subject_id())
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(map_write_error)
    }
}

#[async_trait]
impl ReadinessProbe for PostgresMcpRepository {
    async fn check_ready(&self) -> Result<(), ReadinessError> {
        let probe = sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&self.pool);
        match tokio::time::timeout(Duration::from_secs(2), probe).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "MCP 数据库 readiness 探测失败");
                Err(ReadinessError {
                    dependency: "database",
                })
            }
            Err(_) => Err(ReadinessError {
                dependency: "database",
            }),
        }
    }
}

/// 把授权主体拆成用户 ID 与分组 ID 两组，供 `= ANY` 数组参数使用。
fn split_subjects(subjects: &[AssignmentSubject]) -> (Vec<String>, Vec<String>) {
    let mut user_ids = Vec::new();
    let mut group_ids = Vec::new();
    for subject in subjects {
        match subject {
            AssignmentSubject::User(subject_id) => user_ids.push(subject_id.as_str().to_owned()),
            AssignmentSubject::Group(group_id) => group_ids.push(group_id.as_str().to_owned()),
        }
    }
    (user_ids, group_ids)
}

fn connection_from_row(row: &PgRow) -> Result<Connection, RepositoryError> {
    let id: Uuid = row.try_get("id").map_err(invalid_stored)?;
    let consumer_id: String = row.try_get("consumer_id").map_err(invalid_stored)?;
    let tenant_id: String = row.try_get("tenant_id").map_err(invalid_stored)?;
    let name: String = row.try_get("name").map_err(invalid_stored)?;
    let endpoint_url: String = row.try_get("endpoint_url").map_err(invalid_stored)?;
    let auth_type: String = row.try_get("auth_type").map_err(invalid_stored)?;
    let credential_ciphertext: Option<Vec<u8>> = row
        .try_get("credential_ciphertext")
        .map_err(invalid_stored)?;
    let status: String = row.try_get("status").map_err(invalid_stored)?;
    let created_by: String = row.try_get("created_by").map_err(invalid_stored)?;
    let created_at = row.try_get("created_at").map_err(invalid_stored)?;
    let updated_at = row.try_get("updated_at").map_err(invalid_stored)?;

    let scope = TenantScope {
        consumer_id: tpl_mcp_core::ConsumerId::new(consumer_id).map_err(invalid_stored)?,
        tenant_id: tpl_mcp_core::TenantId::new(tenant_id).map_err(invalid_stored)?,
    };
    let endpoint_url = Url::parse(&endpoint_url).map_err(|_| invalid_stored_record())?;
    let auth_type = AuthType::parse(&auth_type).ok_or_else(invalid_stored_record)?;
    let status = ConnectionStatus::parse(&status).ok_or_else(invalid_stored_record)?;
    let created_by = SubjectId::new(created_by).map_err(invalid_stored)?;
    let credential = match credential_ciphertext {
        Some(ciphertext) => CredentialCiphertext::new(ciphertext),
        None => CredentialCiphertext::none(),
    };
    Ok(Connection {
        id,
        scope,
        name,
        endpoint_url,
        auth_type,
        credential,
        status,
        created_by,
        created_at,
        updated_at,
    })
}

fn assignment_from_row(row: &PgRow) -> Result<Assignment, RepositoryError> {
    let connection_id: Uuid = row.try_get("connection_id").map_err(invalid_stored)?;
    let subject_type: String = row.try_get("subject_type").map_err(invalid_stored)?;
    let subject_id: String = row.try_get("subject_id").map_err(invalid_stored)?;
    let subject = match subject_type.as_str() {
        "user" => AssignmentSubject::User(SubjectId::new(subject_id).map_err(invalid_stored)?),
        "group" => AssignmentSubject::Group(GroupId::new(subject_id).map_err(invalid_stored)?),
        _ => return Err(invalid_stored_record()),
    };
    Ok(Assignment {
        connection_id,
        subject,
    })
}

/// 存储记录不符合领域契约；不回显字段值。
#[derive(Debug, thiserror::Error)]
#[error("存储记录不符合 MCP 连接契约")]
struct InvalidStoredRecord;

fn invalid_stored_record() -> RepositoryError {
    RepositoryError::Other(Box::new(InvalidStoredRecord))
}

fn invalid_stored<E>(_error: E) -> RepositoryError {
    invalid_stored_record()
}

fn map_read_error(error: sqlx::Error) -> RepositoryError {
    match error {
        sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed | sqlx::Error::Io(_) => {
            RepositoryError::Unavailable(Box::new(error))
        }
        other => RepositoryError::Other(Box::new(other)),
    }
}

fn map_write_error(error: sqlx::Error) -> RepositoryError {
    if matches!(
        &error,
        sqlx::Error::Database(database_error) if database_error.is_unique_violation()
    ) {
        return RepositoryError::Conflict;
    }
    map_read_error(error)
}
