//! 已终态 Run 的完整 canonical 事件导出与低敏审计。

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use serde::Serialize;
use time::OffsetDateTime;
use tpl_agent_tracing_core::model::{RunRecord, StoredEvent};
use tpl_agent_tracing_core::ports::ExportAudit;
use tpl_agent_tracing_core::projection::derive_trace_id;

use super::require_admin;
use crate::AppState;
use crate::error::ApiError;

const MAX_EXPORT_EVENTS: usize = 10_000;
const MAX_EXPORT_BYTES: usize = 10 * 1024 * 1024;

#[derive(Serialize)]
struct ExportDocument<'events> {
    schema_version: u16,
    run_id: &'events str,
    source_id: &'events str,
    project_id: &'events str,
    trace_id: String,
    status: &'static str,
    event_count: usize,
    events: &'events [StoredEvent],
}

pub(crate) async fn export_run(
    State(app_state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> Result<Response, ApiError> {
    let admin_identity = require_admin(&app_state, &headers).await?;
    let (run_record, run_events) = app_state
        .repository
        .get_run(&run_id)
        .await
        .map_err(ApiError::from_store)?;
    let export_bytes = create_export_document(&run_record, &run_events)?;
    app_state
        .repository
        .record_export(&ExportAudit {
            run_id: run_record.run_id.clone(),
            admin_user_id: admin_identity.user_id,
            event_count: u32::try_from(run_events.len()).unwrap_or(u32::MAX),
            byte_count: export_bytes.len() as u64,
            exported_at: OffsetDateTime::now_utc(),
        })
        .await
        .map_err(ApiError::from_store)?;
    let mut response_headers = HeaderMap::new();
    response_headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response_headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response_headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response_headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=agent-run-export.json"),
    );
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    Response::builder()
        .status(StatusCode::OK)
        .body(Body::from(export_bytes))
        .map(|mut response| {
            *response.headers_mut() = response_headers;
            response
        })
        .map_err(|_| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "服务器内部错误",
            )
        })
}

fn create_export_document(
    run_record: &RunRecord,
    run_events: &[StoredEvent],
) -> Result<Vec<u8>, ApiError> {
    if !run_record.status.is_terminal()
        || run_events.len() as u64 != run_record.accepted_through_seq
        || run_events
            .iter()
            .enumerate()
            .any(|(index, event)| event.seq != index as u64 + 1)
    {
        return Err(ApiError::from_store(
            tpl_agent_tracing_core::ports::TraceStoreError::ExportNotReady,
        ));
    }
    if run_events.len() > MAX_EXPORT_EVENTS {
        return Err(ApiError::from_store(
            tpl_agent_tracing_core::ports::TraceStoreError::ExportTooLarge,
        ));
    }
    let trace_id = run_record
        .trace_id
        .clone()
        .unwrap_or_else(|| derive_trace_id(&run_record.run_id));
    let export_document = ExportDocument {
        schema_version: 1,
        run_id: &run_record.run_id,
        source_id: &run_record.source_id,
        project_id: &run_record.project_id,
        trace_id,
        status: run_record.status.as_str(),
        event_count: run_events.len(),
        events: run_events,
    };
    let export_bytes = serde_json::to_vec(&export_document).map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "服务器内部错误",
        )
    })?;
    if export_bytes.len() > MAX_EXPORT_BYTES {
        return Err(ApiError::from_store(
            tpl_agent_tracing_core::ports::TraceStoreError::ExportTooLarge,
        ));
    }
    Ok(export_bytes)
}
