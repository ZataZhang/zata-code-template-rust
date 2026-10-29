//! Producer 的版本化 Run 事件追加入口。

use std::sync::Arc;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use sha2::{Digest, Sha256};
use tpl_agent_tracing_core::event::{EventBatch, validate_batch};
use tpl_agent_tracing_core::model::{MAX_PRODUCER_TOKEN_BYTES, MIN_PRODUCER_TOKEN_BYTES};

use crate::AppState;
use crate::error::ApiError;

pub(crate) async fn append_events(
    State(app_state): State<AppState>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
    request_body: Result<Json<EventBatch>, JsonRejection>,
) -> Result<(StatusCode, Json<tpl_agent_tracing_core::model::AppendAck>), ApiError> {
    let request_body = request_body.map_err(|json_rejection| {
        let response_status = json_rejection.status();
        ApiError::invalid_json(response_status)
    })?;
    let event_batch = request_body.0;
    validate_batch(&run_id, &event_batch).map_err(ApiError::from_event_validation)?;
    let bearer_token = bearer_token(&headers).ok_or_else(|| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "producer_unauthorized",
            "producer 凭据无效",
        )
    })?;
    let token_hash = Sha256::digest(bearer_token.as_bytes());
    let producer_identity = app_state
        .repository
        .authenticate_producer(&token_hash)
        .await
        .map_err(ApiError::from_store)?;
    let append_result = app_state
        .repository
        .append_batch(&producer_identity, &run_id, &event_batch.events)
        .await
        .map_err(ApiError::from_store)?;
    let acknowledgement = append_result.acknowledgement.clone();
    if !append_result.inserted_events.is_empty() {
        spawn_best_effort_export(Arc::clone(&app_state.otlp_exporter), append_result);
    }
    Ok((StatusCode::OK, Json(acknowledgement)))
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let authorization_header = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let bearer_token = authorization_header.strip_prefix("Bearer ")?;
    (bearer_token.len() >= MIN_PRODUCER_TOKEN_BYTES
        && bearer_token.len() <= MAX_PRODUCER_TOKEN_BYTES
        && !bearer_token.chars().any(char::is_control))
    .then_some(bearer_token)
}

fn spawn_best_effort_export(
    otlp_exporter: Arc<dyn tpl_agent_tracing_core::ports::OtlpExporter>,
    append_result: tpl_agent_tracing_core::model::AppendResult,
) {
    tokio::spawn(async move {
        if otlp_exporter
            .export_committed_events(&append_result)
            .await
            .is_err()
        {
            tracing::warn!("OTLP span 导出失败；本地事件已提交并保持可查");
        }
    });
}
