//! Canonical Run、持久事件、幂等键与 tracing outbox 的 PostgreSQL 实现。
//!
//! Run 创建、事件追加、取消与终态落盘都在单个事务内完成；事件与 outbox 同事务写入，
//! 因此 tracing 投递失败不会影响 Run 本身的事实。

use async_trait::async_trait;
use serde_json::Value;
use sqlx::postgres::PgRow;
use sqlx::{Postgres, Transaction};
use time::OffsetDateTime;
use tpl_agent_core::error::AgentError;
use tpl_agent_core::ports::RunRepository;
use tpl_agent_core::run::{
    CancelOutcome, CreateRunOutcome, FinalizeRunCommand, NewRun, PendingTraceEvent, RunEvent,
    RunEventDraft, RunOrigin, RunRecord, RunStatus,
};
use tpl_agent_core::run_events::{run_cancelled, run_cancelling, run_failed, run_started};
use tpl_agent_core::session::DEFAULT_SESSION_TITLE;
use uuid::Uuid;

use super::PostgresAgentRepository;
use super::{
    column, deserialize_column, is_unique_violation, parse_database_enum, serialize_structure,
    storage_failure, uuid_array_json,
};

pub(super) const RUN_COLUMNS: &str = "id, organization_id, session_id, owner_user_id, agent_id, \
     runtime_connection_id, parent_run_id, delegation_id, delegation_depth, origin, status, \
     idempotency_key, request_digest, snapshot, last_event_seq, trace_delivered_through_seq, \
     error_code, cancel_requested_at, started_at, finished_at, created_at";

/// 将 Run 行映射为核心记录。
pub(super) fn run_from_row(row: &PgRow) -> Result<RunRecord, AgentError> {
    let origin_text: String = column(row, "origin")?;
    let status_text: String = column(row, "status")?;
    let snapshot_value: Value = column(row, "snapshot")?;
    let delegation_depth = u8::try_from(column::<i16>(row, "delegation_depth")?)
        .map_err(|_| AgentError::StorageUnavailable)?;
    Ok(RunRecord {
        run_id: column(row, "id")?,
        organization_id: column(row, "organization_id")?,
        session_id: column(row, "session_id")?,
        owner_user_id: column(row, "owner_user_id")?,
        agent_id: column(row, "agent_id")?,
        runtime_connection_id: column(row, "runtime_connection_id")?,
        parent_run_id: column(row, "parent_run_id")?,
        delegation_id: column(row, "delegation_id")?,
        delegation_depth,
        origin: parse_database_enum(&origin_text, RunOrigin::from_database)?,
        status: parse_database_enum(&status_text, RunStatus::from_database)?,
        idempotency_key: column(row, "idempotency_key")?,
        request_digest: column(row, "request_digest")?,
        snapshot: deserialize_column(snapshot_value, "run_snapshot")?,
        last_event_seq: column(row, "last_event_seq")?,
        trace_delivered_through_seq: column(row, "trace_delivered_through_seq")?,
        error_code: column(row, "error_code")?,
        cancel_requested_at: column(row, "cancel_requested_at")?,
        started_at: column(row, "started_at")?,
        finished_at: column(row, "finished_at")?,
        created_at: column(row, "created_at")?,
    })
}

fn event_from_row(row: &PgRow) -> Result<RunEvent, AgentError> {
    Ok(RunEvent {
        run_id: column(row, "run_id")?,
        seq: column(row, "seq")?,
        event_type: column(row, "event_type")?,
        occurred_at: column(row, "occurred_at")?,
        payload: column(row, "payload")?,
    })
}

async fn fetch_run<'connection, E>(
    executor: E,
    run_id: Uuid,
) -> Result<Option<RunRecord>, AgentError>
where
    E: sqlx::Executor<'connection, Database = Postgres>,
{
    let run_row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {RUN_COLUMNS} FROM agent_runs WHERE id = $1"
    )))
    .bind(run_id)
    .fetch_optional(executor)
    .await
    .map_err(storage_failure)?;
    run_row.as_ref().map(run_from_row).transpose()
}

async fn lock_run(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<Option<RunRecord>, AgentError> {
    let run_row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {RUN_COLUMNS} FROM agent_runs WHERE id = $1 FOR UPDATE"
    )))
    .bind(run_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_failure)?;
    run_row.as_ref().map(run_from_row).transpose()
}

