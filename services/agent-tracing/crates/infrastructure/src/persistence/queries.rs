//! Run、thread 与原始事件的 PostgreSQL 读取查询。

use std::collections::BTreeMap;

use sqlx::postgres::{PgPool, Postgres};
use sqlx::query_builder::QueryBuilder;
use sqlx::{Executor, FromRow, Transaction};
use time::OffsetDateTime;
use tpl_agent_tracing_core::model::{
    Page, RunActivity, RunQuery, RunRecord, RunStatus, StoredEvent, ThreadQuery, ThreadSummary,
};
use tpl_agent_tracing_core::ports::{ExportAudit, TraceStoreError};

use super::append::storage_error;

#[derive(FromRow)]
struct DbRun {
    run_id: String,
    source_id: String,
    project_id: String,
    thread_id: String,
    agent_id: String,
    status: String,
    created_at: OffsetDateTime,
    started_at: Option<OffsetDateTime>,
    finished_at: Option<OffsetDateTime>,
    trace_id: Option<String>,
    accepted_through_seq: i64,
    last_activity_at: OffsetDateTime,
}

#[derive(FromRow)]
struct DbStoredEvent {
    run_id: String,
    source_id: String,
    project_id: String,
    seq: i64,
    event_type: String,
    occurred_at: OffsetDateTime,
    recorded_at: OffsetDateTime,
    trace_id: Option<String>,
    span_id: Option<String>,
    parent_span_id: Option<String>,
    payload: serde_json::Value,
}

#[derive(FromRow)]
struct DbThreadSummary {
    thread_id: String,
    source_id: String,
    project_id: String,
    run_count: i64,
    limited_run_count: i64,
    agent_names: Vec<String>,
    created_at: OffsetDateTime,
    last_activity_at: OffsetDateTime,
}

#[derive(FromRow)]
struct DbThreadStatusCount {
    thread_id: String,
    status: String,
    run_count: i64,
}

#[derive(FromRow)]
struct DbRunActivity {
    run_id: String,
    model_call_count: i64,
    tool_call_count: i64,
    error_count: i64,
    has_step_events: bool,
}

#[derive(FromRow)]
struct DbUsageActivity {
    run_id: String,
    metric_name: String,
    metric_total: String,
}

/// 按过滤项分页查询 Run 摘要来源。
pub(super) async fn list_runs(
    pool: &PgPool,
    query: &RunQuery,
) -> Result<Page<RunRecord>, TraceStoreError> {
    let mut total_query =
        QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM agent_runs WHERE TRUE");
    append_run_filters(&mut total_query, query);
    let total_count = total_query
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(storage_error)?;

    let mut page_query = QueryBuilder::<Postgres>::new(
        "SELECT run_id, source_id, project_id, thread_id, agent_id, status, created_at, started_at, finished_at, trace_id, accepted_through_seq, last_activity_at FROM agent_runs WHERE TRUE",
    );
    append_run_filters(&mut page_query, query);
    page_query
        .push(" ORDER BY last_activity_at DESC, run_id ASC LIMIT ")
        .push_bind(i64::from(query.page_size.clamp(1, 100)))
        .push(" OFFSET ")
        .push_bind(i64::from(
            query
                .page
                .saturating_sub(1)
                .saturating_mul(query.page_size.clamp(1, 100)),
        ));
    let database_rows = page_query
        .build_query_as::<DbRun>()
        .fetch_all(pool)
        .await
        .map_err(storage_error)?;
    Ok(Page {
        items: database_rows
            .into_iter()
            .map(map_run)
            .collect::<Result<_, _>>()?,
        total: u64::try_from(total_count).unwrap_or_default(),
        page: query.page.max(1),
        page_size: query.page_size.clamp(1, 100),
    })
}

