//! Producer 凭据登记、认证和事务式事件追加。

use sqlx::postgres::{PgPool, Postgres};
use sqlx::{FromRow, Transaction};
use time::OffsetDateTime;
use tpl_agent_tracing_core::event::EventEnvelope;
use tpl_agent_tracing_core::model::{
    AppendAck, AppendResult, ProducerIdentity, ProducerRegistration, RunRecord, RunStatus,
    StoredEvent,
};
use tpl_agent_tracing_core::ports::TraceStoreError;

use super::queries::load_events;

#[derive(FromRow)]
struct ProducerRow {
    source_id: String,
    project_id: String,
}

#[derive(FromRow)]
struct LockedRunRow {
    source_id: String,
    project_id: String,
    thread_id: String,
    agent_id: String,
    status: String,
    trace_id: Option<String>,
    created_at: OffsetDateTime,
    started_at: Option<OffsetDateTime>,
    finished_at: Option<OffsetDateTime>,
    accepted_through_seq: i64,
    last_activity_at: OffsetDateTime,
}

#[derive(FromRow)]
struct InsertedEventRow {
    recorded_at: OffsetDateTime,
}

/// 使配置里的 source 集合与数据库 active token 集合保持一致。
pub(super) async fn register_producers(
    pool: &PgPool,
    registrations: &[ProducerRegistration],
) -> Result<(), TraceStoreError> {
    let mut transaction = pool.begin().await.map_err(storage_error)?;
    sqlx::query("UPDATE producer_sources SET enabled = FALSE")
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
    for registration in registrations {
        sqlx::query(
            "INSERT INTO producer_sources (source_id, project_id, token_hash, enabled) \
             VALUES ($1, $2, $3, TRUE) \
             ON CONFLICT (source_id) DO UPDATE SET \
                 project_id = EXCLUDED.project_id, token_hash = EXCLUDED.token_hash, enabled = TRUE",
        )
        .bind(&registration.source_id)
        .bind(&registration.project_id)
        .bind(&registration.token_hash)
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
    }
    transaction.commit().await.map_err(storage_error)
}

