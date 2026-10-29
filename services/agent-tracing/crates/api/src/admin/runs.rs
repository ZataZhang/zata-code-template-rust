//! Run 列表与单条轨迹详情查询。

use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tpl_agent_tracing_core::model::{RunQuery, RunStatus, TraceDetail, TraceSummary};
use tpl_agent_tracing_core::projection::{derive_trace_id, project_run_summary, project_run_trace};

use super::require_admin;
use crate::AppState;
use crate::error::ApiError;

const MAX_QUERY_TEXT_BYTES: usize = 256;
const MAX_SEARCH_TEXT_BYTES: usize = 128;

#[derive(Deserialize)]
pub(crate) struct RunListParams {
    run_id: Option<String>,
    agent_id: Option<String>,
    status: Option<RunStatus>,
    created_after: Option<String>,
    created_before: Option<String>,
    search: Option<String>,
    page: Option<u32>,
    page_size: Option<u32>,
}

#[derive(Serialize)]
pub(crate) struct RunListResponse {
    items: Vec<TraceSummary>,
    total: u64,
    page: u32,
    page_size: u32,
}

pub(crate) async fn list_runs(
    State(app_state): State<AppState>,
    headers: HeaderMap,
    query_result: Result<Query<RunListParams>, QueryRejection>,
) -> Result<Json<RunListResponse>, ApiError> {
    require_admin(&app_state, &headers).await?;
    let Query(query_params) = query_result.map_err(|_| ApiError::query_invalid())?;
    let created_after = parse_optional_timestamp(query_params.created_after.as_deref())?;
    let created_before = parse_optional_timestamp(query_params.created_before.as_deref())?;
    validate_text_field(query_params.run_id.as_deref(), MAX_QUERY_TEXT_BYTES)?;
    validate_text_field(query_params.agent_id.as_deref(), MAX_QUERY_TEXT_BYTES)?;
    validate_text_field(query_params.search.as_deref(), MAX_SEARCH_TEXT_BYTES)?;
    validate_time_range(created_after, created_before)?;
    let run_query = RunQuery {
        run_id: query_params.run_id,
        agent_id: query_params.agent_id,
        status: query_params.status,
        created_after,
        created_before,
        search: query_params.search,
        page: query_params.page.unwrap_or(1).max(1),
        page_size: query_params.page_size.unwrap_or(50).clamp(1, 100),
    };
    let run_page = app_state
        .repository
        .list_runs(&run_query)
        .await
        .map_err(ApiError::from_store)?;
    let run_ids = run_page
        .items
        .iter()
        .map(|run_record| run_record.run_id.clone())
        .collect::<Vec<_>>();
    let activities_by_run_id = app_state
        .repository
        .summarize_run_activity(&run_ids)
        .await
        .map_err(ApiError::from_store)?;
    let summaries = run_page
        .items
        .iter()
        .map(|run_record| {
            let trace_id = run_record
                .trace_id
                .clone()
                .unwrap_or_else(|| derive_trace_id(&run_record.run_id));
            let activity = activities_by_run_id
                .get(&run_record.run_id)
                .cloned()
                .unwrap_or_default();
            project_run_summary(run_record, &trace_id, &activity)
        })
        .collect();
    Ok(Json(RunListResponse {
        items: summaries,
        total: run_page.total,
        page: run_page.page,
        page_size: run_page.page_size,
    }))
}

pub(crate) async fn get_run(
    State(app_state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(run_id): axum::extract::Path<String>,
) -> Result<Json<TraceDetail>, ApiError> {
    require_admin(&app_state, &headers).await?;
    let (run_record, run_events) = app_state
        .repository
        .get_run(&run_id)
        .await
        .map_err(ApiError::from_store)?;
    Ok(Json(project_run_trace(&run_record, &run_events)))
}

pub(super) fn parse_optional_timestamp(
    timestamp_text: Option<&str>,
) -> Result<Option<OffsetDateTime>, ApiError> {
    timestamp_text
        .map(|timestamp| {
            if timestamp.len() > 64 {
                return Err(ApiError::query_invalid());
            }
            OffsetDateTime::parse(timestamp, &Rfc3339).map_err(|_| ApiError::query_invalid())
        })
        .transpose()
}

pub(super) fn validate_time_range(
    created_after: Option<OffsetDateTime>,
    created_before: Option<OffsetDateTime>,
) -> Result<(), ApiError> {
    if created_after
        .zip(created_before)
        .is_some_and(|(start, end)| start > end)
    {
        return Err(ApiError::query_invalid());
    }
    Ok(())
}

pub(super) fn validate_text_field(
    field_text: Option<&str>,
    max_bytes: usize,
) -> Result<(), ApiError> {
    if field_text.is_some_and(|text| text.len() > max_bytes || text.chars().any(char::is_control)) {
        return Err(ApiError::query_invalid());
    }
    Ok(())
}
