//! 用户资源与 Run 产物的 PostgreSQL 实现。

use async_trait::async_trait;
use sqlx::postgres::PgRow;
use time::OffsetDateTime;
use tpl_agent_core::error::AgentError;
use tpl_agent_core::ports::ResourceRepository;
use tpl_agent_core::resources::{Artifact, UserResource};
use tpl_agent_core::time_utils::now;
use uuid::Uuid;

use super::PostgresAgentRepository;
use super::{column, storage_failure};

const RESOURCE_COLUMNS: &str = "id, organization_id, owner_user_id, filename, media_type, \
     size_bytes, sha256, storage_key, created_at, deleted_at";
const ARTIFACT_COLUMNS: &str =
    "id, run_id, owner_user_id, filename, media_type, size_bytes, sha256, storage_key, created_at";

fn resource_from_row(row: &PgRow) -> Result<UserResource, AgentError> {
    Ok(UserResource {
        id: column(row, "id")?,
        organization_id: column(row, "organization_id")?,
        owner_user_id: column(row, "owner_user_id")?,
        filename: column(row, "filename")?,
        media_type: column(row, "media_type")?,
        size_bytes: column(row, "size_bytes")?,
        sha256: column(row, "sha256")?,
        storage_key: column(row, "storage_key")?,
        created_at: column(row, "created_at")?,
        deleted_at: column(row, "deleted_at")?,
    })
}

fn artifact_from_row(row: &PgRow) -> Result<Artifact, AgentError> {
    Ok(Artifact {
        id: column(row, "id")?,
        run_id: column(row, "run_id")?,
        owner_user_id: column(row, "owner_user_id")?,
        filename: column(row, "filename")?,
        media_type: column(row, "media_type")?,
        size_bytes: column(row, "size_bytes")?,
        sha256: column(row, "sha256")?,
        storage_key: column(row, "storage_key")?,
        created_at: column(row, "created_at")?,
    })
}

#[async_trait]
impl ResourceRepository for PostgresAgentRepository {
    async fn insert_resource(&self, resource: &UserResource) -> Result<(), AgentError> {
        sqlx::query(
            "INSERT INTO user_resources
                 (id, organization_id, owner_user_id, filename, media_type, size_bytes, sha256,
                  storage_key, created_at, deleted_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(resource.id)
        .bind(resource.organization_id)
        .bind(resource.owner_user_id)
        .bind(&resource.filename)
        .bind(&resource.media_type)
        .bind(resource.size_bytes)
        .bind(&resource.sha256)
        .bind(&resource.storage_key)
        .bind(resource.created_at)
        .bind(resource.deleted_at)
        .execute(&self.pool)
        .await
        .map_err(storage_failure)?;
        Ok(())
    }

    async fn get_resource(&self, resource_id: Uuid) -> Result<Option<UserResource>, AgentError> {
        let resource_row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {RESOURCE_COLUMNS} FROM user_resources WHERE id = $1"
        )))
        .bind(resource_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failure)?;
        resource_row.as_ref().map(resource_from_row).transpose()
    }

    async fn list_resources(
        &self,
        organization_id: Uuid,
        owner_user_id: i64,
    ) -> Result<Vec<UserResource>, AgentError> {
        let resource_rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {RESOURCE_COLUMNS} FROM user_resources
             WHERE organization_id = $1 AND owner_user_id = $2 AND deleted_at IS NULL
             ORDER BY created_at DESC, id"
        )))
        .bind(organization_id)
        .bind(owner_user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)?;
        resource_rows.iter().map(resource_from_row).collect()
    }

    async fn soft_delete_resource(
        &self,
        organization_id: Uuid,
        owner_user_id: i64,
        resource_id: Uuid,
    ) -> Result<(), AgentError> {
        let deleted_at: OffsetDateTime = now();
        let update_result = sqlx::query(
            "UPDATE user_resources SET deleted_at = $4
             WHERE id = $1 AND organization_id = $2 AND owner_user_id = $3
               AND deleted_at IS NULL",
        )
        .bind(resource_id)
        .bind(organization_id)
        .bind(owner_user_id)
        .bind(deleted_at)
        .execute(&self.pool)
        .await
        .map_err(storage_failure)?;
        if update_result.rows_affected() == 0 {
            return Err(AgentError::NotFound("resource_not_found"));
        }
        Ok(())
    }

    async fn get_artifact(&self, artifact_id: Uuid) -> Result<Option<Artifact>, AgentError> {
        let artifact_row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {ARTIFACT_COLUMNS} FROM run_artifacts WHERE id = $1"
        )))
        .bind(artifact_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failure)?;
        artifact_row.as_ref().map(artifact_from_row).transpose()
    }

    async fn list_artifacts_for_run(&self, run_id: Uuid) -> Result<Vec<Artifact>, AgentError> {
        let artifact_rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {ARTIFACT_COLUMNS} FROM run_artifacts
             WHERE run_id = $1 ORDER BY created_at, id"
        )))
        .bind(run_id)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)?;
        artifact_rows.iter().map(artifact_from_row).collect()
    }
}
