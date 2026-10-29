//! Canonical thread 会话分页与事件账本查询。

use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use tpl_agent_tracing_core::model::{RunStatus, ThreadQuery, ThreadSummary, ThreadTraceDetail};
use tpl_agent_tracing_core::projection::{project_run_trace, project_thread_trace};

use super::require_admin;
use super::runs::{parse_optional_timestamp, validate_text_field, validate_time_range};
use crate::AppState;
use crate::error::ApiError;

const MAX_THREAD_SEARCH_BYTES: usize = 128;
const MAX_FILTER_BYTES: usize = 256;

#[derive(Deserialize)]
pub(crate) struct ThreadListParams {
    agent_id: Option<String>,
    status: Option<RunStatus>,
    created_after: Option<String>,
    created_before: Option<String>,
    search: Option<String>,
    page: Option<u32>,
    page_size: Option<u32>,
}

#[derive(Serialize)]
pub(crate) struct ThreadListResponse {
    items: Vec<ThreadSummary>,
    total: u64,
    page: u32,
    page_size: u32,
}

pub(crate) async fn list_threads(
    State(app_state): State<AppState>,
    headers: HeaderMap,
    query_result: Result<Query<ThreadListParams>, QueryRejection>,
) -> Result<Json<ThreadListResponse>, ApiError> {
    require_admin(&app_state, &headers).await?;
    let Query(query_params) = query_result.map_err(|_| ApiError::query_invalid())?;
    let created_after = parse_optional_timestamp(query_params.created_after.as_deref())?;
    let created_before = parse_optional_timestamp(query_params.created_before.as_deref())?;
    validate_text_field(query_params.agent_id.as_deref(), MAX_FILTER_BYTES)?;
    validate_text_field(query_params.search.as_deref(), MAX_THREAD_SEARCH_BYTES)?;
    validate_time_range(created_after, created_before)?;
    let thread_query = ThreadQuery {
        agent_id: query_params.agent_id,
        status: query_params.status,
        created_after,
        created_before,
        search: query_params.search,
        page: query_params.page.unwrap_or(1).max(1),
        page_size: query_params.page_size.unwrap_or(50).clamp(1, 100),
    };
    let thread_page = app_state
        .repository
        .list_threads(&thread_query)
        .await
        .map_err(ApiError::from_store)?;
    Ok(Json(ThreadListResponse {
        items: thread_page.items,
        total: thread_page.total,
        page: thread_page.page,
        page_size: thread_page.page_size,
    }))
}

pub(crate) async fn get_thread(
    State(app_state): State<AppState>,
    headers: HeaderMap,
    Path(thread_id): Path<String>,
) -> Result<Json<ThreadTraceDetail>, ApiError> {
    require_admin(&app_state, &headers).await?;
    validate_text_field(Some(&thread_id), MAX_FILTER_BYTES)?;
    let thread_runs = app_state
        .repository
        .get_thread(&thread_id)
        .await
        .map_err(ApiError::from_store)?;
    let run_details = thread_runs
        .iter()
        .map(|(run_record, run_events)| project_run_trace(run_record, run_events))
        .collect();
    let thread_detail =
        project_thread_trace(&thread_id, run_details, &thread_runs).ok_or_else(|| {
            ApiError::new(
                axum::http::StatusCode::NOT_FOUND,
                "not_found",
                "轨迹记录不存在",
            )
        })?;
    Ok(Json(thread_detail))
}