/// 在同一事务内写入持久事件与 outbox 行，返回已编号事件。
async fn insert_event_rows(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    first_seq: i64,
    drafts: &[RunEventDraft],
) -> Result<Vec<RunEvent>, AgentError> {
    let mut persisted_events = Vec::with_capacity(drafts.len());
    for (offset, draft) in drafts.iter().enumerate() {
        let offset = i64::try_from(offset).map_err(|_| AgentError::Internal)?;
        let seq = first_seq.checked_add(offset).ok_or(AgentError::Internal)?;
        sqlx::query(
            "INSERT INTO agent_run_events (run_id, seq, event_type, occurred_at, payload)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(run_id)
        .bind(seq)
        .bind(&draft.event_type)
        .bind(draft.occurred_at)
        .bind(&draft.payload)
        .execute(&mut **transaction)
        .await
        .map_err(storage_failure)?;
        sqlx::query(
            "INSERT INTO agent_trace_outbox
                 (run_id, event_seq, event_type, occurred_at, payload, status, attempts,
                  next_attempt_at)
             VALUES ($1, $2, $3, $4, $5, 'pending', 0, $6)",
        )
        .bind(run_id)
        .bind(seq)
        .bind(&draft.event_type)
        .bind(draft.occurred_at)
        .bind(&draft.payload)
        .bind(draft.occurred_at)
        .execute(&mut **transaction)
        .await
        .map_err(storage_failure)?;
        persisted_events.push(RunEvent {
            run_id,
            seq,
            event_type: draft.event_type.clone(),
            occurred_at: draft.occurred_at,
            payload: draft.payload.clone(),
        });
    }
    Ok(persisted_events)
}

async fn touch_session(
    transaction: &mut Transaction<'_, Postgres>,
    session_id: Uuid,
    activity_at: OffsetDateTime,
    title: Option<&str>,
) -> Result<(), AgentError> {
    if let Some(title) = title {
        sqlx::query(
            "UPDATE chat_sessions SET title = $2, last_activity_at = $3
             WHERE id = $1 AND title = $4",
        )
        .bind(session_id)
        .bind(title)
        .bind(activity_at)
        .bind(DEFAULT_SESSION_TITLE)
        .execute(&mut **transaction)
        .await
        .map_err(storage_failure)?;
        return Ok(());
    }
    sqlx::query("UPDATE chat_sessions SET last_activity_at = $2 WHERE id = $1")
        .bind(session_id)
        .bind(activity_at)
        .execute(&mut **transaction)
        .await
        .map_err(storage_failure)?;
    Ok(())
}

impl PostgresAgentRepository {
    async fn replay_idempotent_run(
        &self,
        owner_user_id: i64,
        idempotency_key: &str,
        request_digest: &str,
    ) -> Result<CreateRunOutcome, AgentError> {
        let idempotency_row = sqlx::query(
            "SELECT run_id, request_digest FROM agent_run_idempotency
             WHERE owner_user_id = $1 AND idempotency_key = $2",
        )
        .bind(owner_user_id)
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failure)?
        .ok_or(AgentError::StorageUnavailable)?;
        let stored_digest: String = column(&idempotency_row, "request_digest")?;
        if stored_digest != request_digest {
            return Err(AgentError::IdempotencyConflict);
        }
        let stored_run_id: Uuid = column(&idempotency_row, "run_id")?;
        let run = fetch_run(&self.pool, stored_run_id)
            .await?
            .ok_or(AgentError::StorageUnavailable)?;
        Ok(CreateRunOutcome {
            run,
            created: false,
        })
    }

    async fn settle_after_idempotency_conflict(
        &self,
        new_run: &NewRun,
    ) -> Result<CreateRunOutcome, AgentError> {
        // 并发事务已登记同一 owner 与幂等键；以已提交记录为准。
        self.replay_idempotent_run(
            new_run.owner_user_id,
            &new_run.idempotency_key,
            &new_run.request_digest,
        )
        .await
    }
}

