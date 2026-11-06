//! 审计记录的 PostgreSQL 实现。

use async_trait::async_trait;
use sqlx::postgres::PgRow;
use tpl_agent_core::audit::{AuditOutcome, AuditRecord, NewAuditRecord};
use tpl_agent_core::error::AgentError;
use tpl_agent_core::ports::AuditRepository;
use uuid::Uuid;

use super::PostgresAgentRepository;
use super::{audit_detail_or_empty, column, parse_database_enum, storage_failure};

fn audit_record_from_row(row: &PgRow) -> Result<AuditRecord, AgentError> {
    let outcome_text: String = column(row, "outcome")?;
    Ok(AuditRecord {
        id: column(row, "id")?,
        occurred_at: column(row, "occurred_at")?,
        service_id: column(row, "service_id")?,
        admin_user_id: column(row, "admin_user_id")?,
        user_id: column(row, "user_id")?,
        organization_id: column(row, "organization_id")?,
        action: column(row, "action")?,
        target_id: column(row, "target_id")?,
        outcome: parse_database_enum(&outcome_text, AuditOutcome::from_database)?,
        detail: column(row, "detail")?,
    })
}

#[async_trait]
impl AuditRepository for PostgresAgentRepository {
    async fn append(&self, record: &NewAuditRecord) -> Result<(), AgentError> {
        sqlx::query(
            "INSERT INTO agent_audit_records
                 (occurred_at, service_id, admin_user_id, user_id, organization_id, action,
                  target_id, outcome, detail)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(record.occurred_at)
        .bind(&record.service_id)
        .bind(record.admin_user_id)
        .bind(record.user_id)
        .bind(record.organization_id)
        .bind(&record.action)
        .bind(&record.target_id)
        .bind(record.outcome.as_str())
        .bind(audit_detail_or_empty(&record.detail))
        .execute(&self.pool)
        .await
        .map_err(storage_failure)?;
        Ok(())
    }

    async fn list(
        &self,
        organization_id: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<AuditRecord>, AgentError> {
        let limit = i64::try_from(limit).map_err(|_| AgentError::Internal)?;
        let record_rows = sqlx::query(
            "SELECT id, occurred_at, service_id, admin_user_id, user_id, organization_id, action,
                    target_id, outcome, detail
             FROM agent_audit_records
             WHERE ($1::uuid IS NULL AND organization_id IS NULL)
                OR organization_id = $1
             ORDER BY id DESC
             LIMIT $2",
        )
        .bind(organization_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)?;
        record_rows.iter().map(audit_record_from_row).collect()
    }
}
