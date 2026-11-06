//! Session 与消息的 PostgreSQL 实现。

use async_trait::async_trait;
use sqlx::postgres::PgRow;
use tpl_agent_core::error::AgentError;
use tpl_agent_core::ports::ConversationRepository;
use tpl_agent_core::run::RunRecord;
use tpl_agent_core::session::{ChatMessage, ChatSession, MessageRole};
use uuid::Uuid;

use super::PostgresAgentRepository;
use super::runs::{RUN_COLUMNS, run_from_row};
use super::{
    column, is_foreign_key_violation, parse_database_enum, storage_failure, uuid_list_from_column,
};

const SESSION_COLUMNS: &str =
    "id, organization_id, owner_user_id, agent_id, title, created_at, last_activity_at";
const MESSAGE_COLUMNS: &str = "id, session_id, run_id, role, content, resource_ids, created_at";

fn session_from_row(row: &PgRow) -> Result<ChatSession, AgentError> {
    Ok(ChatSession {
        id: column(row, "id")?,
        organization_id: column(row, "organization_id")?,
        owner_user_id: column(row, "owner_user_id")?,
        agent_id: column(row, "agent_id")?,
        title: column(row, "title")?,
        created_at: column(row, "created_at")?,
        last_activity_at: column(row, "last_activity_at")?,
    })
}

fn message_from_row(row: &PgRow) -> Result<ChatMessage, AgentError> {
    let role_text: String = column(row, "role")?;
    Ok(ChatMessage {
        id: column(row, "id")?,
        session_id: column(row, "session_id")?,
        run_id: column(row, "run_id")?,
        role: parse_database_enum(&role_text, MessageRole::from_database)?,
        content: column(row, "content")?,
        resource_ids: uuid_list_from_column(row, "resource_ids")?,
        created_at: column(row, "created_at")?,
    })
}

#[async_trait]
impl ConversationRepository for PostgresAgentRepository {
    async fn insert_session(&self, session: &ChatSession) -> Result<(), AgentError> {
        let insert_result = sqlx::query(
            "INSERT INTO chat_sessions
                 (id, organization_id, owner_user_id, agent_id, title, created_at, last_activity_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(session.id)
        .bind(session.organization_id)
        .bind(session.owner_user_id)
        .bind(session.agent_id)
        .bind(&session.title)
        .bind(session.created_at)
        .bind(session.last_activity_at)
        .execute(&self.pool)
        .await;
        match insert_result {
            Ok(_) => Ok(()),
            Err(insert_error)
                if is_foreign_key_violation(
                    &insert_error,
                    "chat_sessions_organization_id_fkey",
                ) =>
            {
                Err(AgentError::NotFound("organization_not_found"))
            }
            Err(insert_error)
                if is_foreign_key_violation(&insert_error, "chat_sessions_agent_id_fkey") =>
            {
                Err(AgentError::NotFound("agent_not_found"))
            }
            Err(insert_error) => Err(storage_failure(insert_error)),
        }
    }

    async fn get_session(&self, session_id: Uuid) -> Result<Option<ChatSession>, AgentError> {
        let session_row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {SESSION_COLUMNS} FROM chat_sessions WHERE id = $1"
        )))
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failure)?;
        session_row.as_ref().map(session_from_row).transpose()
    }

    async fn list_sessions(
        &self,
        organization_id: Uuid,
        owner_user_id: i64,
    ) -> Result<Vec<ChatSession>, AgentError> {
        let session_rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {SESSION_COLUMNS} FROM chat_sessions
             WHERE organization_id = $1 AND owner_user_id = $2
             ORDER BY last_activity_at DESC, id"
        )))
        .bind(organization_id)
        .bind(owner_user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)?;
        session_rows.iter().map(session_from_row).collect()
    }

    async fn update_session_title(
        &self,
        session_id: Uuid,
        owner_user_id: i64,
        title: &str,
    ) -> Result<(), AgentError> {
        let update_result =
            sqlx::query("UPDATE chat_sessions SET title = $3 WHERE id = $1 AND owner_user_id = $2")
                .bind(session_id)
                .bind(owner_user_id)
                .bind(title)
                .execute(&self.pool)
                .await
                .map_err(storage_failure)?;
        if update_result.rows_affected() == 0 {
            return Err(AgentError::NotFound("session_not_found"));
        }
        Ok(())
    }

    async fn list_visible_messages(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<ChatMessage>, AgentError> {
        // 委派子 Run 的消息是父 Agent 的内部上下文，不进入用户可见时间线。
        let message_rows = sqlx::query(
            "SELECT m.id, m.session_id, m.run_id, m.role, m.content, m.resource_ids, m.created_at
             FROM chat_messages m
             LEFT JOIN agent_runs r ON r.id = m.run_id
             WHERE m.session_id = $1 AND (m.run_id IS NULL OR r.origin <> 'delegation')
             ORDER BY m.created_at, m.id",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)?;
        message_rows.iter().map(message_from_row).collect()
    }

    async fn list_session_runs(
        &self,
        session_id: Uuid,
        owner_user_id: i64,
    ) -> Result<Vec<RunRecord>, AgentError> {
        let run_rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {RUN_COLUMNS} FROM agent_runs
             WHERE session_id = $1 AND owner_user_id = $2 AND origin <> 'delegation'
             ORDER BY created_at, id"
        )))
        .bind(session_id)
        .bind(owner_user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)?;
        run_rows.iter().map(run_from_row).collect()
    }

    async fn get_assistant_message(&self, run_id: Uuid) -> Result<Option<ChatMessage>, AgentError> {
        let message_row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {MESSAGE_COLUMNS} FROM chat_messages
             WHERE run_id = $1 AND role = 'assistant'
             ORDER BY created_at DESC, id DESC
             LIMIT 1"
        )))
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failure)?;
        message_row.as_ref().map(message_from_row).transpose()
    }
}