#[async_trait]
impl RunRepository for PostgresAgentRepository {
    async fn create_run(
        &self,
        new_run: &NewRun,
        initial_events: &[RunEventDraft],
    ) -> Result<CreateRunOutcome, AgentError> {
        let existing = sqlx::query(
            "SELECT run_id, request_digest FROM agent_run_idempotency
             WHERE owner_user_id = $1 AND idempotency_key = $2",
        )
        .bind(new_run.owner_user_id)
        .bind(&new_run.idempotency_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failure)?;
        if existing.is_some() {
            return self
                .replay_idempotent_run(
                    new_run.owner_user_id,
                    &new_run.idempotency_key,
                    &new_run.request_digest,
                )
                .await;
        }

        let snapshot_value = serialize_structure(&new_run.snapshot)?;
        let resource_ids = new_run
            .snapshot
            .resources
            .iter()
            .map(|resource| resource.resource_id)
            .collect::<Vec<_>>();
        let mut transaction = self.pool.begin().await.map_err(storage_failure)?;

        sqlx::query(
            "INSERT INTO chat_messages
                 (id, session_id, run_id, role, content, resource_ids, created_at)
             VALUES ($1, $2, NULL, 'user', $3, $4, $5)",
        )
        .bind(new_run.snapshot.input.message_id)
        .bind(new_run.session_id)
        .bind(&new_run.snapshot.input.content)
        .bind(uuid_array_json(&resource_ids))
        .bind(new_run.created_at)
        .execute(&mut *transaction)
        .await
        .map_err(storage_failure)?;

        let run_insert = sqlx::query(
            "INSERT INTO agent_runs
                 (id, organization_id, session_id, owner_user_id, agent_id,
                  runtime_connection_id, parent_run_id, delegation_id, delegation_depth, origin,
                  status, idempotency_key, request_digest, input_message_id, snapshot,
                  agent_snapshot_checksum, context_pack_checksum, access_policy_checksum,
                  routing_checksum, last_event_seq, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'queued', $11, $12, $13, $14,
                     $15, $16, $17, $18, 0, $19)",
        )
        .bind(new_run.run_id)
        .bind(new_run.organization_id)
        .bind(new_run.session_id)
        .bind(new_run.owner_user_id)
        .bind(new_run.agent_id)
        .bind(new_run.runtime_connection_id)
        .bind(new_run.parent_run_id)
        .bind(new_run.delegation_id)
        .bind(i16::from(new_run.delegation_depth))
        .bind(new_run.origin.as_str())
        .bind(&new_run.idempotency_key)
        .bind(&new_run.request_digest)
        .bind(new_run.snapshot.input.message_id)
        .bind(snapshot_value)
        .bind(new_run.snapshot.agent_snapshot_checksum())
        .bind(new_run.snapshot.context_pack_checksum())
        .bind(&new_run.snapshot.access_policy_checksum)
        .bind(new_run.snapshot.routing_checksum())
        .bind(new_run.created_at)
        .execute(&mut *transaction)
        .await;
        if let Err(insert_error) = run_insert {
            return Err(super::is_foreign_key_violation(
                &insert_error,
                "agent_runs_session_id_fkey",
            )
            .then_some(AgentError::NotFound("session_not_found"))
            .or_else(|| {
                super::is_foreign_key_violation(&insert_error, "agent_runs_organization_id_fkey")
                    .then_some(AgentError::NotFound("organization_not_found"))
            })
            .or_else(|| {
                super::is_foreign_key_violation(
                    &insert_error,
                    "agent_runs_runtime_connection_id_fkey",
                )
                .then_some(AgentError::RuntimeUnavailable)
            })
            .unwrap_or_else(|| storage_failure(insert_error)));
        }

        sqlx::query("UPDATE chat_messages SET run_id = $2 WHERE id = $1")
            .bind(new_run.snapshot.input.message_id)
            .bind(new_run.run_id)
            .execute(&mut *transaction)
            .await
            .map_err(storage_failure)?;

        insert_event_rows(&mut transaction, new_run.run_id, 1, initial_events).await?;
        let last_event_seq =
            i64::try_from(initial_events.len()).map_err(|_| AgentError::Internal)?;
        sqlx::query("UPDATE agent_runs SET last_event_seq = $2 WHERE id = $1")
            .bind(new_run.run_id)
            .bind(last_event_seq)
            .execute(&mut *transaction)
            .await
            .map_err(storage_failure)?;

        touch_session(
            &mut transaction,
            new_run.session_id,
            new_run.created_at,
            new_run.session_title.as_deref(),
        )
        .await?;

        let idempotency_insert = sqlx::query(
            "INSERT INTO agent_run_idempotency
                 (owner_user_id, idempotency_key, request_digest, run_id, created_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(new_run.owner_user_id)
        .bind(&new_run.idempotency_key)
        .bind(&new_run.request_digest)
        .bind(new_run.run_id)
        .bind(new_run.created_at)
        .execute(&mut *transaction)
        .await;
        if let Err(insert_error) = idempotency_insert {
            drop(transaction);
            if is_unique_violation(&insert_error, "agent_run_idempotency_pkey") {
                return self.settle_after_idempotency_conflict(new_run).await;
            }
            return Err(storage_failure(insert_error));
        }

        let run = fetch_run(&mut *transaction, new_run.run_id)
            .await?
            .ok_or(AgentError::StorageUnavailable)?;
        transaction.commit().await.map_err(storage_failure)?;
        Ok(CreateRunOutcome { run, created: true })
    }

    async fn get_run(&self, run_id: Uuid) -> Result<Option<RunRecord>, AgentError> {
        fetch_run(&self.pool, run_id).await
    }

    async fn list_run_events(
        &self,
        run_id: Uuid,
        after_seq: i64,
        limit: usize,
    ) -> Result<Vec<RunEvent>, AgentError> {
        let limit = i64::try_from(limit).map_err(|_| AgentError::Internal)?;
        let event_rows = sqlx::query(
            "SELECT run_id, seq, event_type, occurred_at, payload
             FROM agent_run_events
             WHERE run_id = $1 AND seq > $2
             ORDER BY seq
             LIMIT $3",
        )
        .bind(run_id)
        .bind(after_seq)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)?;
        event_rows.iter().map(event_from_row).collect()
    }

    async fn mark_run_started(
        &self,
        run_id: Uuid,
        started_at: OffsetDateTime,
    ) -> Result<Option<RunRecord>, AgentError> {
        let mut transaction = self.pool.begin().await.map_err(storage_failure)?;
        let Some(run) = lock_run(&mut transaction, run_id).await? else {
            return Err(AgentError::NotFound("run_not_found"));
        };
        if run.status != RunStatus::Queued {
            return Ok(None);
        }
        let started_events = [run_started(started_at)];
        let next_seq = run.last_event_seq + 1;
        insert_event_rows(&mut transaction, run_id, next_seq, &started_events).await?;
        sqlx::query(
            "UPDATE agent_runs
             SET status = 'running', started_at = $2, last_event_seq = $3
             WHERE id = $1",
        )
        .bind(run_id)
        .bind(started_at)
        .bind(next_seq)
        .execute(&mut *transaction)
        .await
        .map_err(storage_failure)?;
        let started_run = fetch_run(&mut *transaction, run_id)
            .await?
            .ok_or(AgentError::StorageUnavailable)?;
        transaction.commit().await.map_err(storage_failure)?;
        Ok(Some(started_run))
    }

    async fn append_events(
        &self,
        run_id: Uuid,
        drafts: &[RunEventDraft],
    ) -> Result<Vec<RunEvent>, AgentError> {
        if drafts.is_empty() {
            return Ok(Vec::new());
        }
        let mut transaction = self.pool.begin().await.map_err(storage_failure)?;
        let Some(run) = lock_run(&mut transaction, run_id).await? else {
            return Err(AgentError::NotFound("run_not_found"));
        };
        if run.status.is_terminal() {
            return Err(AgentError::Conflict("run_finished"));
        }
        let next_seq = run.last_event_seq + 1;
        let persisted_events =
            insert_event_rows(&mut transaction, run_id, next_seq, drafts).await?;
        let last_seq = next_seq
            .checked_add(i64::try_from(drafts.len()).map_err(|_| AgentError::Internal)? - 1)
            .ok_or(AgentError::Internal)?;
        sqlx::query("UPDATE agent_runs SET last_event_seq = $2 WHERE id = $1")
            .bind(run_id)
            .bind(last_seq)
            .execute(&mut *transaction)
            .await
            .map_err(storage_failure)?;
        transaction.commit().await.map_err(storage_failure)?;
        Ok(persisted_events)
    }

    async fn finalize_run(&self, command: &FinalizeRunCommand) -> Result<RunRecord, AgentError> {
        let mut transaction = self.pool.begin().await.map_err(storage_failure)?;
        let Some(run) = lock_run(&mut transaction, command.run_id).await? else {
            return Err(AgentError::NotFound("run_not_found"));
        };
        if run.status == command.final_status {
            return Ok(run);
        }
        // 取消请求与成功完成之间存在窗口；工作已经完成时以成功为准，取消保持协作式语义。
        let transition_allowed = run.status.can_transition_to(command.final_status)
            || (run.status == RunStatus::Cancelling
                && command.final_status == RunStatus::Succeeded);
        if !transition_allowed {
            return Err(AgentError::Conflict("run_already_finished"));
        }

        if let Some(assistant_message) = &command.assistant_message {
            sqlx::query(
                "INSERT INTO chat_messages
                     (id, session_id, run_id, role, content, resource_ids, created_at)
                 VALUES ($1, $2, $3, 'assistant', $4, $5, $6)",
            )
            .bind(assistant_message.id)
            .bind(assistant_message.session_id)
            .bind(assistant_message.run_id)
            .bind(&assistant_message.content)
            .bind(uuid_array_json(&assistant_message.resource_ids))
            .bind(assistant_message.created_at)
            .execute(&mut *transaction)
            .await
            .map_err(storage_failure)?;
        }
        for artifact in &command.artifacts {
            sqlx::query(
                "INSERT INTO run_artifacts
                     (id, run_id, owner_user_id, filename, media_type, size_bytes, sha256,
                      storage_key, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            )
            .bind(artifact.id)
            .bind(artifact.run_id)
            .bind(artifact.owner_user_id)
            .bind(&artifact.filename)
            .bind(&artifact.media_type)
            .bind(artifact.size_bytes)
            .bind(&artifact.sha256)
            .bind(&artifact.storage_key)
            .bind(artifact.created_at)
            .execute(&mut *transaction)
            .await
            .map_err(storage_failure)?;
        }
        let next_seq = run.last_event_seq + 1;
        insert_event_rows(&mut transaction, run.run_id, next_seq, &command.events).await?;
        let last_seq = next_seq
            .checked_add(i64::try_from(command.events.len()).map_err(|_| AgentError::Internal)? - 1)
            .ok_or(AgentError::Internal)?;
        sqlx::query(
            "UPDATE agent_runs
             SET status = $2, error_code = $3, finished_at = $4, last_event_seq = $5
             WHERE id = $1",
        )
        .bind(run.run_id)
        .bind(command.final_status.as_str())
        .bind(&command.error_code)
        .bind(command.finished_at)
        .bind(last_seq)
        .execute(&mut *transaction)
        .await
        .map_err(storage_failure)?;
        touch_session(&mut transaction, run.session_id, command.finished_at, None).await?;
        let finalized_run = fetch_run(&mut *transaction, run.run_id)
            .await?
            .ok_or(AgentError::StorageUnavailable)?;
        transaction.commit().await.map_err(storage_failure)?;
        Ok(finalized_run)
    }

    async fn request_cancel(
        &self,
        run_id: Uuid,
        owner_user_id: i64,
        requested_by: &str,
        requested_at: OffsetDateTime,
    ) -> Result<CancelOutcome, AgentError> {
        let mut transaction = self.pool.begin().await.map_err(storage_failure)?;
        let Some(run) = lock_run(&mut transaction, run_id).await? else {
            return Err(AgentError::NotFound("run_not_found"));
        };
        if run.owner_user_id != owner_user_id {
            return Err(AgentError::NotFound("run_not_found"));
        }
        if run.status.is_terminal() {
            return Ok(CancelOutcome {
                run,
                cancelled_immediately: false,
            });
        }
        if run.status == RunStatus::Queued {
            let cancel_events = [
                run_cancelling(requested_at, requested_by, requested_at),
                run_cancelled(requested_at, false),
            ];
            let next_seq = run.last_event_seq + 1;
            insert_event_rows(&mut transaction, run_id, next_seq, &cancel_events).await?;
            let last_seq = next_seq + 1;
            sqlx::query(
                "UPDATE agent_runs
                 SET status = 'cancelled', cancel_requested_at = $2, finished_at = $2,
                     error_code = 'run_cancelled', last_event_seq = $3
                 WHERE id = $1",
            )
            .bind(run_id)
            .bind(requested_at)
            .bind(last_seq)
            .execute(&mut *transaction)
            .await
            .map_err(storage_failure)?;
            touch_session(&mut transaction, run.session_id, requested_at, None).await?;
            let cancelled_run = fetch_run(&mut *transaction, run_id)
                .await?
                .ok_or(AgentError::StorageUnavailable)?;
            transaction.commit().await.map_err(storage_failure)?;
            return Ok(CancelOutcome {
                run: cancelled_run,
                cancelled_immediately: true,
            });
        }
        if run.status == RunStatus::Running && run.cancel_requested_at.is_none() {
            let cancelling_events = [run_cancelling(requested_at, requested_by, requested_at)];
            let next_seq = run.last_event_seq + 1;
            insert_event_rows(&mut transaction, run_id, next_seq, &cancelling_events).await?;
            sqlx::query(
                "UPDATE agent_runs
                 SET status = 'cancelling', cancel_requested_at = $2, last_event_seq = $3
                 WHERE id = $1",
            )
            .bind(run_id)
            .bind(requested_at)
            .bind(next_seq)
            .execute(&mut *transaction)
            .await
            .map_err(storage_failure)?;
            touch_session(&mut transaction, run.session_id, requested_at, None).await?;
            let cancelling_run = fetch_run(&mut *transaction, run_id)
                .await?
                .ok_or(AgentError::StorageUnavailable)?;
            transaction.commit().await.map_err(storage_failure)?;
            return Ok(CancelOutcome {
                run: cancelling_run,
                cancelled_immediately: false,
            });
        }
        Ok(CancelOutcome {
            run,
            cancelled_immediately: false,
        })
    }

    async fn get_run_status(&self, run_id: Uuid) -> Result<Option<RunStatus>, AgentError> {
        let status_text =
            sqlx::query_scalar::<_, String>("SELECT status FROM agent_runs WHERE id = $1")
                .bind(run_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(storage_failure)?;
        status_text
            .as_deref()
            .map(|value| parse_database_enum(value, RunStatus::from_database))
            .transpose()
    }

    async fn count_child_runs(&self, parent_run_id: Uuid) -> Result<u32, AgentError> {
        let child_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM agent_runs WHERE parent_run_id = $1",
        )
        .bind(parent_run_id)
        .fetch_one(&self.pool)
        .await
        .map_err(storage_failure)?;
        u32::try_from(child_count).map_err(|_| AgentError::StorageUnavailable)
    }

    async fn list_queued_run_ids(&self, limit: usize) -> Result<Vec<Uuid>, AgentError> {
        let limit = i64::try_from(limit).map_err(|_| AgentError::Internal)?;
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM agent_runs
             WHERE status = 'queued' AND cancel_requested_at IS NULL
             ORDER BY created_at
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)
    }

    async fn recover_incomplete_runs(&self, now: OffsetDateTime) -> Result<u64, AgentError> {
        let mut transaction = self.pool.begin().await.map_err(storage_failure)?;
        let incomplete_rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {RUN_COLUMNS} FROM agent_runs
             WHERE status IN ('queued', 'running', 'cancelling')
             FOR UPDATE SKIP LOCKED"
        )))
        .fetch_all(&mut *transaction)
        .await
        .map_err(storage_failure)?;
        let mut recovered = 0_u64;
        for row in &incomplete_rows {
            let run = run_from_row(row)?;
            let (final_status, error_code, terminal_event) = if run.status == RunStatus::Cancelling
            {
                (
                    RunStatus::Cancelled,
                    "run_cancelled",
                    run_cancelled(now, false),
                )
            } else {
                (
                    RunStatus::Failed,
                    "service_restarted",
                    run_failed(now, "service_restarted"),
                )
            };
            let next_seq = run.last_event_seq + 1;
            insert_event_rows(&mut transaction, run.run_id, next_seq, &[terminal_event]).await?;
            sqlx::query(
                "UPDATE agent_runs
                 SET status = $2, error_code = $3, finished_at = $4, last_event_seq = $5
                 WHERE id = $1",
            )
            .bind(run.run_id)
            .bind(final_status.as_str())
            .bind(error_code)
            .bind(now)
            .bind(next_seq)
            .execute(&mut *transaction)
            .await
            .map_err(storage_failure)?;
            touch_session(&mut transaction, run.session_id, now, None).await?;
            recovered += 1;
        }
        transaction.commit().await.map_err(storage_failure)?;
        Ok(recovered)
    }

    async fn list_pending_outbox_run_ids(&self, limit: usize) -> Result<Vec<Uuid>, AgentError> {
        let limit = i64::try_from(limit).map_err(|_| AgentError::Internal)?;
        sqlx::query_scalar::<_, Uuid>(
            "SELECT run_id FROM agent_trace_outbox
             WHERE status = 'pending' AND next_attempt_at <= now()
             GROUP BY run_id
             ORDER BY MIN(id)
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)
    }

    async fn fetch_pending_outbox_for_run(
        &self,
        run_id: Uuid,
        limit: usize,
    ) -> Result<Vec<PendingTraceEvent>, AgentError> {
        let limit = i64::try_from(limit).map_err(|_| AgentError::Internal)?;
        let outbox_rows = sqlx::query(
            "SELECT event_seq, event_type, occurred_at, payload,
                    (next_attempt_at <= now()) AS due
             FROM agent_trace_outbox
             WHERE run_id = $1 AND status = 'pending'
             ORDER BY event_seq
             LIMIT $2",
        )
        .bind(run_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failure)?;
        let mut pending_events = Vec::with_capacity(outbox_rows.len());
        for row in &outbox_rows {
            let due: bool = column(row, "due")?;
            if !due {
                // 投递必须按 seq 连续确认；遇到退避中的事件即停止本批。
                break;
            }
            pending_events.push(PendingTraceEvent {
                run_id,
                seq: column(row, "event_seq")?,
                event_type: column(row, "event_type")?,
                occurred_at: column(row, "occurred_at")?,
                payload: column(row, "payload")?,
            });
        }
        Ok(pending_events)
    }

    async fn mark_trace_delivered(
        &self,
        run_id: Uuid,
        delivered_through_seq: i64,
        delivered_at: OffsetDateTime,
    ) -> Result<(), AgentError> {
        let mut transaction = self.pool.begin().await.map_err(storage_failure)?;
        sqlx::query(
            "UPDATE agent_trace_outbox
             SET status = 'delivered', delivered_at = $3
             WHERE run_id = $1 AND event_seq <= $2 AND status = 'pending'",
        )
        .bind(run_id)
        .bind(delivered_through_seq)
        .bind(delivered_at)
        .execute(&mut *transaction)
        .await
        .map_err(storage_failure)?;
        sqlx::query(
            "UPDATE agent_runs
             SET trace_delivered_through_seq =
                 GREATEST(trace_delivered_through_seq, $2)
             WHERE id = $1",
        )
        .bind(run_id)
        .bind(delivered_through_seq)
        .execute(&mut *transaction)
        .await
        .map_err(storage_failure)?;
        transaction.commit().await.map_err(storage_failure)
    }

    async fn mark_trace_failed(
        &self,
        run_id: Uuid,
        seq: i64,
        error_code: &str,
        next_attempt_at: OffsetDateTime,
    ) -> Result<(), AgentError> {
        sqlx::query(
            "UPDATE agent_trace_outbox
             SET attempts = attempts + 1, last_error_code = $3, next_attempt_at = $4
             WHERE run_id = $1 AND event_seq = $2 AND status = 'pending'",
        )
        .bind(run_id)
        .bind(seq)
        .bind(error_code)
        .bind(next_attempt_at)
        .execute(&self.pool)
        .await
        .map_err(storage_failure)?;
        Ok(())
    }

    async fn get_run_origin(&self, run_id: Uuid) -> Result<Option<RunOrigin>, AgentError> {
        let origin_text =
            sqlx::query_scalar::<_, String>("SELECT origin FROM agent_runs WHERE id = $1")
                .bind(run_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(storage_failure)?;
        origin_text
            .as_deref()
            .map(|value| parse_database_enum(value, RunOrigin::from_database))
            .transpose()
    }
}