/// 通过 token 摘要查找有效 producer 来源。
pub(super) async fn authenticate_producer(
    pool: &PgPool,
    token_hash: &[u8],
) -> Result<ProducerIdentity, TraceStoreError> {
    let producer_row = sqlx::query_as::<_, ProducerRow>(
        "SELECT source_id, project_id FROM producer_sources WHERE token_hash = $1 AND enabled = TRUE",
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await
    .map_err(storage_error)?
    .ok_or(TraceStoreError::ProducerUnauthorized)?;
    Ok(ProducerIdentity {
        source_id: producer_row.source_id,
        project_id: producer_row.project_id,
    })
}

/// 在单个事务中校验重试、序号、状态并追加事件。
pub(super) async fn append_batch(
    pool: &PgPool,
    producer: &ProducerIdentity,
    run_id: &str,
    events: &[EventEnvelope],
) -> Result<AppendResult, TraceStoreError> {
    let first_event = events.first().ok_or(TraceStoreError::RunStateConflict)?;
    let mut transaction = pool.begin().await.map_err(storage_error)?;
    ensure_thread(&mut transaction, producer, run_id, first_event.occurred_at).await?;
    sqlx::query(
        "INSERT INTO agent_runs \
         (source_id, run_id, project_id, thread_id, agent_id, status, created_at, last_activity_at) \
         VALUES ($1, $2, $3, $2, $1, 'queued', $4, $4) \
         ON CONFLICT (source_id, run_id) DO NOTHING",
    )
    .bind(&producer.source_id)
    .bind(run_id)
    .bind(&producer.project_id)
    .bind(first_event.occurred_at)
    .execute(&mut *transaction)
    .await
    .map_err(storage_error)?;
    let mut run_row = sqlx::query_as::<_, LockedRunRow>(
        "SELECT source_id, project_id, thread_id, agent_id, status, trace_id, created_at, started_at, finished_at, accepted_through_seq, last_activity_at \
         FROM agent_runs WHERE run_id = $1 FOR UPDATE",
    )
    .bind(run_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(storage_error)?
    .ok_or(TraceStoreError::StorageUnavailable)?;
    if run_row.source_id != producer.source_id || run_row.project_id != producer.project_id {
        return Err(TraceStoreError::ProducerUnauthorized);
    }
    let mut accepted_through_seq = u64::try_from(run_row.accepted_through_seq)
        .map_err(|_| TraceStoreError::StorageUnavailable)?;
    let mut replayed_events = 0_u32;
    let mut inserted_events = Vec::with_capacity(events.len());
    for event in events {
        if event.seq <= accepted_through_seq {
            let stored_event_hash = sqlx::query_scalar::<_, Vec<u8>>(
                "SELECT event_hash FROM agent_run_events WHERE source_id = $1 AND run_id = $2 AND seq = $3",
            )
            .bind(&producer.source_id)
            .bind(run_id)
            .bind(i64::try_from(event.seq).map_err(|_| TraceStoreError::SequenceConflict { seq: event.seq })?)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(storage_error)?
            .ok_or(TraceStoreError::SequenceConflict { seq: event.seq })?;
            if !constant_time_equal(&stored_event_hash, &event_hash(event)?) {
                return Err(TraceStoreError::SequenceConflict { seq: event.seq });
            }
            replayed_events += 1;
            continue;
        }
        let expected_seq = accepted_through_seq.saturating_add(1);
        if event.seq != expected_seq {
            return Err(TraceStoreError::SequenceGap { expected_seq });
        }
        if RunStatus::from_database(&run_row.status)
            .ok_or(TraceStoreError::StorageUnavailable)?
            .is_terminal()
        {
            return Err(TraceStoreError::RunTerminal);
        }
        if let (Some(current_trace_id), Some(event_trace_id)) =
            (run_row.trace_id.as_deref(), event.trace_id.as_deref())
            && current_trace_id != event_trace_id
        {
            return Err(TraceStoreError::TraceIdConflict);
        }
        let next_status = transition_status(&run_row.status, &event.event_type)?;
        let event_thread_id = if event.event_type == "run.created" {
            event
                .payload
                .get("thread_id")
                .and_then(serde_json::Value::as_str)
                .filter(|thread_id| !thread_id.trim().is_empty() && thread_id.len() <= 256)
                .ok_or(TraceStoreError::RunStateConflict)?
                .to_owned()
        } else {
            run_row.thread_id.clone()
        };
        if event_thread_id != run_row.thread_id {
            ensure_thread(
                &mut transaction,
                producer,
                &event_thread_id,
                event.occurred_at,
            )
            .await?;
        }
        let serialized_envelope =
            serde_json::to_value(event).map_err(|_| TraceStoreError::StorageUnavailable)?;
        let serialized_hash = event_hash(event)?;
        let inserted_row = sqlx::query_as::<_, InsertedEventRow>(
            "INSERT INTO agent_run_events \
             (source_id, run_id, seq, event_type, occurred_at, trace_id, span_id, parent_span_id, payload, envelope, event_hash) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING recorded_at",
        )
        .bind(&producer.source_id)
        .bind(run_id)
        .bind(i64::try_from(event.seq).map_err(|_| TraceStoreError::SequenceConflict { seq: event.seq })?)
        .bind(&event.event_type)
        .bind(event.occurred_at)
        .bind(&event.trace_id)
        .bind(&event.span_id)
        .bind(&event.parent_span_id)
        .bind(&event.payload)
        .bind(serialized_envelope)
        .bind(serialized_hash)
        .fetch_one(&mut *transaction)
        .await
        .map_err(storage_error)?;
        if event.event_type == "run.created" {
            run_row.agent_id = event
                .payload
                .get("agent_id")
                .and_then(serde_json::Value::as_str)
                .filter(|agent_id| !agent_id.trim().is_empty() && agent_id.len() <= 256)
                .ok_or(TraceStoreError::RunStateConflict)?
                .to_owned();
            run_row.thread_id = event_thread_id;
            run_row.created_at = run_row.created_at.min(event.occurred_at);
        }
        match event.event_type.as_str() {
            "run.started" => run_row.started_at = Some(event.occurred_at),
            "run.completed" | "run.failed" | "run.cancelled" | "run.interrupted" => {
                run_row.finished_at = Some(event.occurred_at)
            }
            _ => {}
        }
        run_row.status = next_status.as_str().to_owned();
        run_row.trace_id = run_row.trace_id.clone().or_else(|| event.trace_id.clone());
        run_row.accepted_through_seq = i64::try_from(event.seq)
            .map_err(|_| TraceStoreError::SequenceConflict { seq: event.seq })?;
        accepted_through_seq = event.seq;
        run_row.last_activity_at = run_row.last_activity_at.max(event.occurred_at);
        sqlx::query(
            "UPDATE agent_runs SET thread_id = $3, agent_id = $4, status = $5, trace_id = $6, \
             created_at = $7, started_at = $8, finished_at = $9, accepted_through_seq = $10, last_activity_at = $11 \
             WHERE source_id = $1 AND run_id = $2",
        )
        .bind(&producer.source_id)
        .bind(run_id)
        .bind(&run_row.thread_id)
        .bind(&run_row.agent_id)
        .bind(&run_row.status)
        .bind(&run_row.trace_id)
        .bind(run_row.created_at)
        .bind(run_row.started_at)
        .bind(run_row.finished_at)
        .bind(run_row.accepted_through_seq)
        .bind(run_row.last_activity_at)
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        sqlx::query(
            "UPDATE agent_threads SET created_at = LEAST(created_at, $3), last_activity_at = GREATEST(last_activity_at, $3) \
             WHERE thread_id = $1 AND source_id = $2",
        )
        .bind(&run_row.thread_id)
        .bind(&producer.source_id)
        .bind(event.occurred_at)
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        inserted_events.push(StoredEvent {
            run_id: run_id.to_owned(),
            source_id: producer.source_id.clone(),
            project_id: producer.project_id.clone(),
            seq: event.seq,
            event_type: event.event_type.clone(),
            occurred_at: event.occurred_at,
            recorded_at: inserted_row.recorded_at,
            trace_id: event.trace_id.clone(),
            span_id: event.span_id.clone(),
            parent_span_id: event.parent_span_id.clone(),
            payload: event.payload.clone(),
        });
    }
    sqlx::query(
        "DELETE FROM agent_threads WHERE thread_id = $1 AND source_id = $2 \
         AND NOT EXISTS (SELECT 1 FROM agent_runs WHERE thread_id = $1 AND source_id = $2)",
    )
    .bind(run_id)
    .bind(&producer.source_id)
    .execute(&mut *transaction)
    .await
    .map_err(storage_error)?;
    let current_status =
        RunStatus::from_database(&run_row.status).ok_or(TraceStoreError::StorageUnavailable)?;
    let run_events = if current_status.is_terminal() && !inserted_events.is_empty() {
        load_events(&mut transaction, run_id).await?
    } else {
        Vec::new()
    };
    let run_record = RunRecord {
        run_id: run_id.to_owned(),
        source_id: run_row.source_id,
        project_id: run_row.project_id,
        thread_id: run_row.thread_id,
        agent_id: run_row.agent_id,
        status: current_status,
        created_at: run_row.created_at,
        started_at: run_row.started_at,
        finished_at: run_row.finished_at,
        trace_id: run_row.trace_id,
        accepted_through_seq,
        last_activity_at: run_row.last_activity_at,
    };
    transaction.commit().await.map_err(storage_error)?;
    Ok(AppendResult {
        acknowledgement: AppendAck {
            run_id: run_id.to_owned(),
            accepted_through_seq,
            replayed_events,
        },
        current_status,
        run_record,
        inserted_events,
        run_events,
    })
}

async fn ensure_thread(
    transaction: &mut Transaction<'_, Postgres>,
    producer: &ProducerIdentity,
    thread_id: &str,
    occurred_at: OffsetDateTime,
) -> Result<(), TraceStoreError> {
    sqlx::query(
        "INSERT INTO agent_threads (thread_id, source_id, project_id, created_at, last_activity_at) \
         VALUES ($1, $2, $3, $4, $4) ON CONFLICT DO NOTHING",
    )
    .bind(thread_id)
    .bind(&producer.source_id)
    .bind(&producer.project_id)
    .bind(occurred_at)
    .execute(&mut **transaction)
    .await
    .map_err(storage_error)?;
    let source_id =
        sqlx::query_scalar::<_, String>("SELECT source_id FROM agent_threads WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(storage_error)?;
    if source_id.as_deref() != Some(producer.source_id.as_str()) {
        return Err(TraceStoreError::ThreadSourceConflict);
    }
    Ok(())
}

fn transition_status(current_status: &str, event_type: &str) -> Result<RunStatus, TraceStoreError> {
    let current =
        RunStatus::from_database(current_status).ok_or(TraceStoreError::StorageUnavailable)?;
    match event_type {
        "run.started" if current == RunStatus::Queued => Ok(RunStatus::Running),
        "run.cancelling" if matches!(current, RunStatus::Queued | RunStatus::Running) => {
            Ok(RunStatus::Cancelling)
        }
        "run.completed" if current == RunStatus::Running => Ok(RunStatus::Succeeded),
        "run.failed"
            if matches!(
                current,
                RunStatus::Queued | RunStatus::Running | RunStatus::Cancelling
            ) =>
        {
            Ok(RunStatus::Failed)
        }
        "run.cancelled" if current == RunStatus::Cancelling => Ok(RunStatus::Cancelled),
        "run.interrupted"
            if matches!(
                current,
                RunStatus::Queued | RunStatus::Running | RunStatus::Cancelling
            ) =>
        {
            Ok(RunStatus::Interrupted)
        }
        _ if matches!(
            event_type,
            "run.started"
                | "run.cancelling"
                | "run.completed"
                | "run.failed"
                | "run.cancelled"
                | "run.interrupted"
        ) =>
        {
            Err(TraceStoreError::RunStateConflict)
        }
        _ => Ok(current),
    }
}

fn event_hash(event: &EventEnvelope) -> Result<Vec<u8>, TraceStoreError> {
    let canonical_event =
        serde_json::to_vec(event).map_err(|_| TraceStoreError::StorageUnavailable)?;
    use sha2::{Digest, Sha256};
    Ok(Sha256::digest(canonical_event).to_vec())
}

fn constant_time_equal(left_bytes: &[u8], right_bytes: &[u8]) -> bool {
    if left_bytes.len() != right_bytes.len() {
        return false;
    }
    left_bytes
        .iter()
        .zip(right_bytes)
        .fold(0_u8, |difference, (left_byte, right_byte)| {
            difference | (left_byte ^ right_byte)
        })
        == 0
}

pub(super) fn storage_error(database_error: sqlx::Error) -> TraceStoreError {
    tracing::error!(error = %database_error, "tracing 数据库操作失败");
    TraceStoreError::StorageUnavailable
}