/// 读取单个 Run 的投影和按序原始事件。
pub(super) async fn get_run(
    pool: &PgPool,
    run_id: &str,
) -> Result<(RunRecord, Vec<StoredEvent>), TraceStoreError> {
    let database_row = sqlx::query_as::<_, DbRun>(
        "SELECT run_id, source_id, project_id, thread_id, agent_id, status, created_at, started_at, finished_at, trace_id, accepted_through_seq, last_activity_at FROM agent_runs WHERE run_id = $1",
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await
    .map_err(storage_error)?
    .ok_or(TraceStoreError::NotFound)?;
    let run_record = map_run(database_row)?;
    let run_events = load_events_from_pool(pool, run_id).await?;
    Ok((run_record, run_events))
}

/// 批量聚合列表页指标，避免加载正文与逐 Run 查询事件。
pub(super) async fn summarize_run_activity(
    pool: &PgPool,
    run_ids: &[String],
) -> Result<BTreeMap<String, RunActivity>, TraceStoreError> {
    if run_ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let database_rows = sqlx::query_as::<_, DbRunActivity>(
        "SELECT run_id, \
         COUNT(DISTINCT payload->>'model_call_id') FILTER (WHERE event_type IN ('model.call.started', 'model.call.completed', 'model.call.failed', 'routing.router_invoked')) AS model_call_count, \
         COUNT(DISTINCT payload->>'tool_call_id') FILTER (WHERE event_type = 'tool.call.started') AS tool_call_count, \
         COUNT(*) FILTER (WHERE event_type IN ('model.call.failed', 'tool.call.failed') OR (event_type = 'decision.gate.evaluated' AND payload->>'verdict' = 'block')) AS error_count, \
         BOOL_OR(event_type IN ('model.call.started', 'model.call.completed', 'model.call.failed', 'routing.router_invoked', 'tool.call.started', 'tool.call.completed', 'tool.call.failed', 'artifact.created', 'decision.gate.evaluated')) AS has_step_events \
         FROM agent_run_events WHERE run_id = ANY($1) GROUP BY run_id",
    )
    .bind(run_ids.to_vec())
    .fetch_all(pool)
    .await
    .map_err(storage_error)?;
    let usage_rows = sqlx::query_as::<_, DbUsageActivity>(
        "SELECT run_id, usage_metric.key AS metric_name, SUM(usage_metric.value::numeric)::text AS metric_total \
         FROM agent_run_events \
         CROSS JOIN LATERAL jsonb_each_text(CASE WHEN jsonb_typeof(payload->'usage') = 'object' THEN payload->'usage' ELSE '{}'::jsonb END) AS usage_metric \
         WHERE run_id = ANY($1) AND event_type IN ('model.call.completed', 'routing.router_invoked') \
           AND usage_metric.value ~ '^[0-9]+$' \
         GROUP BY run_id, usage_metric.key",
    )
    .bind(run_ids.to_vec())
    .fetch_all(pool)
    .await
    .map_err(storage_error)?;
    let mut activity_by_run_id = database_rows
        .into_iter()
        .map(|database_row| {
            (
                database_row.run_id,
                RunActivity {
                    has_step_events: database_row.has_step_events,
                    model_call_count: u32::try_from(database_row.model_call_count)
                        .unwrap_or_default(),
                    tool_call_count: u32::try_from(database_row.tool_call_count)
                        .unwrap_or_default(),
                    error_count: u32::try_from(database_row.error_count).unwrap_or_default(),
                    usage: BTreeMap::new(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    for usage_row in usage_rows {
        if let Ok(metric_total) = usage_row.metric_total.parse::<u64>() {
            activity_by_run_id
                .entry(usage_row.run_id)
                .or_default()
                .usage
                .insert(usage_row.metric_name, metric_total);
        }
    }
    Ok(activity_by_run_id)
}

/// 按 thread 条件分页查询会话摘要。
pub(super) async fn list_threads(
    pool: &PgPool,
    query: &ThreadQuery,
) -> Result<Page<ThreadSummary>, TraceStoreError> {
    let mut total_query = QueryBuilder::<Postgres>::new(
        "SELECT COUNT(*) FROM (SELECT thread_id FROM agent_runs WHERE TRUE",
    );
    append_thread_filters(&mut total_query, query);
    total_query.push(" GROUP BY thread_id) AS matched_threads");
    let total_count = total_query
        .build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .map_err(storage_error)?;

    let mut page_query = QueryBuilder::<Postgres>::new(
        "SELECT r.thread_id, r.source_id, r.project_id, COUNT(*) AS run_count, \
         COUNT(*) FILTER (WHERE NOT EXISTS (SELECT 1 FROM agent_run_events e WHERE e.source_id = r.source_id AND e.run_id = r.run_id AND e.event_type IN ('model.call.started', 'model.call.completed', 'model.call.failed', 'routing.router_invoked', 'tool.call.started', 'tool.call.completed', 'tool.call.failed'))) AS limited_run_count, \
         array_agg(DISTINCT r.agent_id) AS agent_names, MIN(r.created_at) AS created_at, MAX(r.last_activity_at) AS last_activity_at \
         FROM agent_runs r WHERE TRUE",
    );
    append_thread_filters(&mut page_query, query);
    page_query
        .push(" GROUP BY r.thread_id, r.source_id, r.project_id ORDER BY MAX(r.last_activity_at) DESC, r.thread_id ASC LIMIT ")
        .push_bind(i64::from(query.page_size.clamp(1, 100)))
        .push(" OFFSET ")
        .push_bind(i64::from(query.page.saturating_sub(1).saturating_mul(query.page_size.clamp(1, 100))));
    let thread_rows = page_query
        .build_query_as::<DbThreadSummary>()
        .fetch_all(pool)
        .await
        .map_err(storage_error)?;
    let thread_ids = thread_rows
        .iter()
        .map(|thread_row| thread_row.thread_id.clone())
        .collect::<Vec<_>>();
    let status_counts_by_thread = load_status_counts(pool, &thread_ids).await?;
    let thread_summaries = thread_rows
        .into_iter()
        .map(|thread_row| {
            let status_counts = status_counts_by_thread
                .get(&thread_row.thread_id)
                .cloned()
                .unwrap_or_default();
            ThreadSummary {
                thread_id: thread_row.thread_id,
                source_id: thread_row.source_id,
                project_id: thread_row.project_id,
                run_count: u64::try_from(thread_row.run_count).unwrap_or_default(),
                limited_run_count: u64::try_from(thread_row.limited_run_count).unwrap_or_default(),
                agent_names: thread_row.agent_names,
                status_counts,
                created_at: thread_row.created_at,
                last_activity_at: thread_row.last_activity_at,
            }
        })
        .collect();
    Ok(Page {
        items: thread_summaries,
        total: u64::try_from(total_count).unwrap_or_default(),
        page: query.page.max(1),
        page_size: query.page_size.clamp(1, 100),
    })
}

/// 读取指定 thread 下所有 Run 与事件。
pub(super) async fn get_thread(
    pool: &PgPool,
    thread_id: &str,
) -> Result<Vec<(RunRecord, Vec<StoredEvent>)>, TraceStoreError> {
    let database_rows = sqlx::query_as::<_, DbRun>(
        "SELECT r.run_id, r.source_id, r.project_id, r.thread_id, r.agent_id, r.status, r.created_at, r.started_at, r.finished_at, r.trace_id, r.accepted_through_seq, r.last_activity_at FROM agent_threads t JOIN agent_runs r ON r.thread_id = t.thread_id AND r.source_id = t.source_id WHERE t.thread_id = $1 ORDER BY r.created_at ASC, r.run_id ASC",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await
    .map_err(storage_error)?;
    if database_rows.is_empty() {
        return Err(TraceStoreError::NotFound);
    }
    let run_records = database_rows
        .into_iter()
        .map(map_run)
        .collect::<Result<Vec<_>, _>>()?;
    let run_ids = run_records
        .iter()
        .map(|run_record| run_record.run_id.clone())
        .collect::<Vec<_>>();
    let mut events_by_run_id = load_events_for_runs(pool, &run_ids).await?;
    Ok(run_records
        .into_iter()
        .map(|run_record| {
            let run_events = events_by_run_id
                .remove(&run_record.run_id)
                .unwrap_or_default();
            (run_record, run_events)
        })
        .collect())
}

/// 成功准备下载响应后写入审计元数据。
pub(super) async fn record_export(
    pool: &PgPool,
    audit: &ExportAudit,
) -> Result<(), TraceStoreError> {
    sqlx::query(
        "INSERT INTO agent_run_export_audit (source_id, run_id, admin_user_id, event_count, byte_count, exported_at) \
         SELECT source_id, run_id, $2, $3, $4, $5 FROM agent_runs WHERE run_id = $1",
    )
    .bind(&audit.run_id)
    .bind(&audit.admin_user_id)
    .bind(i32::try_from(audit.event_count).map_err(|_| TraceStoreError::ExportTooLarge)?)
    .bind(i64::try_from(audit.byte_count).map_err(|_| TraceStoreError::ExportTooLarge)?)
    .bind(audit.exported_at)
    .execute(pool)
    .await
    .map_err(storage_error)
    .and_then(|query_result| {
        if query_result.rows_affected() == 1 {
            Ok(())
        } else {
            Err(TraceStoreError::NotFound)
        }
    })
}

/// 在事件写入事务中读取刚提交或即将一同提交的单 Run 事件。
pub(super) async fn load_events(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: &str,
) -> Result<Vec<StoredEvent>, TraceStoreError> {
    load_events_from_executor(&mut **transaction, run_id).await
}

async fn load_events_from_pool(
    pool: &PgPool,
    run_id: &str,
) -> Result<Vec<StoredEvent>, TraceStoreError> {
    load_events_from_executor(pool, run_id).await
}

async fn load_events_from_executor<'query, ExecutorType>(
    executor: ExecutorType,
    run_id: &str,
) -> Result<Vec<StoredEvent>, TraceStoreError>
where
    ExecutorType: Executor<'query, Database = Postgres>,
{
    let database_rows = sqlx::query_as::<_, DbStoredEvent>(
        "SELECT e.run_id, e.source_id, r.project_id, e.seq, e.event_type, e.occurred_at, e.recorded_at, e.trace_id, e.span_id, e.parent_span_id, e.payload \
         FROM agent_run_events e JOIN agent_runs r ON r.source_id = e.source_id AND r.run_id = e.run_id \
         WHERE e.run_id = $1 ORDER BY e.seq ASC",
    )
    .bind(run_id)
    .fetch_all(executor)
    .await
    .map_err(storage_error)?;
    database_rows.into_iter().map(map_event).collect()
}

async fn load_events_for_runs(
    pool: &PgPool,
    run_ids: &[String],
) -> Result<BTreeMap<String, Vec<StoredEvent>>, TraceStoreError> {
    if run_ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let database_rows = sqlx::query_as::<_, DbStoredEvent>(
        "SELECT e.run_id, e.source_id, r.project_id, e.seq, e.event_type, e.occurred_at, e.recorded_at, e.trace_id, e.span_id, e.parent_span_id, e.payload \
         FROM agent_run_events e JOIN agent_runs r ON r.source_id = e.source_id AND r.run_id = e.run_id \
         WHERE e.run_id = ANY($1) ORDER BY e.run_id ASC, e.seq ASC",
    )
    .bind(run_ids.to_vec())
    .fetch_all(pool)
    .await
    .map_err(storage_error)?;
    let mut events_by_run_id = BTreeMap::<String, Vec<StoredEvent>>::new();
    for database_row in database_rows {
        let stored_event = map_event(database_row)?;
        events_by_run_id
            .entry(stored_event.run_id.clone())
            .or_default()
            .push(stored_event);
    }
    Ok(events_by_run_id)
}

async fn load_status_counts(
    pool: &PgPool,
    thread_ids: &[String],
) -> Result<BTreeMap<String, BTreeMap<String, u64>>, TraceStoreError> {
    if thread_ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let database_rows = sqlx::query_as::<_, DbThreadStatusCount>(
        "SELECT thread_id, status, COUNT(*) AS run_count FROM agent_runs WHERE thread_id = ANY($1) GROUP BY thread_id, status",
    )
    .bind(thread_ids.to_vec())
    .fetch_all(pool)
    .await
    .map_err(storage_error)?;
    let mut status_counts_by_thread = BTreeMap::<String, BTreeMap<String, u64>>::new();
    for database_row in database_rows {
        status_counts_by_thread
            .entry(database_row.thread_id)
            .or_default()
            .insert(
                database_row.status,
                u64::try_from(database_row.run_count).unwrap_or_default(),
            );
    }
    Ok(status_counts_by_thread)
}

fn append_run_filters(query_builder: &mut QueryBuilder<Postgres>, query: &RunQuery) {
    if let Some(run_id) = &query.run_id {
        query_builder
            .push(" AND run_id = ")
            .push_bind(run_id.clone());
    }
    if let Some(agent_id) = &query.agent_id {
        query_builder
            .push(" AND agent_id = ")
            .push_bind(agent_id.clone());
    }
    if let Some(status) = query.status {
        query_builder
            .push(" AND status = ")
            .push_bind(status.as_str().to_owned());
    }
    if let Some(created_after) = query.created_after {
        query_builder
            .push(" AND created_at >= ")
            .push_bind(created_after);
    }
    if let Some(created_before) = query.created_before {
        query_builder
            .push(" AND created_at <= ")
            .push_bind(created_before);
    }
    if let Some(search_text) = query.search.as_deref() {
        let search_pattern = format!("%{}%", escape_like(search_text));
        query_builder
            .push(" AND (run_id ILIKE ")
            .push_bind(search_pattern.clone())
            .push(" ESCAPE E'\\\\' OR source_id ILIKE ")
            .push_bind(search_pattern.clone())
            .push(" ESCAPE E'\\\\' OR project_id ILIKE ")
            .push_bind(search_pattern.clone())
            .push(" ESCAPE E'\\\\' OR agent_id ILIKE ")
            .push_bind(search_pattern)
            .push(" ESCAPE E'\\\\')");
    }
}

fn append_thread_filters(query_builder: &mut QueryBuilder<Postgres>, query: &ThreadQuery) {
    if let Some(agent_id) = &query.agent_id {
        query_builder
            .push(" AND agent_id = ")
            .push_bind(agent_id.clone());
    }
    if let Some(status) = query.status {
        query_builder
            .push(" AND status = ")
            .push_bind(status.as_str().to_owned());
    }
    if let Some(created_after) = query.created_after {
        query_builder
            .push(" AND created_at >= ")
            .push_bind(created_after);
    }
    if let Some(created_before) = query.created_before {
        query_builder
            .push(" AND created_at <= ")
            .push_bind(created_before);
    }
    if let Some(search_text) = query.search.as_deref() {
        let search_pattern = format!("%{}%", escape_like(search_text));
        query_builder
            .push(" AND (thread_id ILIKE ")
            .push_bind(search_pattern.clone())
            .push(" ESCAPE E'\\\\' OR source_id ILIKE ")
            .push_bind(search_pattern.clone())
            .push(" ESCAPE E'\\\\' OR project_id ILIKE ")
            .push_bind(search_pattern.clone())
            .push(" ESCAPE E'\\\\' OR agent_id ILIKE ")
            .push_bind(search_pattern)
            .push(" ESCAPE E'\\\\')");
    }
}

fn escape_like(search_text: &str) -> String {
    search_text
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn map_run(database_row: DbRun) -> Result<RunRecord, TraceStoreError> {
    let status = RunStatus::from_database(&database_row.status)
        .ok_or(TraceStoreError::StorageUnavailable)?;
    Ok(RunRecord {
        run_id: database_row.run_id,
        source_id: database_row.source_id,
        project_id: database_row.project_id,
        thread_id: database_row.thread_id,
        agent_id: database_row.agent_id,
        status,
        created_at: database_row.created_at,
        started_at: database_row.started_at,
        finished_at: database_row.finished_at,
        trace_id: database_row.trace_id,
        accepted_through_seq: u64::try_from(database_row.accepted_through_seq)
            .map_err(|_| TraceStoreError::StorageUnavailable)?,
        last_activity_at: database_row.last_activity_at,
    })
}

fn map_event(database_row: DbStoredEvent) -> Result<StoredEvent, TraceStoreError> {
    Ok(StoredEvent {
        run_id: database_row.run_id,
        source_id: database_row.source_id,
        project_id: database_row.project_id,
        seq: u64::try_from(database_row.seq).map_err(|_| TraceStoreError::StorageUnavailable)?,
        event_type: database_row.event_type,
        occurred_at: database_row.occurred_at,
        recorded_at: database_row.recorded_at,
        trace_id: database_row.trace_id,
        span_id: database_row.span_id,
        parent_span_id: database_row.parent_span_id,
        payload: database_row.payload,
    })
}
